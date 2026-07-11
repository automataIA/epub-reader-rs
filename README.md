<p align="center">
  <img src="public/epub-logo.svg" alt="EPUB Reader RS logo" width="128" />
</p>

<h1 align="center">EPUB Reader RS</h1>

<p align="center">
  A local-first EPUB reader written in Rust. It runs as a WebAssembly web app on
  GitHub Pages and as a native desktop application (Windows, macOS, Linux) built with
  Tauri, featuring GPU-accelerated read-aloud. Books, reading position, annotations
  and preferences never leave the device.
</p>

<p align="center">
  <a href="https://github.com/automataIA/epub-reader-rs/actions/workflows/ci.yml"><img src="https://github.com/automataIA/epub-reader-rs/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/automataIA/epub-reader-rs/actions/workflows/deploy.yml"><img src="https://github.com/automataIA/epub-reader-rs/actions/workflows/deploy.yml/badge.svg" alt="Deploy to GitHub Pages" /></a>
  <a href="https://github.com/automataIA/epub-reader-rs/actions/workflows/release.yml"><img src="https://github.com/automataIA/epub-reader-rs/actions/workflows/release.yml/badge.svg" alt="Release (desktop)" /></a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white" alt="Rust" />
  <img src="https://img.shields.io/badge/WebAssembly-654FF0?logo=webassembly&logoColor=white" alt="WebAssembly" />
  <img src="https://img.shields.io/badge/Leptos-EF3939?logo=leptos&logoColor=white" alt="Leptos" />
  <img src="https://img.shields.io/badge/Tauri-24C8DB?logo=tauri&logoColor=white" alt="Tauri" />
  <img src="https://img.shields.io/badge/Tailwind_CSS-06B6D4?logo=tailwindcss&logoColor=white" alt="Tailwind CSS" />
  <img src="https://img.shields.io/badge/daisyUI-1AD1A5?logo=daisyui&logoColor=white" alt="daisyUI" />
  <img src="https://img.shields.io/badge/ONNX_Runtime-005CED?logo=onnx&logoColor=white" alt="ONNX Runtime" />
</p>

<p align="center">
  <a href="https://automataia.github.io/epub-reader-rs/"><b>Live web app</b></a>
  &nbsp;&middot;&nbsp;
  <a href="https://github.com/automataIA/epub-reader-rs/releases/latest"><b>Download desktop builds</b></a>
</p>

## Contents

