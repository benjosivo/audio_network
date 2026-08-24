# Sender GUI

Tauri desktop GUI for the audio-network sender: captures one audio source
and streams it over UDP to one or more receivers. Same pipeline as
`apps/sender/native`, wrapped with Start/Stop and live controls instead of
CLI flags.

## Run in development

```
npm install
npm run tauri dev
```

## Build a release bundle

```
npm run tauri build
```

## Audio source

- **Windows**: the source list includes "System audio (loopback)", which
  captures whatever is currently playing on the default output device via
  WASAPI, plus any physical input devices `cpal` finds.
- **Linux**: WASAPI loopback doesn't exist, so only physical input devices
  are listed. To stream system audio (e.g. on CachyOS with PipeWire/
  PulseAudio), pick the monitor source for your output device from the list
  (e.g. `Monitor of Built-in Audio Analog Stereo`) — PipeWire's PulseAudio
  compatibility layer exposes it as a regular input device.

## Linux build prerequisites

Tauri needs the system WebView (WebKitGTK) and a few dev packages. On
Debian/Ubuntu-based distros:

```
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

On Arch-based distros (including CachyOS):

```
sudo pacman -S --needed webkit2gtk-4.1 base-devel curl wget file \
  openssl appmenu-gtk-module libappindicator-gtk3 librsvg
```
