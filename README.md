<p align="center">
  <img src="assets/logo.png" alt="PercyStar Logo" width="140" />
</p>

<h1 align="center">PercyStar</h1>

<p align="center">
  <strong>Zero-dependency native Rust RPF9 (<code>9FPR</code>) explorer, RAGE 9 3D meshlet viewer, pure-Rust <code>ADAT</code> Opus audio decoder, and docked D3D11 Bink2 player</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.85%2B-orange?style=flat-square&logo=rust" alt="Rust" />
  <img src="https://img.shields.io/badge/Audio-Pure%20Rust%20Opus%20(Zero%20FFmpeg)-90EBA5?style=flat-square" alt="Audio" />
  <img src="https://img.shields.io/badge/3D-RAGE9%20Meshlets%20%2B%20UVs-FF55AF?style=flat-square" alt="3D" />
  <img src="https://img.shields.io/badge/Platform-Windows%20x64-0078D6?style=flat-square" alt="Windows" />
  <img src="https://img.shields.io/badge/Platform-macOS%20(Universal)-111111?style=flat-square&logo=apple" alt="macOS" />
  <img src="https://img.shields.io/badge/License-Source--Available%20(No%20Modifications)-red?style=flat-square" alt="License" />
</p>

<p align="center">
  <strong>On Mac? <a href="https://github.com/rpatty-dev/PercyStar/releases/download/v1.0.3/PercyStar_macOS.zip">Download PercyStar for macOS here</a></strong><br />
  <sub>Universal app (Apple Silicon + Intel), macOS 11+. Unzip, then right-click <code>PercyStar.app</code> &gt; Open the first time (the app is not notarized).</sub>
</p>

<p align="center">
  <img src="assets/demo_overview.webp" alt="PercyStar 30fps Showcase — Neon Sunset UI, Search, Vehicles Filter & In-App Updater" width="920" />
</p>

---

## Promo

<p align="center">
  <a href="assets/promo.mp4"><img src="assets/promo-poster.jpg" alt="PercyStar promo video: pulling a car out of an RPF9 archive into an .OBJ" width="860" /></a><br />
  <sub><a href="assets/promo.mp4">Watch the 31-second promo (MP4)</a></sub>
</p>

---

## Visual Showcase

### RAGE 9 3D Meshlet Viewer (`Textured/Mat` · `Clay Solid` · `Solid+Wire` · `Wire`)
<p align="center">
  <img src="assets/demo_3d.webp" alt="PercyStar 3D Meshlet Viewer — Shading Modes & Smooth Orbit" width="900" />
</p>

### Pure-Rust Audio (`ADAT` Opus → WAV), `PSIN` Metadata & XML Inspector
<p align="center">
  <img src="assets/demo_inspector.webp" alt="PercyStar Pure-Rust Opus Audio, PSIN PSO Inspector & XML Viewer" width="900" />
</p>

<details>
<summary>Static High-Res Screenshot (Neon Sunset v1.0.3)</summary>
<p align="center">
  <img src="assets/preview.png" alt="PercyStar 3D Meshlet Viewer — Neon Sunset Theme" width="900" />
</p>
</details>

---

## Features

- **100% Native Runtime Dependencies for Audio, 3D, XML, and PSO**
  - **No Python, no FFmpeg, no external DLLs required** — `ADAT` (`.awc`) Opus streams are decoded natively inside `PercyStar.exe` using pure-Rust `rusty-opus` (RFC 6716), and `.#dr`/`.#ft`/`.#dd` 3D models are decoded and rasterized natively in Rust.
  - All Rust crate dependencies are pinned in [`Cargo.lock`](Cargo.lock).
- **Native RAGE 9 3D Model Viewer (`.#dr`, `.#ft`, `.#dd`, `.obj`)**
  - Direct click-to-view decoding of unencrypted RAGE 9 virtual (`0x50xxxxxx`) + physical (`0x60xxxxxx`) meshlet resources straight out of `.rpf` archives.
  - Clean **LOD0-only** extraction (`Lod0 -> grmModel -> grmGeometry`), 24/24/16-bit quantized vertices, 7-bit packed meshlet triangles + `u16` remap table, and **UV0** (`type 0x28`).
  - Zero-alloc **Z-buffered software rasterizer** (`Textured/Mat`, `Clay Solid`, `Solid+Wire`, `Wire`, `Spin`), embedded `BC1`/`BC3` TXD decoding, custom `.png`/`.bmp` texture loading, and full `.OBJ` export (`v` + `vt` + `f v/vt`).
  - Dedicated **`Vehicles Only`** toggle in the 3D Models tab + sequential fast-indexing of vehicle archives.
