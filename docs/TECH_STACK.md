# Radivine — Technology Stack Evaluation

Status: **Recommended stack (pending M0 spikes)** · Last updated: 2026-10-22
Crate versions below were read from crates.io on 2026-10-22 and must be re-checked when `Cargo.toml` is first written.

---

## 1. Requirements that drive the choice

From the brief, ranked by how strongly they constrain technology:

1. **The overlay must be a valid XDND drop target during another application's drag** and receive positions/drops — *drawing a transparent window is not enough* (see [FEASIBILITY_STUDY.md](FEASIBILITY_STUDY.md)). We need: override-redirect, ARGB visual, `XdndAware`, direct handling of `XdndEnter/Position/Leave/Drop`, custom `XdndStatus` replies (per-position accept/reject, `XdndActionPrivate`), `XFixes` selection tracking, XI2 raw events, SHAPE input regions, RandR monitors.
2. Transparent, borderless, always-on-top surface positioned at arbitrary screen coordinates.
3. Low-latency, smooth animation of a small number of vector shapes + text + icons.
4. Low idle CPU/memory for a background daemon.
5. Future Wayland/Windows/macOS portability of the **wheel engine and renderer** (platform code may be rewritten per OS).
6. Implementation simplicity for an AI implementer.

## 2. Candidates evaluated

Legend: ✅ good · ⚠️ partial/risky · ❌ unsuitable.

### 2.1 Windowing / platform layer for the overlay

| Option | Version (2026-10) | Override-redirect | ARGB transparency | Custom XDND target (status/action/positions) | XFixes / XI2 raw / SHAPE input | Verdict |
|--------|------------------|-------------------|-------------------|---------------------------------------------|-------------------------------|---------|
| **x11rb** (pure-Rust X11 protocol bindings, `RustConnection`) | 0.14.0 | ✅ | ✅ (choose 32-bit visual) | ✅ (we implement the protocol) | ✅ all extensions as features | ✅ **Selected for X11 backend** |
| winit 0.30 | 0.30.13 | ✅ (`with_override_redirect`) | ✅ | ❌ old `HoveredFile/DroppedFile` API: no positions during drag, no per-position accept, no action control | ❌ not exposed | ❌ |
| winit 0.31 (beta) | 0.31.0-beta.3 | ✅ | ✅ | ⚠️ new `DragEntered/DragPosition/DragDropped` + `set_valid_dnd_actions` API — promising but **unreleased** and X11 action info only on completion | ❌ | ⚠️ revisit for Windows/macOS later |
| GTK4 (gtk4-rs) | 0.11.5 | ❌ GTK4 removed toplevel positioning/override-redirect APIs | ✅ | ✅ `GtkDropTarget` | ❌ | ❌ cannot place a popup at arbitrary screen coordinates |
| GTK3 (gtk-rs) | unmaintained bindings | ✅ | ✅ | ✅ | ❌ | ❌ bindings deprecated |
| iced / Slint / egui (via winit) | 0.14 / 1.18.1 / 0.37 | inherit winit | inherit | ❌ | ❌ | ❌ for the overlay (fine for normal windows) |
| Xlib/XCB via C FFI | — | ✅ | ✅ | ✅ | ✅ | ⚠️ unsafe FFI and C deps for no benefit over x11rb |

**Conclusion:** the overlay needs protocol-level control that no GUI toolkit exposes. Use **x11rb** directly for the X11 backend (ADR-004). x11rb's `RustConnection` supports FD passing (needed for MIT-SHM `AttachFd`) and has no C dependencies.

Required x11rb features: `shm`, `shape`, `xfixes`, `xinput`, `randr`, `res` (X-Resource → source PID), `resource_manager` (Xft.dpi), `cursor` (theme cursor for Invoke-mode grabs).

### 2.2 Rendering

