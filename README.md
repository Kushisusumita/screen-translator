<div align="center">

# Screen Translator

**Instant OCR translation for anything on your screen — no copy-paste required.**

[![Release](https://github.com/Kushisusumita/screen-translator/actions/workflows/release.yml/badge.svg)](https://github.com/Kushisusumita/screen-translator/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Windows](https://img.shields.io/badge/Platform-Windows%2010%2F11-0078D4?logo=windows)](https://github.com/Kushisusumita/screen-translator/releases)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust%201.75%2B-CE422B?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![GitHub stars](https://img.shields.io/github/stars/Kushisusumita/screen-translator?style=flat&color=yellow)](https://github.com/Kushisusumita/screen-translator/stargazers)
[![GitHub issues](https://img.shields.io/github/issues/Kushisusumita/screen-translator)](https://github.com/Kushisusumita/screen-translator/issues)
[![GitHub last commit](https://img.shields.io/github/last-commit/Kushisusumita/screen-translator)](https://github.com/Kushisusumita/screen-translator/commits/main)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

[Download](#download) · [Usage](#usage) · [Build from source](#building-from-source) · [Contributing](#contributing) · [License](#license)

</div>

---

## What it does

Press a global hotkey, drag a rectangle around any on-screen text (games, videos, PDFs, anything), and a full-screen translation overlay appears instantly. Click anywhere or press `ESC` to dismiss it.

No browser extension. No copy-paste. Works in fullscreen apps and games.

---

## Features

| Feature | Details |
|---|---|
| **Global hotkey** | Default `Ctrl+T` — configurable, works everywhere including fullscreen |
| **Region selection** | Freeze-frame overlay with live selection rectangle and size label |
| **OCR** | Yandex OCR API with automatic language detection |
| **Translation** | Yandex web · Yandex API · Google Translate — tried in order, first success wins |
| **Full-screen overlay** | Semi-transparent backdrop, text at top-left, scrollable if long |
| **System tray only** | Zero taskbar presence, minimal CPU/RAM footprint |
| **Clipboard** | Optional: copy translation automatically on each capture |
| **Autostart** | Optional Windows startup entry |
| **12 languages** | EN · RU · DE · FR · ES · ZH · JA · KO · AR · PT · IT · TR |

---

## Download

Pre-built binaries for Windows x64 are on the [**Releases**](https://github.com/Kushisusumita/screen-translator/releases) page.

No installer — just run `screen-translator.exe`.

---

## Usage

1. The app starts silently in the **system tray** (near the clock).
2. Press **`Ctrl+T`** (or your configured hotkey) anywhere on screen.
3. The screen **freezes and dims**. Drag a rectangle around the text you want translated.
4. Release the mouse. A **full-screen overlay** appears with the translation at the top-left.
5. **Click anywhere** on the overlay or press **`ESC`** to close it.

**Right-click the tray icon → Settings** to change the hotkey, language pair, or other options.

---

## Requirements

- Windows 10 or 11 (x64)
- Internet connection (Yandex OCR, Yandex Translate, Google Translate)

---

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) 1.75+ stable toolchain
- Visual Studio Build Tools with the **MSVC** compiler and Windows SDK

### Steps

```bash
git clone https://github.com/Kushisusumita/screen-translator.git
cd screen-translator
cargo build --release
```

Binary output: `target/release/screen-translator.exe`

> Commit `Cargo.lock` to your fork so CI builds are reproducible.

---

## Project Structure

The codebase follows **Feature-Sliced Design (FSD)** and **SOLID** principles:

```
src/
├── main.rs                   # Entry point — 1×1 host window, eframe bootstrap
├── app/mod.rs                # App orchestrator (eframe::App impl)
├── entities/                 # Pure data types with no side-effects
│   ├── language.rs           #   Language enum + ISO codes
│   └── settings.rs           #   Settings struct
├── features/                 # Independent vertical slices
│   ├── capture/              #   GDI screenshot + fullscreen overlay UI
│   ├── hotkey/               #   Win32 global hotkey registration thread
│   ├── settings/             #   Settings persistence (TOML) + settings UI
│   ├── tooltip/              #   Full-screen translation overlay rendering
│   ├── tray/                 #   System tray icon + context menu
│   └── translation/          #   OCR → translate pipeline
│       ├── pipeline.rs       #     Public entry point: run_pipeline(jpeg, src, tgt)
│       ├── ocr.rs            #     Yandex OCR API
│       ├── translator.rs     #     Yandex web · Yandex API · Google fallback chain
│       └── client.rs         #     Shared HTTP client + session token generation
└── shared/                   # Cross-cutting utilities
    ├── error.rs              #   AppError with From impls for all used error types
    └── utils/                #   Clipboard, autostart helpers
```

**Layer rules:** `app` → `features` → `entities` → `shared`. Features never import from each other or from `app`.

---

## Contributing

Contributions, issues, and pull requests are welcome and encouraged.

**To contribute:**

1. Fork the repository
2. Create a feature branch: `git checkout -b feat/my-improvement`
3. Commit using [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, etc.
4. Open a pull request with a clear description of what and why

**You are explicitly allowed to:**
- Fork this project and modify it for any purpose, including commercial use (per MIT License)
- Submit PRs with new languages, translation backends, UI improvements, bug fixes, or platform support
- Redistribute modified versions as long as the original MIT license and copyright notice are retained

There is no formal contributor agreement beyond the MIT license itself. By submitting a PR, you agree that your contribution will be licensed under MIT.

**Good first issues:**
- Add a new translation backend
- Improve OCR language detection fallback
- Add keyboard navigation to the overlay
- Package a proper Windows installer

---

## Versioning

Releases are created automatically on every push to `main`:

| Commit prefix | Version bump |
|---|---|
| `fix:` `docs:` `refactor:` `chore:` | Patch `0.0.x` |
| `feat:` | Minor `0.x.0` |
| `feat!:` `fix!:` `BREAKING CHANGE` | Major `x.0.0` |

---

## Disclaimer

> **This software is provided "as is", without warranty of any kind.**

- The author makes **no guarantees** about accuracy of translations, OCR recognition quality, or service availability (Yandex, Google).
- The author is **not liable** for any damage, data loss, or consequences arising from use or inability to use this software.
- This project uses **unofficial, undocumented APIs** of third-party services. These may break, change, or become unavailable at any time without notice.
- The author has **no obligation** to fix bugs, add features, respond to issues, maintain compatibility, or continue development of this project in any form.
- Use of third-party translation and OCR services is subject to their respective **Terms of Service**. You are responsible for ensuring your use complies with those terms.
- This tool is intended for **personal, non-commercial use** as a productivity aid. The author is not responsible for any misuse.

By using this software you accept these terms.

---

## Author

[@クシススミタ](https://github.com/Kushisusumita)

---

## License

[MIT](LICENSE) © 2026 クシススミタ
