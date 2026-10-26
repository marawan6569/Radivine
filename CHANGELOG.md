# Changelog

All notable changes to **Radivine** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.3.0] - 2026-10-26

### Added
- **Optional Concentric (Nested Rings) Radial Wheel Layout (`radivine-wheel`, `radivine-render`, `apps/radivine`, `apps/radivine-control`):**
  - Added selectable `WheelLayout::Concentric` (`wheel.layout = "concentric"` in `~/.config/radivine/config.toml` or **Control Center → Wheel → Wheel Layout**) alongside the unchanged default `WheelLayout::Branching` (`wheel.layout = "branching"`).
  - Single immobile root center (`C_0`) across all open submenu rings (`R_hub = 50 lp`, root ring `50..118 lp`, outer ring `L >= 1` at `118 + 66·(L-1)..118 + 66·(L-1) + 60 lp`, inter-ring radial gap `6 lp`, minimum `30°` sector sweep with overflow pagination up to 6 rings).
  - Reserved 4-ring + fixed bottom caption box (`Cy + 326..388 lp`) viewport contract requiring a minimum workarea of **`668 × 742` logical pixels** (`1366×768@1x` and `1920×1080@1.25x` activate Concentric; `1280×720@1x` and `1920×1080@1.5x` automatically fall back to `Branching` with `LayoutFallbackReason::InsufficientViewportSpace` without modifying `config.toml`).
  - Strict non-actionable drop & click safety envelope (`T-SAFE1`–`T-SAFE10`): central hub (`r < 50 lp`), inter-ring radial gaps (`6 lp`), angular separator gaps, outer ring margin (`r > R_out(D)`), fixed bottom caption box, displaced-center approach corridor (`> 4 lp` anchor offset), protected pending-switch sibling sectors (`90 ms` outer-ring steering guard), and submenu parent sectors on drag release never arm or execute an action (`XdndStatus(accept=0)`, `XdndFinished(success=0)`), with final drop-time coordinate re-evaluation.
  - Zero warm heap allocations (`0.0 allocs/frame`) during Concentric rendering after cache warmup, path-continuity boundary markers between parent sectors and child rings, breadcrumb caption bar (`Parent › Child › Leaf`), and displaced-origin approach guide.
  - Deferred active-session layout switching (`layout_pending` in `DaemonStatus`) so saving `config.toml` while a wheel session is open never mutates the live session mid-gesture.
- **Portable Arabic Font Fallback in Wheel Renderer & Debian Package Dependencies:**
  - Added Arabic script glyph shaping fallback (`DejaVuSans.ttf` / `DejaVuSans-Bold.ttf`) in `radivine-render::cache::GlyphCache` while keeping embedded `Inter-Variable.ttf` isolated for deterministic Latin rendering.
  - Updated `packaging/debian/control` (`Version: 0.3.0`) to declare `Depends: libc6 (>= 2.34), libgcc-s1 (>= 3.0), fonts-dejavu-core, libfontconfig1` and `Recommends: nemo, libx11-6, libegl1, libgl1, systemd, fonts-noto-core`.

---

## [0.3.0-rc.1] - 2026-10-25 (Public Pre-release — Testing Only)

> **PRE-RELEASE — FOR TESTING ONLY. FINAL DESKTOP ACCEPTANCE PENDING.**

