# Radivine — Local Development Guide (`docs/DEVELOPMENT.md`)

This guide explains how to build, run, and test Radivine (`radivine`, `radivine-worker`, `radivine-control`, `radivine-viewer`, and Nemo file-manager integration) directly from the Rust workspace without rebuilding or installing `.deb` packages after every code change.

---

## 1. Architecture & Isolation Overview

When you run Radivine through the scripts in [`scripts/dev/`](../scripts/dev/), the entire stack runs from your workspace's `target/{release,debug}/` directory inside an isolated development sandbox:

| Resource | Installed Package (`v0.2.0`) | Development Environment (`scripts/dev/`) |
|---|---|---|
| **Daemon binary** | `/usr/bin/radivine` | `<repo>/target/<profile>/radivine` (`RADIVINE_DAEMON_BIN`) |
| **Worker binary** | `/usr/bin/radivine-worker` | `<repo>/target/<profile>/radivine-worker` (`RADIVINE_WORKER_BIN`) |
| **Control Center binary** | `/usr/bin/radivine-control` | `<repo>/target/<profile>/radivine-control` (`RADIVINE_CONTROL_BIN`) |
| **Preview Viewer binary** | `/usr/bin/radivine-viewer` | `<repo>/target/<profile>/radivine-viewer` (`RADIVINE_VIEWER_BIN`) |
| **Daemon IPC socket** | `$XDG_RUNTIME_DIR/radivine/radivine.sock` | `$XDG_RUNTIME_DIR/radivine-dev/radivine.sock` (`RADIVINE_SOCKET`) |
| **Worker IPC socket** | `$XDG_RUNTIME_DIR/radivine/worker.sock` | `$XDG_RUNTIME_DIR/radivine-dev/worker.sock` (`RADIVINE_WORKER_SOCKET`) |
| **Control lock socket** | `$XDG_RUNTIME_DIR/radivine/control.sock` | `$XDG_RUNTIME_DIR/radivine-dev/control.sock` (`RADIVINE_CONTROL_SOCKET`) |
| **Config directory** | `~/.config/radivine/` | `<repo>/.radivine-dev/config/radivine/` (`XDG_CONFIG_HOME`) |
| **Jobs SQLite DB** | `~/.local/state/radivine/jobs.sqlite3` | `<repo>/.radivine-dev/state/radivine/jobs.sqlite3` (`RADIVINE_WORKER_DB`) |
| **Logs** | `journalctl --user -u radivine.service` | `<repo>/.radivine-dev/state/radivine/{daemon.log,worker.log}` |
| **Nemo Action** | `/usr/share/nemo/actions/radivine.nemo_action` | `~/.local/share/nemo/actions/radivine-dev.nemo_action` |

- **Data Safety:** Your real `~/.config/radivine/` settings and `~/.local/state/radivine/jobs.sqlite3` database are never modified or deleted by development runs. On the very first development startup, any existing `config.toml`, `actions.toml`, or `convert.toml` in `~/.config/radivine/` are copied once into `<repo>/.radivine-dev/config/radivine/` as a convenient starting point.
- **X11 Drag Listener Conflict Prevention:** Because only one `radivine daemon` should intercept X11 `Drag + Shift` events on the active display at a time, `dev-start.sh` checks whether the installed `radivine.service` is active and either reports the conflict or stops it cleanly when `--stop-installed` is passed.

---

## 2. Prerequisites

- Linux Mint 22.x (Cinnamon, X11 session) or compatible X11 desktop
- Rust stable toolchain (`cargo`, `rustc >= 1.85`)
- Standard C/pkg-config build tools for `libwebp` (`build-essential`, `pkg-config`, `libwebp-dev`)

---

## 3. First-Time Development Setup & Quick Start

From the repository root, run:

```bash
# Build release binaries, stop any active installed radivine.service, enable Nemo dev action,
# start the dev daemon + worker, and open the dev Control Center:
./scripts/dev/dev-start.sh --stop-installed --enable-nemo --control
```

Check the live status of all development components at any time:

```bash
./scripts/dev/dev-status.sh
```

---

## 4. Starting, Stopping, and Restarting

### Start Development Environment
```bash
# Start (or verify already-running) release build:
./scripts/dev/dev-start.sh

# Start and open Control Center directly on the Jobs page:
./scripts/dev/dev-start.sh --page jobs
```

`dev-start.sh` is **idempotent**: running it again while the development daemon and worker are already running will verify their health without spawning duplicate processes.

