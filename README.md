<div align="center">
  <img src="assets/Radivine_%20Branch%20Into%20Action.png" alt="Radivine — Branch into action." width="760" />

  <h1>Radivine</h1>
  <p><strong>Branch into action.</strong></p>
  <p>A fast, extensible, context-aware radial action wheel for your Linux desktop.</p>

  <p>
    <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-teal.svg" alt="License: MIT OR Apache-2.0" /></a>
    <img src="https://img.shields.io/badge/version-v0.1.0-blue.svg" alt="Version: v0.1.0" />
    <img src="https://img.shields.io/badge/platform-Linux%20X11%20(amd64)-orange.svg" alt="Platform: Linux X11 amd64" />
  </p>
</div>

---

## Overview

**Radivine** brings fluid, hierarchical radial interaction to Linux desktops. Instead of navigating deep linear context menus or switching windows during file operations, summon a branching radial wheel directly under your cursor:

- **Mid-Drag Activation (`Drag + Shift`):** Begin dragging one or more files in **Nemo** (or any XDND v5 source on X11) and press <kbd>Shift</kbd>. Radivine intercepts the drag in place (`< 27 ms`), displays context-aware actions around your cursor, and commits the chosen action on mouse release without moving or deleting your source files.
- **Invoke Mode (`radivine invoke`):** Summon the exact same radial wheel at any time via the **Nemo "Open in Radivine"** right-click action, a desktop keyboard shortcut, or the command line—with full mouse and keyboard navigation.
- **High-Performance File Preview (`radivine-viewer`):** Inspect text and log files in a dedicated, read-only desktop viewer supporting **Full Preview** (virtualized scrolling + streaming search), **Head** (first $N$ lines), and **Tail** (last $N$ lines)—capable of opening `5 GiB` log files in `< 1 ms` of file I/O.
- **Context-Aware & Extensible:** Automatically inspects file types, MIME signatures, permissions, symlinks, and multi-file selections in the background, and lets you define custom shell-free actions in `~/.config/radivine/actions.toml`.

---

## Screenshots

### Branching Radial Wheel (1-Level, 2-Level & 3-Level Navigation)

| Root Wheel | 2-Level Submenu (`File Preview`) | 3-Level Submenu (`Custom -> Scripts`) |
| :---: | :---: | :---: |
| ![Root Wheel](docs/screenshots/wheel-root.png) | ![2-Level Wheel](docs/screenshots/wheel-2level.png) | ![3-Level Wheel](docs/screenshots/wheel-3level.png) |

### Read-Only File Preview (`radivine-viewer`)

| Full Preview (Dark Theme) | Head Preview (Light Theme) | Streaming Search (`Ctrl+F`) |
| :---: | :---: | :---: |
| ![Full Preview Dark](docs/screenshots/viewer-full-dark.png) | ![Head Preview Light](docs/screenshots/viewer-head-light.png) | ![Search Bar Dark](docs/screenshots/viewer-search-dark.png) |

---

## Supported Platforms

| Environment | Support Level | Notes |
| :--- | :--- | :--- |
| **Linux Mint 22.x (Cinnamon on X11)** | **Primary / Tier 1** | Full `Drag + Shift` mid-drag interception in Nemo + `Invoke` mode |
| **Ubuntu / Debian X11 Desktops** (XFCE, MATE, GNOME Xorg, KDE X11) | **Supported** | Full X11 overlay + XDND v5 + `Invoke` mode |
| **Wayland Sessions** | **Invoke Mode / Experimental** | Wayland security protocols disallow global mid-drag X11 interception; use `radivine invoke` via file-manager action or shortcut (`radivine doctor` reports session details) |

---

## Installation

### Option 1: Install from the Debian Package (`.deb`)

