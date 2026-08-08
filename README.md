<div align="center">

# Sakura Screen Translator

**Instant OCR translation for anything on your screen — no copy-paste required.**

[![Release](https://github.com/Kushisusumita/screen-translator/actions/workflows/release.yml/badge.svg)](https://github.com/Kushisusumita/screen-translator/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Windows](https://img.shields.io/badge/Platform-Windows%2010%2F11-0078D4?logo=windows)](https://github.com/Kushisusumita/screen-translator/releases)
[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust%201.75%2B-CE422B?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![GitHub stars](https://img.shields.io/github/stars/Kushisusumita/screen-translator?style=flat&color=yellow)](https://github.com/Kushisusumita/screen-translator/stargazers)
[![GitHub issues](https://img.shields.io/github/issues/Kushisusumita/screen-translator)](https://github.com/Kushisusumita/screen-translator/issues)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

[Download](#download) · [Usage](#usage) · [Translation engines](#translation-engines) · [Build from source](#building-from-source) · [Contributing](#contributing) · [License](#license)

</div>

---

## What it does

Press a global hotkey, point at some text — a region you drag, a window, or the
whole screen — and the translation appears next to it. Click anywhere or press
`Esc` to dismiss.

No browser extension. No copy-paste. Works in fullscreen apps and games.

---

## Features

| Feature | Details |
|---|---|
| **Three capture modes** | Region (`Ctrl+T`), window (`Ctrl+Shift+W`), full screen (`Ctrl+Shift+S`) — switch mid-capture with `Tab` |
| **Three ways to show the result** | Glass popup at the selection · translation painted over the original · floating two-column window that can stay on top |
| **Bring your own AI** | Any model with a token: OpenAI-compatible, Anthropic, or Gemini — including local Ollama and LM Studio |
| **Four engines, in your order** | AI · DeepL · Yandex · Google, tried top to bottom until one answers |
| **OCR repair** | Line wraps rejoined, hyphenation undone, small text upscaled before recognition |
| **31 languages** | Source detected automatically, or pinned |
| **Light and dark** | Follows the system appearance, or force one |
| **Tokens encrypted at rest** | API keys sealed with Windows DPAPI, never written to the config in the clear |
| **Bounded logs** | One file per day, older ones deleted automatically, screen contents never logged |
| **System tray only** | No taskbar presence, idles at a fraction of the CPU it used to |
| **Autostart** | Optional Windows startup entry |

---

## Download

Pre-built binaries for Windows x64 are on the [**Releases**](https://github.com/Kushisusumita/screen-translator/releases) page.

No installer — just run `screen-translator.exe`.

---

## Usage

1. The app starts silently in the **system tray** (near the clock).
2. Press **`Ctrl+T`** anywhere on screen.
3. The screen **freezes and dims**. Drag a rectangle around the text — or press
   `Tab` to switch to window or full-screen mode.
4. Release the mouse. The translation appears next to the selection.
5. **Click anywhere** or press **`Esc`** to close it.

Right-click the tray icon for the capture modes, **Параметры** and **Выход**.

### Command line

```
screen-translator.exe --settings          open the settings window on launch
screen-translator.exe --settings engine   open it straight on a given page
screen-translator.exe --help              list the options
```

Pages: `general`, `keys`, `languages`, `engine`, `appearance`, `logs`, `about`.

---

## Translation engines

Engines are tried in the order shown in **Параметры → Движок**; the first one
that answers wins. An engine that is switched on but missing its key is skipped
rather than failing the capture.

| Engine | Key needed | Notes |
|---|---|---|
| **AI** | yes, unless local | Any OpenAI-compatible, Anthropic or Gemini endpoint |
| **DeepL** | yes | Free-tier keys (`…:fx`) are routed to the free host automatically |
| **Yandex** | no | Uses the JSON endpoint directly; an optional headless-browser fallback exists but is off by default |
| **Google** | no | Long text is chunked on sentence boundaries |

### Setting up an AI engine

**Параметры → Движок → AI-переводчик.** Pick a preset (Anthropic, OpenAI,
Gemini, OpenRouter, DeepSeek, Groq, Mistral, Ollama, LM Studio) or fill in the
three fields yourself:

- **Протокол** — which wire format the endpoint speaks
- **Адрес API** — base URL, e.g. `https://api.openai.com/v1`
- **Модель** — model id
- **Токен** — your key; a `localhost` endpoint needs none

**Проверить подключение** sends one short translation and reports what came
back, so a wrong key or model shows up immediately instead of at the next
capture.

The AI engine is told the text came from OCR of a screenshot, so it repairs
recognition damage before translating rather than faithfully translating the
damage.

---

## Privacy

- Captured images are sent to whichever engine you enabled, and nowhere else.
- **Recognised text and translations are never written to the log.** The log
  records lengths and timings. Turning on *Параметры → Журнал → Записывать
  распознанный текст* changes that, and the setting says so.
- Translation history lives in memory and is gone when you quit.
- API keys are sealed with DPAPI, tied to your Windows account.

---

## Requirements

- Windows 10 or 11 (x64)
- Internet connection, unless you point the AI engine at a local model

---

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) 1.75+ stable toolchain
- Visual Studio Build Tools with the **MSVC** compiler and Windows SDK

### Steps

```bash
cargo build --release
```

Binary output: `target/release/screen-translator.exe`

```bash
cargo test
```

---

## Project Structure

The codebase follows **Feature-Sliced Design (FSD)** and **SOLID** principles:

The logo has one source. `shared/mark.rs` holds the geometry of the five-petal
sakura; the settings window draws it with egui, the tray turns it into a Win32
icon at whatever size the shell asks for, and `build.rs` rasterises it into the
nine-size `.ico` embedded in the executable. There is no icon file to keep in
sync, and no path next to the executable that can go missing — which is what
used to leave the tray showing the generic Windows icon under autostart.
`assets/icon.png` is the same mark, exported for documentation only.

```
src/
├── main.rs                   # Entry point: DPI awareness, logging, single instance
├── app/
│   ├── mod.rs                #   Orchestrator (eframe::App impl)
│   └── result.rs             #   The three result presentations
├── entities/                 # Pure data types with no side-effects
│   ├── language.rs           #   Language enum, codes, display names
│   ├── settings.rs           #   Settings tree, hotkeys, engine config
│   └── history.rs            #   In-memory translation history
├── features/                 # Independent vertical slices
│   ├── capture/              #   Screenshot, coordinate system, overlay, window picking
│   ├── hotkey/               #   Global hotkey registration thread
│   ├── settings/             #   Persistence, v0 migration, settings window
│   ├── tray/                 #   Tray icon and menu
│   ├── translation/          #   OCR → repair → translate
│   │   ├── pipeline.rs       #     Public entry point
│   │   ├── ocr.rs            #     Yandex OCR
│   │   ├── cache.rs          #     LRU of recent translations
│   │   └── providers/        #     yandex · google · deepl · ai
│   └── updater/              #   Update check and self-replacement
├── ui/                       # Sakura design system
│   ├── theme.rs              #   Colour tokens, light/dark, fonts
│   ├── platform.rs           #   Windows vs macOS metrics and hotkey spelling
│   ├── icons.rs              #   Vector icons, no glyph dependencies
│   └── widgets.rs            #   Toggles, cards, chips, segmented control
└── shared/                   # Cross-cutting utilities
    ├── mark.rs               #   The sakura logo geometry, shared with build.rs
    ├── logging.rs            #   Daily rotation, retention, redaction
    ├── secret.rs             #   DPAPI-sealed API tokens
    ├── error.rs              #   AppError
    └── utils/                #   Clipboard, autostart, speech
```

**Layer rules:** `app` → `features` → `entities` → `shared`, with `ui` used by
`app` and `features` for presentation only. Features never import from each
other or from `app`.

### Two platform dialects, one surface treatment

The design specifies Fluent and Aqua as two complete rounds, and each platform
gets its own. `ui/platform.rs` holds the differences and `ui/theme.rs` the
palettes; every screen and widget is written once and reads `theme.metrics`.

| | Windows 11 (Fluent) | macOS (Aqua) |
|---|---|---|
| Accent | `#0F6CBD` light · `#4CC2FF` dark on navy | one deepened Aqua blue, white label |
| Navigation | accent bar on the leading edge, search field above | selected row filled with the accent |
| Rows | icon plus a one-line explanation | title only |
| Switch | grey knob when off | white knob |
| Window controls | minimise and close, top right | traffic lights, top left |
| Shortcut | `Alt + Shift + T` | `⌥⇧T` |
| Page names | Общие · Сочетания клавиш · Движок перевода | Основные · Клавиши · Движок |

**Surfaces are frameless on both.** Nothing in the app outlines a container or a
control: a group is one rounded fill a couple of tones off the page, rows inside
it are divided by a hairline at `rgba(255,255,255,.08)`, and a dropdown, text
field or button is a soft fill that reacts to the pointer. The only outlines
left are a hairline on panels that float over the desktop — where there is
nothing else to separate them from the screenshot behind — and the dashed ring
on a shortcut field that is waiting for a keypress, a state no fill conveys.

This departs from both rounds of the mockup, which outline every row in its own
bordered card. It was asked for explicitly.

Two further departures, for legibility:

- the Aqua blue is `#2A72E0` rather than `#2F7CF6` — white on the original is
  3.9:1, under the 4.5:1 a button label needs at this size. A unit test enforces
  the ratio across all four themes, so a future palette edit cannot quietly
  regress it;
- the floating result window says `История · N`, not `Журнал · N`. The mockup
  has no log page to collide with; this app does.

### macOS runtime

The interface layer is cross-platform. The runtime layer is not: screen capture,
global hotkeys, the tray icon, autostart and token sealing are all Win32, behind
`#[cfg(windows)]` with non-Windows stubs that say what is missing. A macOS build
needs CoreGraphics capture, `RegisterEventHotKey`, `NSStatusItem`, a LaunchAgent
plist and Keychain storage. **There is no macOS build today** — the groundwork is
in place, the platform back ends are not.

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
- macOS platform back ends (see above)
- A translation-history window
- Text-to-speech on more than the system voice
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

- The author makes **no guarantees** about accuracy of translations, OCR recognition quality, or service availability.
- The author is **not liable** for any damage, data loss, or consequences arising from use or inability to use this software.
- This project uses **unofficial, undocumented APIs** of third-party services. These may break, change, or become unavailable at any time without notice.
- The author has **no obligation** to fix bugs, add features, respond to issues, maintain compatibility, or continue development of this project in any form.
- Use of third-party translation, OCR and AI services is subject to their respective **Terms of Service**, and you are billed by them directly for any key you configure here. You are responsible for ensuring your use complies with those terms.
- This tool is intended for **personal, non-commercial use** as a productivity aid. The author is not responsible for any misuse.

By using this software you accept these terms.

---

## Author

[@クシススミタ](https://github.com/Kushisusumita)

---

## License

[MIT](LICENSE) © 2026 クシススミタ
