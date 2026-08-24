const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const LOOPBACK_LABEL = "System audio (loopback)";

let targets = [];
let running = false;

const el = {};

function q(id) {
  return document.getElementById(id);
}

function setError(message) {
  el.errorMsg.textContent = message ?? "";
}

function setRunning(isRunning) {
  running = isRunning;
  el.statusBadge.textContent = isRunning ? "Streaming" : "Idle";
  el.statusBadge.className = "badge " + (isRunning ? "badge-running" : "badge-idle");
  el.startBtn.disabled = isRunning;
  el.stopBtn.disabled = !isRunning;
  el.sourceSelect.disabled = isRunning;
  el.refreshSources.disabled = isRunning;
  el.bindInput.disabled = isRunning;
  el.framesInput.disabled = isRunning;
  el.targetInput.disabled = isRunning;
  el.addTarget.disabled = isRunning;
  document.querySelectorAll(".chip-remove").forEach((btn) => (btn.disabled = isRunning));
}

function renderTargets() {
  el.targetList.innerHTML = "";
  for (const [index, target] of targets.entries()) {
    const li = document.createElement("li");
    li.className = "chip";
    const span = document.createElement("span");
    span.textContent = target;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "chip-remove";
    remove.textContent = "×";
    remove.disabled = running;
    remove.addEventListener("click", () => {
      targets.splice(index, 1);
      renderTargets();
    });
    li.append(span, remove);
    el.targetList.append(li);
  }
}

async function loadSources() {
  setError(null);
  try {
    const sources = await invoke("list_sources");
    el.sourceSelect.innerHTML = "";
    for (const source of sources) {
      const opt = document.createElement("option");
      opt.value = source;
      opt.textContent = source;
      el.sourceSelect.append(opt);
    }
    if (sources.length === 0) {
      const opt = document.createElement("option");
      opt.value = "";
      opt.textContent = "No input devices found";
      el.sourceSelect.append(opt);
    }
    el.sourceHint.textContent =
      sources[0] === LOOPBACK_LABEL
        ? "First option captures whatever is currently playing; others are physical input devices."
        : "Pick a physical input device (on Linux, choose a monitor source to capture system audio).";
  } catch (err) {
    setError(String(err));
  }
}

function addTarget() {
  const value = el.targetInput.value.trim();
  if (!value) return;
  if (!targets.includes(value)) {
    targets.push(value);
    renderTargets();
  }
  el.targetInput.value = "";
  el.targetInput.focus();
}

async function startSender() {
  setError(null);
  if (targets.length === 0) {
    setError("Add at least one receiver address first.");
    return;
  }
  const cfg = {
    source: el.sourceSelect.value,
    targets,
    bind: el.bindInput.value.trim() || "0.0.0.0:0",
    gain: Number(el.gainSlider.value),
    frames_per_packet: Number(el.framesInput.value) || 160,
  };
  try {
    const info = await invoke("start_sender", { cfg });
    el.statSource.textContent = info.source_name;
    el.statFormat.textContent = `${info.sample_rate} Hz, ${info.channels} ch, ${info.frames_per_packet} frames/packet`;
    el.statLocal.textContent = info.local_addr;
    el.statPackets.textContent = "0";
    setRunning(true);
  } catch (err) {
    setError(String(err));
  }
}

async function stopSender() {
  try {
    await invoke("stop_sender");
  } catch (err) {
    setError(String(err));
  } finally {
    setRunning(false);
  }
}

function onGainInput() {
  const value = Number(el.gainSlider.value);
  el.gainValue.textContent = value.toFixed(2);
  if (running) {
    invoke("set_gain", { gain: value }).catch((err) => setError(String(err)));
  }
}

function onMuteToggle() {
  if (running) {
    invoke("set_muted", { muted: el.muteToggle.checked }).catch((err) => setError(String(err)));
  }
}

window.addEventListener("DOMContentLoaded", () => {
  el.statusBadge = q("status-badge");
  el.sourceSelect = q("source-select");
  el.refreshSources = q("refresh-sources");
  el.sourceHint = q("source-hint");
  el.targetInput = q("target-input");
  el.addTarget = q("add-target");
  el.targetList = q("target-list");
  el.bindInput = q("bind-input");
  el.framesInput = q("frames-input");
  el.gainSlider = q("gain-slider");
  el.gainValue = q("gain-value");
  el.muteToggle = q("mute-toggle");
  el.startBtn = q("start-btn");
  el.stopBtn = q("stop-btn");
  el.errorMsg = q("error-msg");
  el.statSource = q("stat-source");
  el.statFormat = q("stat-format");
  el.statLocal = q("stat-local");
  el.statPackets = q("stat-packets");

  el.refreshSources.addEventListener("click", loadSources);
  el.addTarget.addEventListener("click", addTarget);
  el.targetInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      addTarget();
    }
  });
  el.startBtn.addEventListener("click", startSender);
  el.stopBtn.addEventListener("click", stopSender);
  el.gainSlider.addEventListener("input", onGainInput);
  el.muteToggle.addEventListener("change", onMuteToggle);

  listen("sender://stats", (event) => {
    el.statPackets.textContent = String(event.payload.packets_sent);
  });

  setRunning(false);
  loadSources();
});
