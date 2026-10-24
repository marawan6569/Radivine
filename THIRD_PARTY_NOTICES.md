# Third-Party Notices

Radivine is dual-licensed under **`MIT OR Apache-2.0`** ([`LICENSE-MIT`](file:///home/marwanmohamed/Documents/work/rust/radivine/LICENSE-MIT), [`LICENSE-APACHE`](file:///home/marwanmohamed/Documents/work/rust/radivine/LICENSE-APACHE)).

This document lists the third-party fonts, vector icons, and Rust crate dependencies bundled or linked into Radivine and `radivine-viewer`.

---

## 1. Bundled Fonts & Icons

| Asset | Location | Upstream Project | License | Full License Text |
| :--- | :--- | :--- | :--- | :--- |
| **Inter Variable Font** (`Inter-Variable.ttf`) | `assets/fonts/Inter-Variable.ttf` | [Inter by Rasmus Andersson](https://github.com/rsms/inter) | SIL Open Font License 1.1 (`OFL-1.1`) | [`assets/third_party/OFL-Inter.txt`](file:///home/marwanmohamed/Documents/work/rust/radivine/assets/third_party/OFL-Inter.txt) |
| **Lucide Action Icons** (`*.svg`) | `assets/wheel-icons/*.svg` | [Lucide Icons](https://lucide.dev) (derived from Feather Icons) | `ISC` (with `MIT` Feather notice) | [`assets/third_party/ISC-Lucide.txt`](file:///home/marwanmohamed/Documents/work/rust/radivine/assets/third_party/ISC-Lucide.txt) |
| **egui Default Fonts** (`Ubuntu-Light.ttf`, `Hack-Regular.ttf`, `NotoEmoji-Regular.ttf`, `emoji-icon-font.ttf`) | Embedded via `epaint_default_fonts` (`apps/radivine-viewer`) | [egui / epaint](https://github.com/emilk/egui) | `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0` | Upstream `epaint_default_fonts` crate |

First-party Radivine logos and application icons under `assets/branding/`, `assets/icons/`, and `assets/Radivine_ Branch Into Action.png` are original project assets distributed under the repository's `MIT OR Apache-2.0` license.

---

## 2. Direct Rust Dependencies (`Cargo.toml`)

All direct workspace dependencies have been audited via `cargo metadata` for compatibility with `MIT OR Apache-2.0` distribution:

| Crate | Version | SPDX License Expression | Upstream Repository |
| :--- | :--- | :--- | :--- |
| `anyhow` | `1.0.104` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/anyhow> |
| `calloop` | `0.14.5` | `MIT` | <https://github.com/Smithay/calloop> |
| `clap` | `4.6.7` | `MIT OR Apache-2.0` | <https://github.com/clap-rs/clap> |
| `cosmic-text` | `0.12.1` | `MIT OR Apache-2.0` | <https://github.com/pop-os/cosmic-text> |
| `criterion` *(dev)* | `0.5.1` | `Apache-2.0 OR MIT` | <https://github.com/bheisler/criterion.rs> |
| `eframe` | `0.37.0` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui/tree/main/crates/eframe> |
| `egui` | `0.37.0` | `MIT OR Apache-2.0` | <https://github.com/emilk/egui> |
| `encoding_rs` | `0.8.42` | `(Apache-2.0 OR MIT) AND BSD-3-Clause` | <https://github.com/hsivonen/encoding_rs> |
| `fastrand` | `2.5.0` | `Apache-2.0 OR MIT` | <https://github.com/smol-rs/fastrand> |
| `image` | `0.25.10` | `MIT OR Apache-2.0` | <https://github.com/image-rs/image> |
| `jpeg-encoder` | `0.7.1` | `MIT OR Apache-2.0 OR IJG` | <https://github.com/vstroebel/jpeg-encoder> |
| `libwebp-sys` | `0.14.4` (bundles `libwebp 1.6.0`) | `MIT` (bindings) / `BSD-3-Clause` (`libwebp`) | <https://github.com/NoXF/libwebp-sys> / <https://chromium.googlesource.com/webm/libwebp> |
| `memchr` | `2.8.3` | `Unlicense OR MIT` | <https://github.com/BurntSushi/memchr> |
| `mime_guess` | `2.0.5` | `MIT` | <https://github.com/abonander/mime_guess> |
| `percent-encoding` | `2.3.2` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-url/> |
| `proptest` *(dev)* | `1.12.0` | `MIT OR Apache-2.0` | <https://github.com/proptest-rs/proptest> |
| `resvg` | `0.44.0` | `MPL-2.0` | <https://github.com/RazrFalcon/resvg> |
| `rusqlite` | `0.32.1` (bundles `SQLite 3.x` via `libsqlite3-sys`) | `MIT` (bindings) / `Public Domain` (`SQLite`) | <https://github.com/rusqlite/rusqlite> / <https://www.sqlite.org> |
| `rustix` | `1.0.x` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | <https://github.com/bytecodealliance/rustix> |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` | <https://github.com/serde-rs/serde> |
| `serde_json` | `1.0.154` | `MIT OR Apache-2.0` | <https://github.com/serde-rs/json> |
| `sha2` *(dev)* | `0.10.9` | `MIT OR Apache-2.0` | <https://github.com/RustCrypto/hashes> |
| `signal-hook` | `0.3.18` | `Apache-2.0 OR MIT` | <https://github.com/vorner/signal-hook> |
| `smallvec` | `1.16.3` | `MIT OR Apache-2.0` | <https://github.com/servo/rust-smallvec> |
| `softbuffer` *(dev/sandbox)* | `0.4.8` | `MIT OR Apache-2.0` | <https://github.com/rust-windowing/softbuffer> |
| `tempfile` *(dev)* | `3.27.0` | `MIT OR Apache-2.0` | <https://github.com/Stebalien/tempfile> |
| `thiserror` | `2.0.x` | `MIT OR Apache-2.0` | <https://github.com/dtolnay/thiserror> |
| `tiny-skia` | `0.11.4` | `BSD-3-Clause` | <https://github.com/RazrFalcon/tiny-skia> |
| `toml` | `0.8.23` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `toml_edit` | `0.22.27` | `MIT OR Apache-2.0` | <https://github.com/toml-rs/toml> |
| `tracing` | `0.1.44` | `MIT` | <https://github.com/tokio-rs/tracing> |
| `tracing-subscriber` | `0.3.23` | `MIT` | <https://github.com/tokio-rs/tracing> |
| `winit` | `0.30.13` | `Apache-2.0` | <https://github.com/rust-windowing/winit> |
| `x11rb` | `0.13.2` | `MIT OR Apache-2.0` | <https://github.com/psychon/x11rb> |

---

## 3. Transitive License Summary

Across the full Cargo dependency graph (`Cargo.lock`), all transitive crates use permissive or file-level weak-copyleft open-source licenses compatible with binary and source redistribution under `MIT OR Apache-2.0`:

- `MIT`, `Apache-2.0`, `MIT OR Apache-2.0`, `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`
- `BSD-2-Clause`, `BSD-3-Clause` (including bundled Google `libwebp 1.6.0` via `libwebp-sys`)
- `IJG` (`jpeg-encoder`, dual-licensed `MIT OR Apache-2.0 OR IJG`)
- `ISC`, `Zlib`, `0BSD`, `BSL-1.0`, `Unlicense`
- `Unicode-3.0`, `OFL-1.1`, `Ubuntu-font-1.0`
- `MPL-2.0` (`resvg`, `usvg`, `svgtypes`, `simplecss`, `roxmltree`, `kurbo`/`peniko` transitive SVG parsing crates — linked unmodified from crates.io)
