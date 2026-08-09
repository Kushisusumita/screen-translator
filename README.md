<div align="center">

<img src="assets/icon.png" width="120" alt="Sakura Screen Translator">

# 🌸 Sakura Screen Translator

**Instant OCR translation for anything on your screen — no copy-paste required.**

[![CI](https://github.com/Kushisusumita/screen-translator/actions/workflows/ci.yml/badge.svg)](https://github.com/Kushisusumita/screen-translator/actions/workflows/ci.yml)
[![Release](https://github.com/Kushisusumita/screen-translator/actions/workflows/release.yml/badge.svg)](https://github.com/Kushisusumita/screen-translator/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Windows | macOS | Linux](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-555555)](https://github.com/Kushisusumita/screen-translator/releases)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust%201.75%2B-CE422B?logo=rust&logoColor=white)](https://www.rust-lang.org/)

[**⚡ Quick start**](#-quick-start) ·
[**🎛️ Usage**](#-usage) ·
[**📋 Requirements**](#-requirements) ·
[**🛠️ Build**](#-building-from-source) ·
[**📄 License**](#-license)

</div>

---

## 🎯 What it does

Press a global hotkey, point at some text — a region you drag, a window, or the
whole screen — and the translation appears next to it. Click anywhere or press
`Esc` to dismiss.

> 🚫 No browser extension. 📋 No copy-paste. 🎮 Works in fullscreen apps and games.

Four translation engines — AI, DeepL, Yandex, Google — are tried in the order you
set, and the first one that answers wins.

---

## ⚡ Quick start

Pre-built binaries for **Windows, macOS and Linux** are on the
[**Releases**](https://github.com/Kushisusumita/screen-translator/releases) page —
no installer, just run the file. The macOS build is universal, so one download
covers Apple silicon and Intel. Updating later is a button in *About*: the app
fetches the build for its own platform, swaps itself out and restarts. Or build
it yourself:

```bash
# 1. Build it
cargo build --release

# 2. Run it — it lives in the tray / menu bar, not on the taskbar
./target/release/screen-translator
```

1. 🔔 The app starts silently in the **system tray** (Windows/Linux) or the
   **menu bar** (macOS).
2. ⌨️ Press **`Ctrl+T`** anywhere on screen.
3. ❄️ The screen **freezes and dims**. Drag a rectangle around the text — or
   press `Tab` to switch to window or full-screen mode.
4. 🖱️ Release the mouse. The translation appears next to the selection.
5. ✖️ **Click away** or press **`Esc`** to close it.

---

## 🎛️ Usage

Right-click the tray icon — left-click on macOS — for the capture modes,
**Settings** and **Exit**.

### Capture keys

| Key | Does |
|---|---|
| `Ctrl+T` | 🖼️ Capture a region you drag |
| `Ctrl+Shift+W` | 🪟 Capture a window you point at |
| `Ctrl+Shift+S` | 🖥️ Capture the whole screen |
| `Tab` | 🔄 Switch capture mode without starting over |
| `Space` (held) | ✋ Move the whole selection instead of resizing it |
| **Right click** | 🧹 Throw the selection away and start a new one |
| `Esc` | 🚪 Abandon the capture entirely |

A crosshair follows the pointer with a live **X / Y readout in captured
pixels** — the same unit as the size badge and the image that comes back, so
what you read is what you get.

### Handy settings

| Setting | Where | Default |
|---|---|---|
| 🪟 Result view — popup · over the original · window | *Appearance* | **Over the original** |
| 📌 Keep the result window on top | *Appearance → Result window* | **On** |
| 🫥 Close the result when you click away | *Appearance → Result window* | **On** |
| 🔔 Notify about new versions | *About → Updates* | **On** |
| 📋 Copy the translation to the clipboard | *General* | Off |
| 🔇 Hide the tray icon (hotkeys keep working) | *General* | Off |
| 🤖 AI endpoint, model and token | *Engine → AI translator* | — |

**Test connection** on the engine page sends one short translation and reports
what came back, so a wrong key or model shows up immediately instead of at the
next capture.

### Command line

```bash
screen-translator --settings          # open the settings window on launch
screen-translator --settings engine   # open it straight on a given page
screen-translator --help              # list the options
```

Pages: `general`, `keys`, `languages`, `engine`, `appearance`, `logs`, `about`.

> 🌏 The interface follows the system language, with fourteen to choose from in
> *General → Interface language*. The English section names above are what the
> command line takes.

---

## 📋 Requirements

- 💻 Windows 10 or 11 (x64), macOS 13+, or a Linux desktop (X11, or Wayland with
  the screencast portal)
- 🌐 Internet connection, unless you point the AI engine at a local model

<details>
<summary>🍎 <b>macOS</b> — two permissions, asked for on first use</summary>

In *System Settings → Privacy & Security*:

- **Screen Recording** — without it every capture comes back blank;
- **Accessibility** — only if a global shortcut refuses to fire.

The app runs as a **menu-bar agent**: no Dock icon, and — the reason it matters —
no Space of its own, so the capture overlay appears over the full-screen app you
are actually looking at instead of switching you away from it.

</details>

<details>
<summary>🐧 <b>Linux</b> — optional desktop services</summary>

- a **Secret Service** (GNOME Keyring, KWallet) for token encryption;
- an **appindicator-capable panel** for the tray icon;
- **libnotify** (`notify-send`) for update notices;
- **speech-dispatcher** (`spd-say`) for read-aloud.

Missing any of them is not fatal: the app says so in the settings window and
keeps the shortcuts working.

</details>

---

## 🛠️ Building from source

### Prerequisites

- 🦀 [Rust](https://rustup.rs/) 1.75+ stable toolchain
- 🪟 **Windows** — Visual Studio Build Tools with the **MSVC** compiler and Windows SDK
- 🍎 **macOS** — Xcode command line tools (`xcode-select --install`)
- 🐧 **Linux** — the desktop development headers:

```bash
sudo apt install pkg-config clang libclang-dev libgtk-3-dev \
  libxcb1-dev libxcb-randr0-dev libxcb-shm0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev libgl1-mesa-dev \
  libayatana-appindicator3-dev libpipewire-0.3-dev libdbus-1-dev \
  libgbm-dev libxdo-dev
```

### Steps

```bash
cargo build --release
```

Binary output: `target/release/screen-translator` (`.exe` on Windows)

```bash
cargo test              # unit tests, no display needed
cargo test -- --ignored # capture checked against the real screen
```

> 🤖 CI builds and tests on **Windows, macOS and Linux** on every push, so the
> platform split cannot quietly rot. Releases are cut from `main` only.

---

## 🔖 Versioning

`Cargo.toml` is the single source of truth. A push to `main` releases whatever
version it names — tagged `vX.Y.Z`, published as *Sakura Screen Translator
X.Y.Z*, and reported by the app itself on the About page, so the three can never
disagree.

Cutting a release is therefore one edit: bump the version, push. If the tag
already exists the workflow stops rather than overwrite it, so a push that
forgot the bump fails loudly instead of quietly replacing yesterday's binaries.

---

## ⚠️ Disclaimer

> **This software is provided "as is", without warranty of any kind.**

- The author makes **no guarantees** about accuracy of translations, OCR recognition quality, or service availability.
- The author is **not liable** for any damage, data loss, or consequences arising from use or inability to use this software.
- This project uses **unofficial, undocumented APIs** of third-party services. These may break, change, or become unavailable at any time without notice.
- The author has **no obligation** to fix bugs, add features, respond to issues, maintain compatibility, or continue development of this project in any form.
- Use of third-party translation, OCR and AI services is subject to their respective **Terms of Service**, and you are billed by them directly for any key you configure here. You are responsible for ensuring your use complies with those terms.
- This tool is intended for **personal, non-commercial use** as a productivity aid. The author is not responsible for any misuse.

By using this software you accept these terms.

---

## 👤 Author

[@クシススミタ](https://github.com/Kushisusumita)

---

## 📄 License

[MIT](LICENSE) © 2026 クシススミタ

<div align="center">

**⭐ If this saved you some copy-paste, a star is welcome.**

</div>