| Option | Version | Transparency | Cost/complexity | Portability of output | Verdict |
|--------|---------|--------------|-----------------|-----------------------|---------|
| **tiny-skia** (CPU, Skia-subset rasteriser; premultiplied RGBA8) | 0.12.0 | ✅ any target that accepts premultiplied pixels | tiny dependency, deterministic, no GPU init; AA paths, gradients, strokes | ✅ the same buffer can be presented on X11 (SHM), Wayland (`wl_shm` ARGB8888), Windows (`UpdateLayeredWindow`), macOS (`CGImage` → `CALayer`) | ✅ **Selected** |
| wgpu (+ lyon/vello) | 30.0.1 / vello 0.11.0 | ⚠️ surface alpha modes vary by driver/platform; X11 + proprietary NVIDIA historically limited for premultiplied alpha (reported in community issues; not verified here) | GPU device/driver initialisation latency and driver memory (expected to be significant for a tiny overlay — not measured); shader pipeline | ⚠️ | ❌ overkill for ≤ 50 shapes/frame; consider only if CPU budget is missed |
| OpenGL (glow/glutin) | — | ⚠️ ARGB GLX visual fiddly | moderate | ⚠️ | ❌ |
| cairo (C) | — | ✅ | C dependency | ✅ | ⚠️ unnecessary C dep |
| softbuffer (present CPU buffers via winit) | 0.4.8 | ❌ no reliable per-pixel alpha on X11/Wayland | — | — | ❌ for the overlay; ✅ for the **opaque** dev sandbox |

Back-of-envelope budget (to be benchmarked, not a claim): a 3-level cluster is ~30 filled arcs, ~30 icons, ~30 label blits in a ~900×700 px region. tiny-skia rasterises such scenes in low single-digit milliseconds on a desktop CPU; uploading the damaged region through MIT-SHM is sub-millisecond. This fits a 60 Hz (16.7 ms) budget with large margin. **Benchmark in M1b** (`criterion`) and S-5.

Text: **cosmic-text** 0.19.0 (shaping via HarfBuzz-compatible pure-Rust shaper, BiDi, font fallback, swash rasterisation). Labels are static during a session → rasterise once per session into cached alpha masks, then per frame only blit. Alternatives: `fontdue` 0.9.5 / `ab_glyph` 0.2.32 are lighter but lack shaping/fallback (non-Latin filenames and future i18n) → rejected for labels.

Icons: **resvg / usvg** 0.48.1 (renders SVG directly onto tiny-skia pixmaps, same ecosystem). Icons are rasterised once per size and cached.

Fonts & icons shipped in-repo for deterministic rendering (ADR-019):
- **Inter** (SIL OFL 1.1) as the default UI font; system fonts loaded lazily only for fallback glyphs.
- **Lucide** icon subset (ISC licence) for wheel icons (`eye`, `file-text`, `arrow-up-to-line`, `arrow-down-to-line`, `copy`, `info`, `sparkles`, `terminal`, `x`, `chevron-left`).
- Licence files committed under `assets/third_party/`.

### 2.3 Event loop and concurrency

| Option | Verdict | Reason |
|--------|---------|--------|
| **calloop** 0.14.5 | ✅ **Selected** | Small, callback-based, fd sources + timers + channels; the native loop of smithay-client-toolkit → same loop for a future Wayland backend |
| tokio / async-std | ❌ | An async runtime adds threads and complexity for an fd + timers + a few channels |
| Hand-rolled `poll(2)` loop | ⚠️ | Possible, but calloop already provides timers/channels correctly |

Concurrency model: **one UI thread** (X11 I/O, wheel engine, rendering, presentation) + **short-lived worker threads** (context enrichment, action execution) returning results through `calloop::channel`. No locks on the hot path.

### 2.4 Normal application windows (Full Preview viewer, future settings)

| Option | Version | Verdict |
|--------|---------|---------|
| **eframe/egui** | 0.37.0 | ✅ **Selected** for `radivine-viewer` and `radivine-control` (`features = ["clipboard", "default_fonts", "glow", "x11", "wayland"]`): immediate mode, `ScrollArea::show_rows` virtualised rendering of huge line counts, native OS clipboard interoperability (`egui-winit/clipboard` via `arboard` on X11 and `smithay-clipboard` on Wayland), cross-platform, mature |
| iced | 0.14.0 | ⚠️ capable; virtualised text of multi-GB files needs more custom work |
| Slint | 1.18.1 | ⚠️ capable; DSL + licensing (GPL/royalty-free/commercial) considerations |
| GTK4 | 0.11.5 | ⚠️ most native look on Cinnamon; heavier dependency; not portable to Windows/macOS cleanly |