### Added
- **Optional Concentric (Nested Rings) Radial Wheel Layout (`radivine-wheel`, `radivine-render`, `apps/radivine`, `apps/radivine-control`):**
  - Added selectable `WheelLayout::Concentric` (`wheel.layout = "concentric"` in `~/.config/radivine/config.toml` or **Control Center → Wheel → Wheel Layout**) alongside the unchanged default `WheelLayout::Branching` (`wheel.layout = "branching"`).
  - Single immobile root center (`C_0`) across all open submenu rings (`R_hub = 50 lp`, root ring `50..118 lp`, outer ring `L >= 1` at `118 + 66·(L-1)..118 + 66·(L-1) + 60 lp`, inter-ring radial gap `6 lp`, minimum `30°` sector sweep with overflow pagination up to 6 rings).
  - Reserved 4-ring + fixed bottom caption box (`Cy + 326..388 lp`) viewport contract requiring a minimum workarea of **`668 × 742` logical pixels** (`1366×768@1x` and `1920×1080@1.25x` activate Concentric; `1280×720@1x` and `1920×1080@1.5x` automatically fall back to `Branching` with `LayoutFallbackReason::InsufficientViewportSpace` without modifying `config.toml`).
  - Strict non-actionable drop & click safety envelope (`T-SAFE1`–`T-SAFE10`): central hub (`r < 50 lp`), inter-ring radial gaps (`6 lp`), angular separator gaps, outer ring margin (`r > R_out(D)`), fixed bottom caption box, displaced-center approach corridor (`> 4 lp` anchor offset), protected pending-switch sibling sectors (`90 ms` outer-ring steering guard), and submenu parent sectors on drag release never arm or execute an action (`XdndStatus(accept=0)`, `XdndFinished(success=0)`), with final drop-time coordinate re-evaluation.
  - Zero warm heap allocations (`0.0 allocs/frame`) during Concentric rendering after cache warmup, path-continuity boundary markers between parent sectors and child rings, breadcrumb caption bar (`Parent › Child › Leaf`), and displaced-origin approach guide.
  - Deferred active-session layout switching (`layout_pending` in `DaemonStatus`) so saving `config.toml` while a wheel session is open never mutates the live session mid-gesture.
- **Portable Arabic Font Fallback in Wheel Renderer & Debian Package Dependencies:**
  - Added Arabic script glyph shaping fallback (`DejaVuSans.ttf` / `DejaVuSans-Bold.ttf`) in `radivine-render::cache::GlyphCache` while keeping embedded `Inter-Variable.ttf` isolated for deterministic Latin rendering.
  - Updated `packaging/debian/control` (`Version: 0.3.0~rc1`) to declare `Depends: libc6 (>= 2.34), libgcc-s1 (>= 3.0), fonts-dejavu-core, libfontconfig1` and `Recommends: nemo, libx11-6, libegl1, libgl1, systemd, fonts-noto-core`.

---

## [0.2.0] - 2026-10-25

### Added
- **Built-In Image Conversion Subsystem (`radivine-convert`, `radivine-convert-image`, `ffi/radivine-webp`):**
  - Codec-free conversion engine (`radivine-convert`, `#![forbid(unsafe_code)]`) supporting format catalog & magic-byte sniffing (`PNG`, `JPEG`, `WebP`), capability routing (`CapabilityCatalog`), quality intents (`Small`, `Balanced`, `High`, `Lossless`) and explicit per-format codec parameters, 5 built-in presets (`web-optimized`, `balanced`, `high-quality`, `lossless`, `compact-share`), and user-defined presets in `~/.config/radivine/convert.toml`.
  - Built-in image codec backend (`radivine-convert-image`, `#![forbid(unsafe_code)]`) with pure-Rust PNG (`png` `0.18`), JPEG (`zune-jpeg` `0.5` + SIMD `jpeg-encoder` `0.6`), and WebP (`image-webp` `0.2`) plus isolated, memory-fence-wrapped vendored `libwebp` (`1.5.0`, `sse41` + `avx2` + multi-threaded lossy encoding via `ffi/radivine-webp`).
  - Full image pipeline: 8-bit/16-bit/grayscale/CMYK decoding, EXIF orientation auto-application (`1..=8`) with in-place tag-`0x0112` reset, ICC profile extraction and embedding across PNG (`iCCP` zlib)/JPEG (`APP2` `ICC_profile\0`)/WebP (RIFF `VP8X` + `ICCP` muxer), privacy-by-default EXIF stripping (`Strip` default; optional `Preserve` with `gps_privacy_preserved` warning), Lanczos3 aspect-preserving resize (`Width`, `Height`, `Fit`), and configurable alpha background flattening for JPEG outputs.
  - Strict source and destination file-safety guarantees (`ADR-040`, `ADR-041`): sources opened `O_RDONLY | O_NOFOLLOW | O_NONBLOCK` + `fstat` regular-file verification; outputs staged in `0700` `.rvstage-<uid>` directory with `.part` temp files, `fsync`, and atomic `renameat2(RENAME_NOREPLACE)` / `linkat` non-overwriting commit with `stem (1).ext`..`stem (999).ext` collision numbering and cooperative cancellation cleanup.