- [Download](#download)
- [Features](#features)
- [Technology](#technology)
- [Getting started](#getting-started)
- [Building](#building)
- [Text-to-speech](#text-to-speech)
- [Testing and linting](#testing-and-linting)
- [Project layout](#project-layout)
- [Scope and roadmap](#scope-and-roadmap)
- [Contributing](#contributing)
- [License](#license)

## Download

Signed-in visitors of a release find the installers under **Assets**. The latest
desktop builds are published here:

**https://github.com/automataIA/epub-reader-rs/releases/latest**

| Platform | Installer(s) | File |
| --- | --- | --- |
| Windows | NSIS setup, MSI | `EPUB.Reader.RS_<version>_x64-setup.exe`, `..._x64_en-US.msi` |
| macOS (Apple Silicon) | Disk image | `EPUB.Reader.RS_<version>_aarch64.dmg` |
| Linux | AppImage, Debian, RPM | `..._amd64.AppImage`, `..._amd64.deb`, `...x86_64.rpm` |

Notes:

- macOS builds target Apple Silicon (arm64) only. Intel Macs are not shipped as a
  prebuilt binary; build from source instead.
- No installation is required to try the reader — the web app above runs entirely in
  the browser.

## Features

- Local-first EPUB import via drag-and-drop or file picker; nothing is uploaded.
- Custom Rust/WebAssembly parser for non-DRM reflowable EPUB 2 and EPUB 3 files.
- Persistent library, reading position, annotations and preferences stored in
  IndexedDB, so state survives reloads and restarts.
- Reader workspace: library, table of contents, full-text search across the spine,
  notes, and typography controls (font, size, spacing, theme).
- Read-aloud (text-to-speech) with GPU acceleration on desktop and a WebAssembly
  engine in the browser. See [Text-to-speech](#text-to-speech).
- Single custom dark theme tuned for long reading sessions.

## Technology

| Layer | Choice |
| --- | --- |
| UI | Leptos 0.8 (client-side rendering) compiled to WebAssembly |
| Styling | Tailwind CSS v4 (CSS-first config) and daisyUI v5 |
| Bundler | Trunk |
| Storage | IndexedDB via `rexie` |
| EPUB parsing | Custom parser over `zip` and `quick-xml` (no third-party EPUB crate) |
| Desktop shell | Tauri v2 |
| Read-aloud | Piper (VITS) via ONNX Runtime; GPU execution provider selected per OS |

## Getting started

Prerequisites:

- Rust with the `wasm32-unknown-unknown` target
- [Trunk](https://trunkrs.dev/)
- Node.js (for the Tailwind and TTS build steps)

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk
npm install
```

Run the web app locally on http://127.0.0.1:1420:

```sh
NO_COLOR=false trunk serve --port 1420
```

## Building

Web app (release):

```sh
NO_COLOR=false trunk build --release
```

GitHub Pages build (enables the `gh-pages` feature and the `/epub-reader-rs/` base
path):

```sh
NO_COLOR=false trunk build --release --config Trunk.ghpages.toml
```

Desktop application (Tauri):

```sh
cd src-tauri
cargo tauri build --features gpu-tts
```

The `gpu-tts` feature enables the native read-aloud engine and selects the GPU
execution provider at compile time: DirectML on Windows, CoreML on macOS, and CUDA
on Linux, each with a CPU fallback at runtime.

Continuous delivery is automated:

- `ci.yml` runs Clippy and unit tests on every push and pull request.
- `deploy.yml` publishes the web app to GitHub Pages on pushes to `main`.
- `release.yml` builds the desktop installers for all platforms and attaches them to
  a GitHub Release when a `v*` tag is pushed.

## Text-to-speech

Read-aloud uses the [Piper](https://github.com/rhasspy/piper) VITS voices running on
ONNX Runtime — no cloud service and no data leaves the device.

- **Web:** the engine runs entirely in WebAssembly. Voice models are fetched on first
  use and cached in the browser.
- **Desktop:** synthesis runs natively on the GPU through ONNX Runtime, using the
  platform execution provider (DirectML / CoreML / CUDA) with a CPU fallback.

## Testing and linting

Unit tests (including the EPUB parser) run natively — no WebAssembly runner needed:

```sh
cargo test
```

Lint with Clippy (warnings treated as errors):

```sh
cargo clippy -- -D warnings
```

## Project layout

```
src/            WebAssembly application crate (Leptos UI, EPUB parser, storage)
src-tauri/      Tauri desktop shell and native TTS backend (separate crate)
public/         Static assets, generated CSS, and the Piper TTS bundle
style/          Tailwind CSS v4 input and theme tokens
scripts/        Build helpers (TTS bundle, GitHub Pages base path)
```

## Scope and roadmap

The current version supports reflowable EPUB 2 and EPUB 3 books without DRM.

Not yet supported, and considered future work:

- Fixed-layout EPUB
- Readium LCP and other DRM schemes
- Cloud sync across devices
- OPDS catalog browsing
- Native file associations and Intel macOS binaries

## Contributing

Issues and pull requests are welcome. Before opening a pull request, please run the
tests and Clippy locally and keep changes focused. The desktop crate under
`src-tauri/` is independent from the web crate under `src/`.

## License

No license has been declared for this repository yet. Until a `LICENSE` file is added,
all rights are reserved by the author. If you intend to reuse the code, please open an
issue to clarify licensing.