The viewer runs as a **separate process**, so its dependencies never inflate the daemon's memory (ADR-014).

### 2.5 Supporting crates

| Concern | Crate (version 2026-10) | Notes |
|---------|------------------------|-------|
| Errors | `thiserror` 2.0 (libs), `anyhow` 1.0 (apps only) | |
| Logging | `tracing` 0.1, `tracing-subscriber` 0.3 | `RADIVINE_LOG=debug`; no telemetry |
| Config | `serde` 1, `toml` 1.x | `~/.config/radivine/*.toml` |
| IPC | `serde_json` over a Unix socket | JSON lines; single-instance lock |
| CLI | `clap` 4 (derive) | `radivine daemon/invoke/status/reload/dismiss/quit/doctor` |
| Paths | `xdg` 3.0 (Linux), later `directories` 6.0 for cross-platform | |
| Small vectors | `smallvec` | Avoid per-frame allocations |
| URIs | `percent-encoding` (or `url` 2.x) | RFC 2483 parsing |
| MIME (no I/O) | `mime_guess` 2.0.5 | Extension-based, for the Basic context |
| MIME (content) | `infer` 0.22.0 + NUL/UTF-8/BOM heuristics | Pure Rust magic numbers; no system DB dependency |
| Large files | `memchr` (+ chunked `pread`) | Viewer line index & search; **no `memmap2`** — a file truncated while mapped raises `SIGBUS` |
| Encoding | `encoding_rs` (+ optionally `chardetng`) | Viewer: UTF-8/UTF-16 BOM; legacy encodings later |
| Syscalls | `rustix` 1.1 | `memfd_create`, `mmap` for SHM; signals via calloop |
| D-Bus | `zbus` 6.0 (blocking API in worker threads) | `org.freedesktop.FileManager1.ShowItemProperties`, notifications. Note: zbus's blocking API drives an internal executor thread; this is accepted and confined to the notification/action workers — the daemon itself still has no async runtime |
| Icon theme lookup (M4, `theme:` icons for custom actions) | `freedesktop-icons` (or a small own lookup per the XDG Icon Theme spec) + tiny-skia `png-format` feature for PNG icons | Verify crate maintenance at M4; fallback = own lookup |
| PNG I/O for snapshot tests | tiny-skia `png-format` feature (`Pixmap::encode_png/decode_png`) + small custom pixel-diff with tolerance | No extra image crate |
| Testing | `proptest` 1.12, `insta` 1.49, `criterion` 0.8 | See [TESTING_STRATEGY.md](TESTING_STRATEGY.md) |
| Dev sandbox only | `winit` 0.30.13 + `softbuffer` 0.4.8 | Opaque window; not used by the daemon |

Required crate features: `x11rb` (see §2.1), `calloop` `signals`, `rustix` `fs` + `mm`, `serde` `derive`, `tiny-skia` `png-format`, `clap` `derive`.

Development/CI tools (not dependencies): `cargo-deny`, `cargo-llvm-cov`, `cargo-msrv`, `cargo-fuzz` (+ `libfuzzer-sys` in `fuzz/`), `cargo-deb`, optional `clap_mangen` (man page).