- **Whole-Folder & Single-RPF Scanning**
  - Indexes across worker threads, streams results live, and caches to `%LOCALAPPDATA%\PercyStar\scan_cache_v4.tsv`.
- **Docked D3D11 Bink2 Video Player (Optional `binkplay.exe`)**
  - Docks RAD `binkplay.exe` (`/NoBorder /DrawD3D11 /L`) directly into the inspector pane. Run [`scripts/setup_deps.ps1`](scripts/setup_deps.ps1) if you don't already have RAD Video Tools installed.
- **CLI Batch Model Export**
  ```powershell
  PercyStar.exe --extract-vehicles "<path-to-levels.rpf>" "<output-dir>"
  PercyStar.exe --extract-models "<path-to-archive.rpf>" "<output-dir>" "filter1,filter2"
  ```

---

## Project Layout

```text
PercyStar/
├── assets/
│   ├── logo.png            # App & window icon
│   ├── logo.ico            # Windows executable icon
│   ├── demo_overview.webp  # 30fps Neon Sunset UI, search & updater showcase
│   ├── demo_3d.webp        # 30fps 3D meshlet viewer shading modes showcase
│   ├── demo_inspector.webp # 30fps XML / PSIN / ADAT Opus audio showcase
│   ├── preview.png         # High-res Neon Sunset screenshot
│   ├── promo.mp4           # 31s promo video
│   └── promo-poster.jpg    # Promo poster frame
├── promo/                  # HyperFrames source for the promo video
├── scripts/
│   ├── build_macos.sh      # Universal macOS (.app / .zip) build helper
│   ├── setup_deps.ps1      # Optional RAD Video Tools (binkplay.exe) setup
│   ├── awc_extract.py      # Standalone CLI ADAT (.awc) Opus -> .wav helper
│   └── rpf9_carve.py       # Standalone CLI RPF9 carver
├── src/
│   ├── main.rs             # PercyStar egui application & RPF9 scanner
│   ├── model3d.rs          # RAGE 9 meshlet decoder, UV/TXD parser & Z-buffer renderer
│   └── awc.rs              # Pure-Rust ADAT (.awc) Opus -> .wav decoder
├── app.rc / build.rs       # Win32 icon resource
├── Cargo.toml / Cargo.lock # All Rust crate dependencies pinned cleanly
└── LICENSE
```

---

## Building from Source

Requires **Rust** (`x86_64-pc-windows-gnu` with MinGW `windres`/`dlltool`, or `msvc`). Cargo fetches all pinned dependencies from `Cargo.lock`:

```powershell
cargo test --release
cargo build --release
```

Compiled standalone binary: `target/release/PercyStar.exe`.

---

## Scope, Clean-Room Research & Legal Notice

PercyStar is an **independent, clean-room binary inspection and parser tool** built from scratch in Rust. It only parses **unencrypted, user-supplied files** (`KB2*` streams, `ADAT` audio, plain `zlib` XML/PSIN containers, and standard uncompressed meshlet vertex/index blocks) for research and interoperability (cf. 17 U.S.C. § 1201(f)):

- **Zero Circumvention / Zero Keys:** Contains **no** decryption routines, encryption keys, hashes, DRM bypasses, or access-control workarounds. Encrypted data is skipped untouched.
- **Zero Proprietary Content:** Ships **no** third-party binaries, copyrighted models, textures, audio, shaders, or game assets. Users are solely responsible for supplying archives they legally own or are authorized to inspect.
- **Independent Trademark Disclaimer:** PercyStar is an independent open-source project and is not affiliated with, endorsed by, or sponsored by any game studio or publisher.

**Copyright (c) 2026 rpatty-dev — All Rights Reserved.**  
See [LICENSE](LICENSE). You may download, run, and fork this repository on GitHub for personal use/reference, **but you may not modify, alter, or redistribute modified versions of the code or binaries without explicit written permission.**
