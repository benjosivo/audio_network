const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let running = false;

const el = {};

function q(id) {
  return document.getElementById(id);
}

function setError(message) {
  el.errorMsg.textContent = message ?? "";
}

function setBadge(text, kind) {
  el.statusBadge.textContent = text;
  el.statusBadge.className = "badge badge-" + kind;
}

function setRunning(isRunning) {
  running = isRunning;
  el.startBtn.disabled = isRunning;
  el.stopBtn.disabled = !isRunning;
  el.listenInput.disabled = isRunning;
  el.jitterInput.disabled = isRunning;
  if (!isRunning) {
    setBadge("Idle", "idle");
    el.statFrom.textContent = "-";
    el.statFormat.textContent = "-";
  }
}

async function startReceiver() {
  setError(null);
  const cfg = {
    listen: el.listenInput.value.trim() || "0.0.0.0:9000",
    jitter_ms: Number(el.jitterInput.value) || 40,
    master_volume: Number(el.volumeSlider.value),
    master_mute: el.muteToggle.checked,
  };
  try {
    await invoke("start_receiver", { cfg });
    setRunning(true);
    setBadge("Waiting for stream...", "waiting");
  } catch (err) {
    setError(String(err));
  }
}

async function stopReceiver() {
  try {
    await invoke("stop_receiver");
  } catch (err) {
    setError(String(err));
  } finally {
    setRunning(false);
  }
}

function onVolumeInput() {
  const value = Number(el.volumeSlider.value);
  el.volumeValue.textContent = value.toFixed(2);
  if (running) {
    invoke("set_master_volume", { volume: value }).catch((err) => setError(String(err)));
  }
}

function onMuteToggle() {
  if (running) {
    invoke("set_master_mute", { muted: el.muteToggle.checked }).catch((err) => setError(String(err)));
  }
}

function onReceiverEvent(event) {
  const payload = event.payload;
  switch (payload.status) {
    case "waiting":
      setBadge("Waiting for stream...", "waiting");
      break;
    case "playing":
      setBadge("Playing", "running");
      el.statFrom.textContent = payload.from;
      el.statFormat.textContent = `${payload.sample_rate} Hz, ${payload.channels} ch`;
      el.statReceived.textContent = String(payload.received);
      el.statLate.textContent = String(payload.late_dropped);
      el.statDup.textContent = String(payload.duplicate_dropped);
      el.statUnderruns.textContent = String(payload.underruns);
      break;
    case "error":
      setError(payload.message);
      setRunning(false);
      break;
    case "stopped":
      setRunning(false);
      break;
  }
}

window.addEventListener("DOMContentLoaded", () => {
  el.statusBadge = q("status-badge");
  el.listenInput = q("listen-input");
  el.jitterInput = q("jitter-input");
  el.volumeSlider = q("volume-slider");
  el.volumeValue = q("volume-value");
  el.muteToggle = q("mute-toggle");
  el.startBtn = q("start-btn");
  el.stopBtn = q("stop-btn");
  el.errorMsg = q("error-msg");
  el.statFrom = q("stat-from");
  el.statFormat = q("stat-format");
  el.statReceived = q("stat-received");
  el.statLate = q("stat-late");
  el.statDup = q("stat-dup");
  el.statUnderruns = q("stat-underruns");

  el.startBtn.addEventListener("click", startReceiver);
  el.stopBtn.addEventListener("click", stopReceiver);
  el.volumeSlider.addEventListener("input", onVolumeInput);
  el.muteToggle.addEventListener("change", onMuteToggle);

  listen("receiver://event", onReceiverEvent);

  setRunning(false);
});