Deliberately **not** used: `keyframe` (stale; our animation needs are ~100 lines), `content_inspector` (unmaintained since 2018), `tree_magic_mini` (depends on system shared-mime-info DB; may be added later for parity with Nemo's MIME naming), any GPU stack in the daemon, any async runtime in the daemon.

### 2.6 Media Conversion Core & Built-in Image Backend (v0.2.0 — Milestone C1)

| Concern | Crate (version 2026-10) | Notes |
|---------|------------------------|-------|
| Safe Image Decode & PNG/Lossless-WebP Encode | `image` 0.25.10 (`default-features = false, features = ["png", "jpeg", "webp"]`) | Pure-Rust decoding (`png 0.18.1`, `zune-jpeg 0.5.15`, `image-webp 0.2.4`) under `#![forbid(unsafe_code)]` with strict `image::Limits` (`ADR-030`) |
| JPEG Encode (subsampling, progressive, ICC/EXIF) | `jpeg-encoder` 0.7.1 (`default-features = false, features = ["std"]`) | `#![forbid(unsafe_code)]`, zero transitive dependencies, 2.6× faster and 36% smaller output at `q=82` than `image`'s basic JPEG encoder; supports `4:4:4`/`4:2:2`/`4:2:0`, progressive DCT, optimized Huffman coding, ICC, and EXIF |
| Lossy WebP Encode (bundled C `libwebp`) | `libwebp-sys` 0.14.4 (bundles `libwebp 1.6.0`) wrapped by internal `ffi/radivine-webp` | Audited ~300-LoC encode-only wrapper (`ADR-030`, `ADR-031`, `D2`). Never decodes untrusted bytes; uses `picture.use_argb = 0` direct YUV420 import and `progress_hook` mid-encode cancellation |
| Comment-Preserving TOML Editing | `toml_edit` 0.22.27 | Used in `radivine-convert` and `radivine-control` to save `convert.toml` without stripping user comments |
| Cryptographic Digest (Tests & Verification) | `sha2` 0.10.9 | Byte-for-byte ICC profile and source-immutability verification |
| CSPRNG / Staging Suffices | `fastrand` 2.5.0 | Collision-free `.rvstage-<job>-<rand16>` and `item-<idx>-<rand16>.part` names |
| Durable Job Store (Milestone C2) | `rusqlite` 0.32 (`features = ["bundled"]`) | SQLite WAL persistence and `C0a`–`C6` crash-recovery journal in `apps/radivine-worker` (`ADR-032`) |

## 3. Final recommended stack

| Layer | Choice |
|-------|--------|
| Language / edition | Rust, edition 2024; MSRV determined with `cargo msrv` at M1 and recorded in DECISIONS.md |
| X11 backend | `x11rb` 0.14 (pure Rust), MIT-SHM via `memfd` + `AttachFd`, SHAPE, XFixes, XInput 2.2, RandR, X-Resource |
| Event loop | `calloop` 0.14 |
| Wheel engine | Pure Rust, std + `smallvec` only |
| Renderer | `tiny-skia` 0.12 + `cosmic-text` 0.19 + `resvg` 0.48; bundled Inter + Lucide |
| Viewer | `eframe`/`egui` 0.37, separate process |
| Config / IPC | TOML / JSON-lines over Unix socket |
| Logging | `tracing` |
| D-Bus | `zbus` 6 (worker threads only) |
| CI | GitHub Actions: fmt, clippy, test, deny, e2e under Xvfb |

## 4. Trade-offs accepted

- **We own the X11 protocol code** (~1.5–2.5 k lines estimated): more code than using a toolkit, but it is the only way to get the required XDND behaviour, and it is isolated in `radivine-platform::x11`.
- **CPU rendering** caps visual effects (no real-time blur/backdrop filter). The design uses solid translucent fills, subtle gradients and simple drop shadows drawn into the pixmap, which suit the minimal brand style.
- **Separate viewer process**: some start-up latency for the first preview window (estimate: a few hundred ms, to be measured in M5); in exchange the daemon stays small and crash-isolated.
- **Bundled font/icons**: roughly 1–2 MB in the binary/package (estimate; depends on how many Inter weights are bundled); gives deterministic visuals and snapshot tests.

## 5. Re-evaluation triggers

Re-open this decision (and record an ADR) if:
- Frame time p95 > 8 ms for a 3-level cluster at 2× scale on the reference machine (→ consider wgpu renderer behind the same `Scene` interface).
- winit ≥ 0.31 is released with stable DnD + override-redirect/layered-window support (→ consider for Windows/macOS backends).
- A Wayland backend is started (→ smithay-client-toolkit 0.21+, same calloop loop, same renderer output).
