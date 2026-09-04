const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const LOOPBACK_LABEL = "System audio (loopback)";
const STORAGE_KEY = "audio-network.sender-config";

let targets = [];
let running = false;
let lastInfo = null;

const el = {};

function q(id) {
  return document.getElementById(id);
}

function setError(message) {
  el.errorMsg.textContent = message ?? "";
}

function loadConfig() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function saveConfig() {
  const cfg = {
    source: el.sourceSelect.value,
    targets,
    bind: el.bindInput.value.trim() || "0.0.0.0:0",
    frames_per_packet: Number(el.framesInput.value) || 160,
    gain: Number(el.gainSlider.value),
    muted: el.muteToggle.checked,
  };
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(cfg));
  } catch {
    // ignore storage errors (private mode, quota, ...)
  }
}

function openSettings(open) {
  el.settingsPanel.hidden = !open;
  el.settingsToggle.setAttribute("aria-expanded", String(open));
}

function formatStatus(info, packets) {
  if (!info) return "";
  return `${info.source_name} → ${targets.join(", ")} · ${info.sample_rate} Hz ${info.channels}ch · ${packets} pkts`;
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
  if (!isRunning) {
    lastInfo = null;
    el.statusLine.textContent = "";
  }
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
      saveConfig();
    });
    li.append(span, remove);
    el.targetList.append(li);
  }
}

async function loadSources(preferredSource) {
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
    } else if (preferredSource && sources.includes(preferredSource)) {
      el.sourceSelect.value = preferredSource;
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
    saveConfig();
  }
  el.targetInput.value = "";
  el.targetInput.focus();
}

async function startSender() {
  setError(null);
  if (targets.length === 0) {
    setError("Add at least one receiver address first.");
    openSettings(true);
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
    lastInfo = info;
    setRunning(true);
    el.statusLine.textContent = formatStatus(lastInfo, 0);
    if (el.muteToggle.checked) {
      invoke("set_muted", { muted: true }).catch((err) => setError(String(err)));
    }
    saveConfig();
  } catch (err) {
    setError(String(err));
    openSettings(true);
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
  saveConfig();
}

function onMuteToggle() {
  if (running) {
    invoke("set_muted", { muted: el.muteToggle.checked }).catch((err) => setError(String(err)));
  }
  saveConfig();
}

window.addEventListener("DOMContentLoaded", async () => {
  el.statusBadge = q("status-badge");
  el.statusLine = q("status-line");
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
  el.settingsToggle = q("settings-toggle");
  el.settingsPanel = q("settings-panel");
  el.errorMsg = q("error-msg");

  el.refreshSources.addEventListener("click", () => loadSources());
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
  el.settingsToggle.addEventListener("click", () => openSettings(el.settingsPanel.hidden));
  el.bindInput.addEventListener("change", saveConfig);
  el.framesInput.addEventListener("change", saveConfig);

  listen("sender://stats", (event) => {
    if (running) {
      el.statusLine.textContent = formatStatus(lastInfo, event.payload.packets_sent);
    }
  });

  setRunning(false);

  const saved = loadConfig();
  if (saved) {
    targets = Array.isArray(saved.targets) ? saved.targets : [];
    renderTargets();
    el.bindInput.value = saved.bind || "0.0.0.0:0";
    el.framesInput.value = saved.frames_per_packet || 160;
    el.gainSlider.value = saved.gain ?? 1;
    el.gainValue.textContent = Number(el.gainSlider.value).toFixed(2);
    el.muteToggle.checked = !!saved.muted;
  }

  await loadSources(saved?.source);

  if (saved && targets.length > 0) {
    openSettings(false);
    startSender();
  } else {
    openSettings(true);
  }
});
