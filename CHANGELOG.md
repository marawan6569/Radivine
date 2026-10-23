# Changelog

All notable changes to **Radivine** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.1] - 2026-10-23

### Added
- **Radivine Control Center (`apps/radivine-control`):**
  - Standalone `#![forbid(unsafe_code)]` native desktop GUI (`eframe`/`egui` `0.37`) with 6 pages: **Dashboard**, **General**, **Wheel**, **Custom Actions**, **Diagnostics & Logs**, and **About Radivine**.
  - Single-instance window activation lock (`$XDG_RUNTIME_DIR/radivine/control.sock`) that focuses the existing Control Center window (and switches to `--page <NAME>` if requested) instead of opening duplicate windows.
  - Non-blocking background worker thread for `systemctl --user`, daemon IPC queries, X11 doctor probes, and log collection so the UI remains responsive at all times.
- **Terminal-Independent Background Daemon & Autostart Management:**
  - Installed `/usr/lib/systemd/user/radivine.service` (`WantedBy=default.target`, `SuccessExitStatus=0 143`) for reliable login autostart on Linux Mint Cinnamon and Ubuntu X11 sessions.
  - Automatic graphical environment propagation (`systemctl --user import-environment DISPLAY XAUTHORITY WAYLAND_DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP DBUS_SESSION_BUS_ADDRESS`) prior to interactive service start/restart.
  - Detached `process_group(0)` background daemon fallback logging to `~/.local/state/radivine/daemon.log` when `systemd --user` is unavailable.
  - Automatic detection and non-destructive `.bak` conflict resolution for duplicate `systemd` + XDG autostart entries and legacy `graphical-session.target` user unit overrides.
- **Format-Preserving TOML Persistence & Custom Actions Manager:**
  - `toml_edit::DocumentMut` persistence for `~/.config/radivine/config.toml` and `~/.config/radivine/actions.toml` preserving user comments, formatting, and unknown keys.
  - Pre-write strict validation (`AppConfig::validate_strict()`, `EditableAction::validate()`), external modification conflict detection (`mtime` + FNV-1a content digest), `.bak` backups, and same-directory atomic replacement (`.tmp` + `fsync` + `rename`).
  - Graphical Custom Actions Manager supporting `enabled = true|false`, `timeout_ms`, whole-argument placeholder insertion (`{path}`, `{paths}`, `{uri}`, `{uris}`, `{dir}`, `{name}`), and live hot reload into the running daemon without interrupting active drag sessions.
- **Privacy-Conscious Diagnostics & CI Hardening:**
  - Bounded log viewer (`journalctl --user -u radivine.service` or `daemon.log`) with severity and substring filtering plus local diagnostic report export with automatic redaction of personal paths, usernames, UIDs, and `file://` URIs.
  - GitHub Actions CI workflow (`.github/workflows/ci.yml`) running `cargo fmt`, `cargo clippy -D warnings`, headless workspace tests, screenshot export verification, and `.deb` + `lintian` verification.

### Changed
- Updated `/usr/share/applications/radivine.desktop` to launch `radivine-control` instead of `radivine daemon`.
- Extended `DaemonStatus` IPC telemetry with `version`, `uptime_secs`, `desktop_session`, `desktop_environment`, `display`, `custom_action_count`, and `rss_kib`.

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
