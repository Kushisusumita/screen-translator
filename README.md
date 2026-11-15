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
whole screen — and the translation takes the place of the original: same spot,
same size, same colours, on a plate that continues the panel the text was on.
Click anywhere or press `Esc` and it is gone.

> 🚫 No browser extension. 📋 No copy-paste. 🎮 Made to be used mid-game.

- **Over the original** — a dialogue box stays a dialogue box, only readable.
  Nothing else on the screen is covered, and the game keeps the keyboard.
- **Subtitles** — or have the translation as a line at the bottom of the screen
  instead, the way film subtitles sit, and leave the original untouched.
- **Watch an area** — select the dialogue box once and every new line is
  translated as it appears, with no further key presses.
- **Four engines** — AI, DeepL, Yandex, Google — tried in the order you set; the
  first one that answers wins. An AI model that can see can be handed the
  screenshot itself, which copes with stylised type a recogniser gives up on.

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
2. ⌨️ Press **`Alt+Q`** anywhere on screen.
3. ❄️ The screen **freezes and dims**. Drag a rectangle around the text — or
   press `Tab` to switch to window or full-screen mode.
4. 🖱️ Release the mouse. The translation appears where the text was.
5. ✖️ **Click away** or press **`Esc`** to close it.

---

## 🎛️ Usage

Right-click the tray icon — left-click on macOS — for the capture modes,
**Settings** and **Exit**.

### Keys

Global — they work in any program, and every one can be changed or switched off
in *Settings → Keyboard shortcuts*:

| Key | Does |
|---|---|
| `Alt+Q` | 🖼️ Translate a region you drag |
| `Alt+Shift+W` | 🪟 Translate a window you point at |
| `Alt+Shift+F` | 🖥️ Translate the whole screen the pointer is on |
| `Alt+Shift+R` | 🔁 Translate the last area again, without selecting it |
| `Alt+Shift+Q` | 👁️ Watch an area: translate it again whenever its text changes |
| `Esc` | ✖️ Take the translation off the screen (only while one is showing) |

`F1`–`F12`, `Insert`, `Delete`, `Home`, `End`, `Page Up` and `Page Down` can be
bound on their own, without `Ctrl`, `Alt` or `Shift` — one key is all a hand on
the mouse can spare.

While selecting:

| Key | Does |
|---|---|
| `Tab` | 🔄 Switch capture mode without starting over |
| `Space` (held) | ✋ Move the whole selection instead of resizing it |
| **Right click** | 🧹 Throw the selection away and start a new one |
| `Esc` | 🚪 Abandon the capture entirely |

A crosshair follows the pointer with a live **X / Y readout in captured
pixels** — the same unit as the size badge and the image that comes back, so
what you read is what you get.

### In a game

- The translation never takes the focus: the game does not pause, mute or
  minimise because a window appeared over it.
- **Repeat** (`Alt+Shift+R`) is the one to keep under a finger — dialogue
  comes up in the same box every time, so select it once and repeat from then on.
- **Watch** goes a step further and needs no key at all. It reads the area
  through its own translation, so the two do not get in each other's way; press
  the same shortcut again to stop.
- Text laid straight over the scene, with no panel behind it, is given a plain
  dark caption — there is no surface to continue there.
- If nothing shows up over a game in *exclusive* full-screen, switch the game to
  borderless or windowed full-screen: no desktop window can be drawn over a
  display a game has taken for itself.

### Handy settings

| Setting | Where | Default |
|---|---|---|
| 🪟 How the translation is shown — over the original · subtitles · card · window | *Appearance* | **Over the original** |
| ⏱️ Hide it by itself — never · once read · after a set time | *Appearance* | **Never** |
| 🫥 Hide it when you click somewhere else | *Appearance* | **On** |
| 👻 Let clicks pass through it to the game | *Appearance* | Off |
| 🎬 Subtitle size, position and opacity, with a sample to try | *Appearance → Subtitles* | — |
| 👁️ How often a watched area is checked | *General → Watching an area* | **1 s** |
| 🔔 Notify about new versions | *About → Updates* | **On** |
| 📋 Copy the translation to the clipboard | *General* | Off |
| 🔇 Hide the tray icon (hotkeys keep working) | *General* | Off |
| 🤖 AI endpoint, model and token | *Engine → AI translator* | — |
| 🖼️ Let the AI model read the screenshot itself | *Engine → AI translator* | Off |

**Test connection** on the engine page sends one short translation and reports
what came back, so a wrong key or model shows up immediately instead of at the
next capture.

### Command line

```bash
screen-translator --settings              # open the settings window on launch
screen-translator --settings engine       # open it straight on a given page
screen-translator --region 100,200,800,160  # translate this rectangle right away
screen-translator --help                  # list the options
```

Pages: `general`, `keys`, `languages`, `engine`, `appearance`, `history`, `logs`,
`about`. `--region` takes `left,top,width,height` in desktop pixels — for a
launcher button, a macro key, or a controller mapping.

Set `SAKURA_CONFIG_DIR` to keep settings and logs in a folder of your choosing
instead of the system's usual place: a portable copy, or a second profile.

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
