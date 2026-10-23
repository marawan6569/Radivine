# Changelog

All notable changes to **Radivine** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0] - 2026-10-23

### Added
- **Pure Radial Wheel Engine (`radivine-wheel`):**
  - Deterministic, time-injected 1-to-3-level branching radial wheel state machine (`#![forbid(unsafe_code)]`, zero steady-state heap allocations).
  - Intent-triangle corridor protection for diagonal cursor travel between parent segments and child wheels.
  - Viewport-aware root and child placement near screen edges and corners with submenu hit-margin overlap guards.
  - Full pointer (dwell/click/drag) and keyboard (`Tab`, Arrow keys, `1`–`9`, `Enter`/`Space`, `Backspace`, `Escape`) navigation.
- **Software RGBA8 Renderer (`radivine-render`):**
  - Premultiplied RGBA8 rendering (`tiny-skia`, `cosmic-text`, `resvg`) with Dark, Light, and Auto desktop themes.
  - Cached radial drop shadows, shaped glyphs (`Inter-Variable.ttf`), and crisp Lucide vector icons.
  - Incremental damage-rectangle tracking and dirty-union session resets (`~212 µs` warm 3-level frame render at 1080p).
- **Native X11 Desktop Integration (`radivine-platform` & `apps/radivine`):**
  - Persistent single-instance background daemon (`radivine daemon`) with Unix domain socket IPC (`invoke`, `status`, `reload`, `quit`, `doctor`).
  - Pre-created `_NET_WM_WINDOW_TYPE_DND` ARGB32 overlay window with damage-clipped POSIX `MIT-SHM` uploads.
  - `XInput2` (`XI_RawKeyPress` / `XI_RawButtonPress`) + `XFixes` (`XdndSelection`) modifier arming for mid-drag <kbd>Shift</kbd> activation in Nemo without stealing the drag source's pointer grab.
  - Complete XDND v5 target state machine (`XdndActionPrivate` only) and `SelectionReader` supporting both single-shot and `INCR` multi-chunk `text/uri-list` transfers.
  - Persistent X11 `CLIPBOARD` selection service supporting `x-special/gnome-copied-files`, `text/uri-list`, `UTF8_STRING`, `MULTIPLE`, and `INCR`.
  - No-compositor fallback mode using opaque backdrop rendering and `XShape` `SK::BOUNDING` masks.
  - Automatic desktop theme (`GTK_THEME` / `gtk-3.0/settings.ini`), `Xft.dpi` scale, and `_XSETTINGS_S0` `Net/EnableAnimations` reduced-motion detection, plus first-run Wayland session diagnostics.
- **Context-Aware Action System (`radivine-core` & `radivine-actions`):**
  - Two-tier context resolution: synchronous zero-I/O `BasicContext` (`parse_uri_list`) followed by non-blocking background `Enricher` (`1,500 ms` timeout watchdog, `O_RDONLY | O_NONBLOCK` + `fstat` + `8 KiB` content/encoding sniffing).
  - Built-in actions: **File Preview** (`Full Preview`, `Head`, `Tail`), **Copy** (`radivine.copy`), **Properties** (`radivine.properties` via `org.freedesktop.FileManager1`), and user-defined shell-free custom actions (`~/.config/radivine/actions.toml`).
- **Standalone Read-Only File Preview Application (`apps/radivine-viewer`):**
  - Separate `eframe`/`egui` desktop application (`#![forbid(unsafe_code)]`) for crash and memory isolation.
  - **Head** (`read_head`) and **Tail** (`read_tail` via reverse `memrchr` seek from EOF) opening `5 GiB` log files in `< 1 ms` with byte-for-byte GNU `head`/`tail` parity.
  - **Full Preview** (`FullDocument`) using chunked `pread` (never `mmap`, immune to `SIGBUS` on concurrent truncation) with background sparse line indexing and virtualized row rendering.
  - Streaming `1 MiB` chunked search with boundary overlap (`Ctrl+F`), Go-to-line navigation, live Dark/Light theme toggle, and `UTF-8` / `UTF-8 BOM` / `UTF-16 LE` / `UTF-16 BE` / lossy 8-bit decoding.
- **Packaging, Licensing & Verification:**
  - Dual licensed under **`MIT OR Apache-2.0`** ([`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)).
  - Reproducible Debian package builder (`scripts/build-deb.sh` -> `radivine_0.1.0_amd64.deb` + `SHA256SUMS.txt`, `lintian` clean) with Nemo context-menu action, desktop entry, man pages, and opt-in autostart examples.
