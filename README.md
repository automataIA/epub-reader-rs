# EPUB Reader RS

Desktop-first EPUB reader built with Leptos 0.8 CSR, WebAssembly, Tailwind CSS v4,
daisyUI v5 and a Tauri v2 shell.

## Features

- Local-first EPUB import with drag/drop or file picker.
- Custom Rust/WASM EPUB parser for non-DRM reflowable EPUB files.
- IndexedDB persistence for books, reader location, annotations and preferences.
- Reader workspace with library, TOC, search, notes, typography controls and TTS.
- GitHub Pages build config and Tauri desktop shell.

## Development

Install Rust with the `wasm32-unknown-unknown` target, Trunk, and Node dependencies:

```sh
rustup target add wasm32-unknown-unknown
npm install
```

Run the web app:

```sh
NO_COLOR=false trunk serve --port 1420
```

Build the web app:

```sh
NO_COLOR=false trunk build --release
```

Build for GitHub Pages:

```sh
NO_COLOR=false trunk build --release --config Trunk.ghpages.toml
```

Check the Tauri shell:

```sh
cd src-tauri
cargo check
```

## Scope

The first version supports EPUB 2/3 reflowable books without DRM. Fixed-layout EPUB,
Readium LCP, cloud sync, OPDS catalogs and native file associations are future work.