- **On-Demand Durable Background Job Worker (`radivine-jobs`, `apps/radivine-worker`):**
  - Dedicated `#![forbid(unsafe_code)]` background conversion worker (`radivine-worker`) communicating over `$XDG_RUNTIME_DIR/radivine/worker.sock` (`0600` socket inside `0700` dir) with Linux `SO_PEERCRED` peer-UID authentication and newline-delimited JSON-RPC v1 framing (`MAX_FRAME_BYTES = 1 MiB`).
  - Lossless Unix path serialization via `WirePath` (`utf8:` for valid UTF-8 paths, `hex:` for arbitrary `0x80..=0xFF` bytes) across IPC, SQLite storage, and CLI transport (`ADR-038`).
  - Durable SQLite WAL job store (`~/.local/state/radivine/jobs.sqlite3`, `0600` mode, `schema_version = 1`) with crash-recovery journal (`active_stages`) that reconciles interrupted `Preparing`/`Running`/`Cancelling` jobs and cleans up `.rvstage-<uid>` directories on startup.
  - Bounded item worker pool (`default_concurrency = min(num_cpus / 2, 4)`), token-bucket `ResourceGovernor` memory reservations, cooperative per-item cancellation, linked immutable retry (`FailedOnly`, `FailedAndCancelled`, `AllItems` with optional `output_dir_override`), automatic idle shutdown (`idle_exit_s = 60`), and D-Bus batch summary notifications via extracted `crates/radivine-dbus`.
  - Hardened `/usr/lib/systemd/user/radivine-worker.service` (`NoNewPrivileges=yes`, `PrivateTmp=yes`, `PrivateNetwork=yes`, `ProtectSystem=strict`, `RestrictAddressFamilies=AF_UNIX`).
- **Radial Wheel Integration (`Convert ▸ Image`):**
  - New `Convert ▸ Image` submenu at index 1 in the radial wheel with **Quick Convert** (`PNG`, `JPEG`, `WebP`) and **Advanced…** (`radivine.convert.image.advanced`) actions (`ADR-042`).
  - Dynamic context-aware wheel captions (`Action::description_for`), single-item same-format hiding for Quick Convert, mixed-selection skip summaries, and optimistic `> 64`-item / timeout-safe batch enablement (`FileFacts::is_uninspected`).
  - Lossless RFC 8089 raw-byte percent-encoding in `radivine_core::local_path_to_file_uri` and `OsString` path support in `radivine invoke` so non-UTF-8 filenames work end-to-end.
- **Control Center Conversion Settings, Jobs Monitor & Advanced Convert Dialog (`apps/radivine-control`):**
  - **Conversion Page (`--page conversion`):** Full editor for `~/.config/radivine/convert.toml` and `config.toml [jobs]` (Quick Convert presets per format, output location/suffix/JPEG extension/collision/mtime policies, EXIF/ICC/orientation policies with GPS privacy banner, custom preset CRUD, resource limits, backend capability matrix, notification policy, and worker concurrency/retention).
  - **Jobs Page (`--page jobs`):** Live conversion progress monitor (`JobsClient::subscribe`), state filter pills, 200-item-per-page paginated & `ScrollArea::show_rows`-virtualized item table, `Cancel`, linked `Retry failed` / `Retry failed & cancelled` / `Retry all` / `Retry to folder…`, D-Bus `ShowItems` `Open folder`, `Clear history`, concurrency selector, and `.rvstage-<uid>` inspection/cleanup.
  - **Standalone Advanced Convert Window (`radivine-control convert [--stdin] [PATHS…]`):** Dedicated `640×720` dialog (`app_id = "radivine-convert"`) that bypasses `control.sock` (`ADR-C15`) and launches in an isolated user scope (`ADR-C14`) so multiple Advanced Convert windows can run alongside the Control Center and survive `radivine.service` restarts.
