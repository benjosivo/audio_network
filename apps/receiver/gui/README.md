# Receiver GUI

Tauri desktop GUI for the audio-network receiver: listens for a UDP audio
stream, smooths it through a jitter buffer, and plays it out through the
default output device with a local master volume/mute applied. Same
pipeline as `apps/receiver/native`, wrapped with Start/Stop and live
controls instead of CLI flags.

The window is a small panel showing only status, volume, and mute, and it
starts listening automatically as soon as the app launches. Click the ⚙
button to change the listen address or jitter buffer size — that
configuration is remembered in the window's local storage.

## Run in development

```
npm install
npm run tauri dev
```

## Build a release bundle

```
npm run tauri build
```

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
