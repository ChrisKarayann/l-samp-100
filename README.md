<![CDATA[<div align="center">

# L-SAMP 100

### Broadcast Audio Controller

*A keyboard-triggered sample pad for live performance, built with Rust and Angular.*

<!-- TODO: Replace with actual screenshot -->
<!-- ![L-SAMP 100 Interface](docs/screenshot.png) -->

[![Release](https://img.shields.io/github/v/release/user/lsamp-100?style=flat-square&color=00ffcc)](https://github.com/user/lsamp-100/releases)
[![License](https://img.shields.io/badge/license-MIT-333333?style=flat-square)](LICENSE)

</div>

---

<!-- TODO: Replace with your demo video link -->
<!-- https://github.com/user/lsamp-100/assets/.../demo.mp4 -->
<!-- Or use a GIF: ![Demo](docs/demo.gif) -->

> **🎬 Demo video placeholder** — A short recording of the boot sequence, pad triggering, BPM sync, and theme switching goes here.

---

## What is this?

L-SAMP 100 is a **dedicated keyboard sampler** designed for real-time audio triggering. Load audio files onto 12 pads, trigger them with your QWERTY keyboard (Q W E R / A S D F / Z X C V), shape the sound with attack/release envelopes, and synchronize everything to a master clock.

It's not a DAW. It's not a plugin host. It's a **single-purpose instrument** — the kind of tool a radio broadcaster, theater sound designer, DJ, or live performer keeps open and ready.

### Features

- **12-Pad Grid** — Configurable 1×4, 2×4, or 3×4 layout
- **Rust Audio Engine** — Sample-accurate playback with real-time resampling and linear interpolation
- **Attack/Release Envelopes** — Per-pad fade shaping with symmetric fade-out protection
- **Waveform Navigation** — Non-destructive trim markers (in/out points) with visual waveform display
- **BPM Detection** — Automatic tempo analysis on every loaded sample
- **Master Clock** — Manual (120 BPM default) or Auto (average BPM of all loaded samples)
- **Per-Pad Sync** — Lock any pad to the master clock for pitch-shifted tempo matching
- **Loop & One-Shot Modes** — Toggle per pad, with jitter-free mid-playback transitions
- **Real-Time Oscilloscopes** — Per-pad mini waveform monitors
- **3 Chromatic Themes** — Teal, Amber, Marooned — with live color morphing
- **Persistent State** — All pad configurations, labels, and settings cached automatically
- **Global Keyboard Capture** — OS-level key listening (toggle on/off)
- **Cross-Platform** — Linux, macOS, Windows

### Supported Formats

MP3 · WAV · OGG · FLAC

---

## Quick Start

### Download

Pre-built binaries for Linux, macOS, and Windows are available on the [Releases](https://github.com/user/lsamp-100/releases) page.

### Build from Source

**Prerequisites:** [Rust](https://rustup.rs/), [Node.js](https://nodejs.org/) (LTS), and platform-specific dependencies for Tauri.

<details>
<summary><strong>Linux dependencies</strong></summary>

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev
```
</details>

```bash
# Clone the repository
git clone https://github.com/user/lsamp-100.git
cd lsamp-100

# Install frontend dependencies
npm install

# Run in development mode
npm run tauri dev

# Build for production
npm run tauri-build
```

### Loading Samples

Audio files go in the **Harbor** directory:

| Platform | Path |
|---|---|
| Linux | `~/.config/lsamp-100/audio/` |
| macOS | `~/Library/Application Support/lsamp-100/audio/` |
| Windows | `%APPDATA%\lsamp-100\audio\` |

Subdirectories within Harbor are displayed as grouped categories in the file selector. You can also inject files from any location using the **Inject External** option.

---

## Controls

| Key | Action |
|---|---|
| `Q` `W` `E` `R` | Trigger pads (row 1) |
| `A` `S` `D` `F` | Trigger pads (row 2) |
| `Z` `X` `C` `V` | Trigger pads (row 3) |
| `Space` | Global stop (all pads) |
| Mouse wheel | Adjust gain, fades, trim, and BPM with encoder-like precision |
| Click pad | Select pad for calibration panel |

Pressing a pad key while it's playing will stop it with its configured release envelope. The **Capture Keyboard** toggle controls whether the app listens for keypresses globally or only when focused.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│                  Angular 20                      │
│           (Zoneless / Signals)                   │
│                                                  │
│  app.ts ─── The Conductor                        │
│  app.html ── The Surface                         │
│  app.scss ── The Identity                        │
│                                                  │
│  services/                                       │
│    audio.ts ─────── Signal Chain Manager          │
│    tauri-bridge ─── IPC Adapter                   │
├──────────────────── Tauri IPC ───────────────────┤
│                   Rust Backend                    │
│                                                  │
│  main.rs ─────── Hardware Bridge & File System    │
│  audio_engine.rs ─ Real-Time Audio Callback       │
│                    (cpal + symphonia)              │
│                                                  │
│  ┌─────────────────────────────────┐             │
│  │  write_audio() — per-frame:    │             │
│  │  • Voice mixing                │             │
│  │  • Linear interpolation        │             │
│  │  • Envelope (attack/release)   │             │
│  │  • BPM-synced resampling       │             │
│  │  • Loop boundary management    │             │
│  │  • Peak metering               │             │
│  └─────────────────────────────────┘             │
└─────────────────────────────────────────────────┘
```

**Key design decisions:**

- **Audio stays in Rust.** All sample playback, mixing, envelope shaping, and resampling runs in a native `cpal` audio callback. No Web Audio API.
- **UI stays in Angular.** All state management, visualization, and user interaction runs in the frontend via Angular Signals. No Zone.js.
- **The bridge is thin.** `TauriBridgeService` is a 450-line IPC adapter with no business logic.
- **The frontend is a monolith.** `app.ts` (1,054 lines) is a single-file conductor — intentionally. It reads like a score, not a manual.

### Dependencies

| Crate | Purpose |
|---|---|
| `cpal` | Cross-platform audio output |
| `symphonia` | Audio file decoding (MP3, WAV, OGG, FLAC) |
| `stratum-dsp` | BPM detection and audio analysis |
| `rdev` | OS-level keyboard capture |
| `tauri` | Native app shell and IPC |
| `rfd` | Native file dialogs |

---

## Themes

The interface supports three chromatic identities, switchable live from Settings:

| Teal | Amber | Marooned |
|:---:|:---:|:---:|
| `#00ffcc` | `#ffbf00` | `#ff4d4d` |

All UI elements — borders, glows, accents, oscilloscopes, waveforms — respond to the active theme in real time.

---

## Credits

| | |
|---|---|
| **Engineering & Logic** | Chris Karayannidis |
| **Produced by** | Liturgy // Progressive Perceptions |
| **Technical Collaboration** | AI-assisted development |

---

## License

This project is released under the [MIT License](LICENSE).

---

<div align="center">
<sub>ALL SIGNAL PATHS NOMINAL</sub>
</div>
]]>