- **Native System Clipboard & Arabic / Non-Latin Font Fallback:**
  - Enabled `eframe`'s `clipboard` feature (`arboard` on X11, `smithay-clipboard` on Wayland) for native cross-application `Ctrl+C`, `Ctrl+V`, `Ctrl+X`, and copy-button integration across `radivine-control`, Advanced Convert, and `radivine-viewer`.
  - Enabled `eframe`'s `system_fonts` feature (`egui_system_font_provider::SystemFontProvider` + `fontique`/Fontconfig) for automatic Arabic, CJK, Hebrew, Cyrillic, and Devanagari glyph fallback and HarfBuzz complex-script shaping (`cosmic-text`) while keeping `Inter-Variable.ttf` as the primary UI font.
- **CI Dependency-Graph Guard & Security Verification:**
  - Added `./scripts/check-dep-graph.sh` to enforce workspace dependency boundaries (`radivine` and `radivine-control` never link `rusqlite`, `libwebp-sys`, or `radivine-worker`; `radivine-worker` never links `radivine-wheel`, `radivine-render`, `x11rb`, or `eframe`).
  - Added 8 SanitizerCoverage `libfuzzer-sys` fuzz targets (`tools/convert-fuzz`), synthetic benchmark/corpus CLI (`tools/convert-cli`), and C5 system benchmark and 1-hour reliability soak test suites.

### Fixed
- Fixed a cooperative-cancellation race in `PendingOutput::create` where a cancellation signal arriving immediately after creating `.rvstage-<uid>` or `.part` returned `Err(ConversionError::Cancelled)` before `PendingOutput::drop` could clean up the temporary staging artifacts.
- Fixed `ReadWritePaths` in `packaging/radivine-worker.service` to prefix optional mount directories (`-/media -/mnt -/run/media`) with `-` so `systemd --user` starts the unit cleanly on distributions (including Linux Mint 22.x) where `/run/media` does not exist at boot.
- Updated `clipboard-win` (`5.4.1 -> 5.4.3`) in `Cargo.lock` to resolve a yanked transitive crate advisory in `cargo deny check`.

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
- Extended `DaemonStatus` IPC telemetry with `version`, `uptime_secs`, `desktop_session`, `desktop_environment`, `display`, `custom_action_count`, `rss_kib`, `graphical_ready`, `compositor_active`, `translucent_overlay`, and `monitor_count`.

### Fixed
- **Login Autostart Rendering & Dynamic Compositor Recovery (`ADR-026`):**
  - Fixed radial wheel visual distortion after Linux Mint Cinnamon login autostart when `radivine.service` starts before Muffin (`cinnamon --replace`) claims `_NET_WM_CM_S0` or before `csd-xsettings`/`csd-xrandr` finish applying DPI and monitor geometry.
  - Subscribed the X11 platform adapter to `XfixesSelectionNotify` on `_NET_WM_CM_S0` and `_XSETTINGS_S0` and re-verified compositor ownership and display settings inside `prepare_overlay()` before every wheel summon, dynamically switching to 32-bit ARGB translucency and resetting `ShapeBounding` without requiring a daemon restart.
  - Hardened `radivine.service` with `After=default.target graphical-session-pre.target dbus.socket`, `StartLimitIntervalSec=0`, and `Environment=RADIVINE_SYSTEMD_SERVICE=1`, plus bounded login X11 connection retry and initial compositor wait.
  - Restricted no-compositor `ShapeBounding` fallback strictly to visible wheel disks and caption boxes (excluding invisible pointer hit-slop corridors) and updated `ShapeBounding` on `HoverChanged`, `SubmenuOpened`, and `SubmenuClosed`.

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
