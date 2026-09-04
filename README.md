# audio-network

Stream audio from one machine to another over a local network: a **sender**
captures an audio source (system output loopback, or a physical input
device) and streams it over UDP; a **receiver** listens for that stream,
smooths it through a jitter buffer, and plays it out.

Both ends ship two ways:

- a native **CLI** (`sender`, `receiver`) — no UI, flags only
- a desktop **GUI** built with [Tauri](https://tauri.app), wrapping the same
  pipeline with Start/Stop and live controls

## Workspace layout

```
crates/
  protocol/            wire protocol: UDP audio packet framing, TCP control messages
  network/              UDP audio transport, jitter buffer, TCP control channel
  audio-core/capture/    AudioSource trait: WASAPI loopback (Windows), cpal input devices
  audio-core/mixer/      sender-side per-source mixing/volume, receiver-side master gain
apps/
  sender/native/         sender CLI
  sender/gui/             sender Tauri GUI (React + Rust)
  receiver/native/       receiver CLI
  receiver/gui/           receiver Tauri GUI (React + Rust)
```

### Protocol

- **Audio (UDP)** — each datagram is one self-contained, fixed-header packet
  (no serde on the hot path). A dropped or reordered datagram never blocks
  later ones. Sample rate and channel count are carried in-band, so a
  receiver configures playback from the first packet with no separate
  negotiation step.
- **Control (TCP)** — length-prefixed, bincode-encoded messages for
  low-volume, reliable exchanges (e.g. remote per-source volume). Master
  volume/mute is deliberately local-only on the receiver and never crosses
  the network.

## Building and running

Requires a recent stable Rust toolchain.

### Quick start (Arch Linux)

`./install.sh` installs system build/runtime dependencies via `pacman`,
builds the whole workspace, and builds + installs both GUI apps as pacman
packages in one go:

```sh
./install.sh              # both apps
./install.sh sender       # sender GUI only
./install.sh receiver     # receiver GUI only
```

It's a thin wrapper around the steps below and the pacman installer
described under [Linux installer (pacman)](#linux-installer-pacman). On
other distros, follow the manual steps in the sections below instead.

### CLI

```sh
cargo build --release

# Receiver: listen on the default port, play what arrives
cargo run -p receiver -- --listen 0.0.0.0:9000

# Sender: capture the default source and stream to one or more receivers
cargo run -p sender -- --target 192.168.1.50:9000
```

Both CLIs run on Windows and Linux. The sender's default capture source is
WASAPI loopback of the default output device on Windows, or the default
input device (via `cpal`) elsewhere. To stream system audio on Linux with
PipeWire/PulseAudio, list devices and pick the monitor source for your
output device:

```sh
cargo run -p sender -- --list-devices
cargo run -p sender -- --device "Monitor of Built-in Audio Analog Stereo" --target 192.168.1.50:9000
```

Run `cargo run -p sender -- --help` or `cargo run -p receiver -- --help` for
the full flag list (gain, jitter buffer depth, frames per packet, etc).

### GUI

Each GUI app has its own README with setup, dev, and build instructions:
[`apps/sender/gui/README.md`](apps/sender/gui/README.md) and
[`apps/receiver/gui/README.md`](apps/receiver/gui/README.md). In short:

```sh
cd apps/sender/gui   # or apps/receiver/gui
npm install
npm run tauri dev
```

### Linux installer (pacman)

**Rule:** Linux installers for the GUI apps are built as native pacman
packages via a `PKGBUILD`, not through Tauri's built-in bundler targets
(`deb`/`rpm`/`AppImage`). See
[`packaging/pacman/`](packaging/pacman/) — one subdirectory per app, each
with a `PKGBUILD` and a `.desktop` file. Each `PKGBUILD` builds straight from
this monorepo checkout (`source=()`, walking back up to the workspace root)
rather than fetching a tarball, since these GUIs are cargo workspace
members, not standalone crates with independent releases.

Build and install with [`packaging/pacman/install.sh`](packaging/pacman/install.sh),
which wraps `makepkg -f` + `sudo pacman -U` for whichever app(s) you pick:

```sh
./packaging/pacman/install.sh sender     # sender only
./packaging/pacman/install.sh receiver   # receiver only
./packaging/pacman/install.sh both       # both (default if no arg given)
```

Or drive `makepkg`/`pacman` yourself per app:

```sh
cd packaging/pacman/audio-network-sender   # or audio-network-receiver
makepkg -f
sudo pacman -U audio-network-sender-*.pkg.tar.zst
```

Uninstall with `sudo pacman -R audio-network-sender audio-network-receiver`.

On Wayland (e.g. KDE), the generated `.desktop` launcher sets
`WEBKIT_DISABLE_DMABUF_RENDERER=1` to avoid a WebKitGTK DMA-BUF crash; set
the same variable if launching the installed binary directly from a
terminal.

## Testing

```sh
cargo test --workspace
```

## License

Dual-licensed under MIT or Apache-2.0, at your option.
