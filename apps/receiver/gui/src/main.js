const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const STORAGE_KEY = "audio-network.receiver-config";

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
    listen: el.listenInput.value.trim() || "0.0.0.0:9000",
    jitter_ms: Number(el.jitterInput.value) || 40,
    master_volume: Number(el.volumeSlider.value),
    master_mute: el.muteToggle.checked,
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

function setRunning(isRunning) {
  running = isRunning;
  el.startBtn.disabled = isRunning;
  el.stopBtn.disabled = !isRunning;
  el.listenInput.disabled = isRunning;
  el.jitterInput.disabled = isRunning;
  if (!isRunning) {
    setBadge("Idle", "idle");
    el.statusLine.textContent = "";
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
    setBadge("Waiting...", "waiting");
    saveConfig();
  } catch (err) {
    setError(String(err));
    openSettings(true);
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
  saveConfig();
}

function onMuteToggle() {
  if (running) {
    invoke("set_master_mute", { muted: el.muteToggle.checked }).catch((err) => setError(String(err)));
  }
  saveConfig();
}

function onReceiverEvent(event) {
  const payload = event.payload;
  switch (payload.status) {
    case "waiting":
      setBadge("Waiting...", "waiting");
      break;
    case "playing":
      setBadge("Playing", "running");
      el.statusLine.textContent = `${payload.from} · ${payload.sample_rate} Hz ${payload.channels}ch · ${payload.received} pkts`;
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

window.addEventListener("DOMContentLoaded", async () => {
  el.statusBadge = q("status-badge");
  el.statusLine = q("status-line");
  el.listenInput = q("listen-input");
  el.jitterInput = q("jitter-input");
  el.volumeSlider = q("volume-slider");
  el.volumeValue = q("volume-value");
  el.muteToggle = q("mute-toggle");
  el.startBtn = q("start-btn");
  el.stopBtn = q("stop-btn");
  el.settingsToggle = q("settings-toggle");
  el.settingsPanel = q("settings-panel");
  el.errorMsg = q("error-msg");

  el.startBtn.addEventListener("click", startReceiver);
  el.stopBtn.addEventListener("click", stopReceiver);
  el.volumeSlider.addEventListener("input", onVolumeInput);
  el.muteToggle.addEventListener("change", onMuteToggle);
  el.settingsToggle.addEventListener("click", () => openSettings(el.settingsPanel.hidden));
  el.listenInput.addEventListener("change", saveConfig);
  el.jitterInput.addEventListener("change", saveConfig);

  listen("receiver://event", onReceiverEvent);

  setRunning(false);

  const saved = loadConfig();
  if (saved) {
    el.listenInput.value = saved.listen || "0.0.0.0:9000";
    el.jitterInput.value = saved.jitter_ms || 40;
    el.volumeSlider.value = saved.master_volume ?? 1;
    el.volumeValue.textContent = Number(el.volumeSlider.value).toFixed(2);
    el.muteToggle.checked = !!saved.master_mute;
  }

  openSettings(false);
  startReceiver();
});