Download `radivine_0.1.0_amd64.deb` and `SHA256SUMS.txt` from the [GitHub Releases](https://github.com/marawan6569/Radivine/releases) page:

```bash
# 1. Verify the package checksum
sha256sum -c SHA256SUMS.txt

# 2. Install the package
sudo apt install ./radivine_0.1.0_amd64.deb
```

What the `.deb` package installs:
- `/usr/bin/radivine` — background daemon, CLI client, and diagnostic tool.
- `/usr/bin/radivine-viewer` — standalone read-only file preview application.
- `/usr/share/applications/radivine.desktop` — application launcher entry.
- `/usr/share/nemo/actions/radivine.nemo_action` — **"Open in Radivine"** right-click action for Nemo.
- `/usr/share/doc/radivine/examples/` — opt-in autostart (`.desktop` and `systemd` user unit) and example `config.toml` / `actions.toml` files.

### Option 2: Build from Source

Prerequisites on Linux Mint 22.x / Ubuntu 24.04+:
- Rust `1.85+` (`edition = "2024"`)

```bash
git clone https://github.com/marawan6569/Radivine.git
cd Radivine

# Build optimized release binaries
cargo build --release

# Or build the installable Debian package at target/debian/radivine_0.1.0_amd64.deb
./scripts/build-deb.sh
```

---

## Quick Start & Usage

### 1. Check Desktop Compatibility (`radivine doctor`)

```bash
radivine doctor
```

Verifies X11 connection, required extensions (`Composite`, `XFixes`, `XShape`, `XInput2`, `RandR`, `MIT-SHM`), compositor status, active monitors, and DPI scale.

### 2. Start the Background Daemon

```bash
radivine daemon
```

*(Opt-in Autostart)* To start the daemon automatically on desktop login, copy either the XDG autostart entry or the systemd user service from `/usr/share/doc/radivine/examples/`:

```bash
# Option A: XDG Autostart
mkdir -p ~/.config/autostart
cp /usr/share/doc/radivine/examples/radivine-daemon.desktop ~/.config/autostart/

# Option B: systemd user service
mkdir -p ~/.config/systemd/user
cp /usr/share/doc/radivine/examples/radivine.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now radivine.service
```

### 3. Use Radivine

- **Mid-Drag (`Drag + Shift`):**
  1. Select one or more files in **Nemo** and begin dragging with the left mouse button held.
  2. Press <kbd>Shift</kbd> once while holding the drag. The Radivine radial wheel blooms around your cursor.
  3. Hover over a segment to highlight it; dwell briefly on a parent segment (`File Preview`, `Custom`) to branch into a child wheel.
  4. Release the left mouse button on an enabled action segment to execute it—or release on the center hub (or press <kbd>Shift</kbd> / <kbd>Esc</kbd>) to cancel cleanly.
- **Invoke Mode (`radivine invoke`):**
  - Right-click any selection in Nemo and choose **Open in Radivine**, or run:
    ```bash
    radivine invoke /path/to/file.txt
    ```
  - Navigate with the mouse or keyboard (<kbd>Tab</kbd> / Arrow keys to cycle, <kbd>1</kbd>–<kbd>9</kbd> for direct segment selection, <kbd>Enter</kbd> / <kbd>Space</kbd> to activate, <kbd>Backspace</kbd> to close a submenu, <kbd>Esc</kbd> to cancel).

### CLI Reference

```text
Usage: radivine [--config <PATH>] [--socket <PATH>] <COMMAND>

Commands:
  daemon   Run the persistent background Radivine daemon
  invoke   Summon the radial wheel at the cursor in Invoke mode (`radivine invoke [FILES...]`)
  status   Query the running daemon status (`--json` for machine-readable output)
  reload   Reload `~/.config/radivine/config.toml` and `~/.config/radivine/actions.toml`
  quit     Gracefully stop the running daemon
  doctor   Inspect X11 extensions, compositor status, monitors, and desktop environment
```

---

## Configuration & Custom Actions

Radivine reads optional configuration files from `~/.config/radivine/` (`$XDG_CONFIG_HOME/radivine/`). Copy the commented examples to get started:

```bash
mkdir -p ~/.config/radivine
cp /usr/share/doc/radivine/examples/config.example.toml ~/.config/radivine/config.toml
cp /usr/share/doc/radivine/examples/actions.example.toml ~/.config/radivine/actions.toml
radivine reload
```

### Custom Actions (`~/.config/radivine/actions.toml`)

Custom actions execute directly via `execve` **without a shell** (`sh -c` is rejected) to prevent command-injection vulnerabilities even on filenames containing spaces, quotes, or shell metacharacters:

```toml
[[action]]
id = "custom.sha256_notify"
label = "SHA-256 Digest"
icon = "lucide:code"
description = "Calculate SHA-256 checksum for selected files"
exec = ["/usr/bin/sha256sum", "{paths}"]
per_item = false
wait = true
timeout_ms = 10000
notify = "always"
when = { count = "1..=16" }
```

Supported whole-argument placeholders: `{path}`, `{paths}`, `{uri}`, `{uris}`, `{dir}`, `{name}`.

---

## Architecture Overview

Radivine is organized as a modular Rust workspace with strict safety boundaries:

- **[`crates/radivine-wheel`](crates/radivine-wheel):** `#![forbid(unsafe_code)]`, pure, deterministic, zero-I/O hierarchical radial wheel engine (`0` heap allocations in steady-state pointer/tick/scene loops; `~420 ns` per frame).
- **[`crates/radivine-render`](crates/radivine-render):** `#![forbid(unsafe_code)]` CPU renderer (`tiny-skia`, `cosmic-text`, `resvg`) with premultiplied RGBA8 alpha, cached shadows/glyphs/icons, and incremental damage-rectangle tracking (`~212 µs` at 1080p).
- **[`crates/radivine-core`](crates/radivine-core):** Context model, RFC 2483 `text/uri-list` parser, non-blocking background `Enricher` (`1.5 s` timeout watchdog), and bounded `Dispatcher` thread pool.
- **[`crates/radivine-actions`](crates/radivine-actions):** `#![forbid(unsafe_code)]` built-in actions (`File Preview`, `Copy`, `Properties`), D-Bus integration, and `actions.toml` parser/validator.
- **[`crates/radivine-platform`](crates/radivine-platform):** X11 backend (`x11rb` + POSIX MIT-SHM), `_NET_WM_WINDOW_TYPE_DND` ARGB overlay, `XShape` input/bounding regions, `XInput2` modifier arming, XDND v5 state machine (`XdndActionPrivate`), and persistent `CLIPBOARD` selection server.
- **[`apps/radivine`](apps/radivine):** Single-instance background daemon, Unix domain socket IPC server, and CLI client.
- **[`apps/radivine-viewer`](apps/radivine-viewer):** `#![forbid(unsafe_code)]` standalone `eframe`/`egui` file preview binary (`Full`, `Head`, `Tail`, and streaming `1 MiB` overlap search).

For detailed design documents and benchmarks, see:
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- [`docs/WHEEL_ENGINE_DESIGN.md`](docs/WHEEL_ENGINE_DESIGN.md)
- [`docs/PLATFORM_INTEGRATION.md`](docs/PLATFORM_INTEGRATION.md)
- [`docs/ACTION_SYSTEM.md`](docs/ACTION_SYSTEM.md)
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)
- [`docs/DECISIONS.md`](docs/DECISIONS.md)

---

## Uninstallation

```bash
sudo apt remove radivine
```

User configuration files under `~/.config/radivine/` are preserved on removal.

---

## License

Licensed under either of:

- **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- **MIT License** ([`LICENSE-MIT`](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option. See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for third-party font, icon, and crate license attributions.