### Restart After Code Changes
After editing Rust code in the workspace, rebuild and restart in a single command (reusing Cargo's incremental build cache):

```bash
./scripts/dev/dev-restart.sh
```

If `cargo build` fails due to a compile error, `dev-restart.sh` aborts cleanly and prints the compiler output instead of starting stale binaries.

### Stop Development Environment
```bash
# Stop only the development daemon, worker, and Control Center:
./scripts/dev/dev-stop.sh
```

`dev-stop.sh` verifies `/proc/<pid>/exe` before signaling any process, ensuring it only stops processes running from `<repo>/target/`.

---

## 5. Debug vs. Release Builds

By default, `dev-start.sh` and `dev-restart.sh` use `--release` so radial wheel rendering and image conversion run at full speed.

To switch to an unoptimized `--debug` build for faster compile times or debugger attachment:

```bash
# Build and start debug binaries (automatically stops a running release dev instance first):
./scripts/dev/dev-start.sh --debug

# Switch back to release binaries:
./scripts/dev/dev-restart.sh --release
```

---

## 6. Opening the Control Center & Running CLI Commands

### Launch Control Center via `dev-start.sh`
```bash
# Open Control Center (or focus existing dev Control Center window):
./scripts/dev/dev-start.sh --skip-build --control

# Open directly to a specific page (dashboard, general, wheel, actions, conversion, jobs, diagnostics, about):
./scripts/dev/dev-start.sh --skip-build --page conversion
./scripts/dev/dev-start.sh --skip-build --page jobs
```

### Run CLI Commands Directly in Your Terminal
Source `.radivine-dev/env.sh` in any terminal shell to put the workspace binaries first in `PATH` and point all `RADIVINE_*` and `XDG_*` variables at the development instance:

```bash
source .radivine-dev/env.sh

radivine status
radivine-worker status
radivine-worker list
radivine-control --page jobs
```

---

## 7. Using Radivine from Nemo (Development Integration)

Both Nemo interaction modes work with the development build:

1. **Drag + Shift (`XDND`):**
   - Start dragging one or more files in Nemo and press `Shift`.
   - The running development `radivine daemon` (`target/<profile>/radivine`) intercepts the drag and summons the radial wheel.
2. **Right-Click Context Menu (`Open in Radivine (Dev)`):**
   - Enable the user-scoped Nemo override:
     ```bash
     ./scripts/dev/dev-nemo.sh enable
     ```
   - This creates `~/.local/share/nemo/actions/radivine-dev.nemo_action` and `~/.local/share/nemo/actions/radivine-dev-invoke.sh` (which executes `<repo>/target/<profile>/radivine --socket <dev-socket> invoke "$@"`), and temporarily adds `'radivine.nemo_action'` to `org.nemo.plugins disabled-actions` (saving the original list to `.radivine-dev/nemo-disabled-actions.orig`) so the installed `/usr/share/nemo/actions/radivine.nemo_action` does not appear twice.
   - Nemo automatically watches `~/.local/share/nemo/actions/` and refreshes its context menu immediately without requiring `nemo --quit` or root privileges.

---

## 8. Testing Advanced Convert, Conversion Settings, and Jobs

With the development environment running (`./scripts/dev/dev-start.sh --stop-installed --enable-nemo`):

1. **Quick Convert & Advanced Convert from Nemo:**
   - Select one or more images (including filenames with spaces, Unicode/Arabic characters, or `%` characters) in Nemo.
   - Right-click → **Open in Radivine (Dev)** (or **Drag + Shift**).
   - Navigate to **`Convert → Image → Advanced…`**.
   - The wheel daemon spawns `<repo>/target/<profile>/radivine-control convert --stdin` connected to the development worker and config.
   - Adjust target format (`PNG`, `JPEG`, `WebP`), quality, resize mode, or suffix, and click **Convert**.
   - Click **Open Jobs** in the submission banner to open the development Control Center directly on the **Jobs** page.
2. **Direct CLI Invocation of Advanced Convert:**
   ```bash
   source .radivine-dev/env.sh
   radivine-control convert "/path/to/My Image 1.webp" "/path/to/My Image 2.png"
   ```
3. **Conversion Settings & Live Worker Reload:**
   ```bash
   ./scripts/dev/dev-start.sh --skip-build --page conversion
   ```
   - Edit presets, output suffix, metadata policies, or worker concurrency and click **Save Changes**.
   - Changes are written to `.radivine-dev/config/radivine/convert.toml` and `.radivine-dev/config/radivine/config.toml`, and `reload_settings` is sent to the running development worker.

---

## 9. Logs & Troubleshooting

- **Follow Daemon Logs:**
  ```bash
  tail -f .radivine-dev/state/radivine/daemon.log
  ```
- **Follow Worker Logs:**
  ```bash
  tail -f .radivine-dev/state/radivine/worker.log
  ```
- **Increase Log Verbosity (`debug` / `trace`):**
  ```bash
  RUST_LOG=debug RADIVINE_LOG=debug ./scripts/dev/dev-restart.sh
  ```
- **Inspect Environment & Process Ownership:**
  ```bash
  ./scripts/dev/dev-status.sh
  ```

---

## 10. Switching Back & Restoring the Original Installed Environment

To stop all development processes, remove the development Nemo override (`~/.local/share/nemo/actions/radivine-dev.nemo_action` and `radivine-dev-invoke.sh`), restore `org.nemo.plugins disabled-actions`, and restart the system-installed `radivine.service`:

```bash
./scripts/dev/dev-stop.sh --all
```

Or run the steps individually:

```bash
# 1. Stop dev daemon, worker, and Control Center:
./scripts/dev/dev-stop.sh

# 2. Restore original Nemo action state:
./scripts/dev/dev-nemo.sh restore

# 3. Start the installed systemd user service:
systemctl --user start radivine.service
```

---

## 11. Testing Concentric vs. Branching Layouts in Isolated `Xvfb`

To run the full real-X11 protocol, `XShape`, `XDND`, and `668×742` viewport fallback suites without touching your live Cinnamon desktop or installed `radivine.service`:

```bash
xvfb-run -a -s "-screen 0 1920x1080x24 +extension COMPOSITE +extension RANDR" \
  cargo test -p radivine --test nested_acceptance \
  t_x111_to_t_x118_live_xvfb_concentric_safety_and_protocol_gates -- --nocapture --test-threads=1
```
