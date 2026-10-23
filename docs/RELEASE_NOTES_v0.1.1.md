# Radivine v0.1.1 — Control Center & Background Service

**Branch into action.**  
*A fast, extensible, context-aware radial action wheel for your Linux desktop.*

---

## Highlights of v0.1.1

`v0.1.1` turns Radivine into a complete, self-contained desktop application with a native graphical **Control Center** (`radivine-control`) and terminal-independent background service management (`systemd --user`):

1. **Native Radivine Control Center (`radivine-control`):**
   - Launch **Radivine** directly from your Linux Mint / Cinnamon application menu (`radivine.desktop` now opens `radivine-control`).
   - **Dashboard:** Start, stop, restart, and hot-reload the background daemon without keeping a terminal open, inspect live IPC telemetry (PID, uptime, resident memory `VmRSS`, session state, scale, active custom actions, and frame/summon latencies), and toggle **Launch Radivine at Login** with one click.
   - **General Settings:** Configure login autostart, color theme (`Dark`, `Light`, `Auto`), UI scale factor (`Auto` or `0.5×`–`4.0×`), desktop notifications, and default `radivine-viewer` Head/Tail line counts.
   - **Wheel Settings:** Adjust root and child wheel inner/outer radii, parent-to-child gap, submenu dwell time, intent-triangle corridor grace period, leave grace period, segment hysteresis, backdrop dimming, reduced motion, and mid-drag trigger modifier (`Shift`, `Ctrl`, `Alt`, `Super`, or `Disabled`).
   - **Custom Actions Manager:** Create, edit, enable/disable, search, and delete shell-free custom actions in `~/.config/radivine/actions.toml` with interactive whole-argument placeholder insertion (`{path}`, `{paths}`, `{uri}`, `{uris}`, `{dir}`, `{name}`) and live validation before saving.
   - **Diagnostics & Logs:** Run `radivine doctor`, inspect bounded daemon logs (`journalctl --user -u radivine.service` or `~/.local/state/radivine/daemon.log`) with severity and search filters, and copy/export a privacy-redacted diagnostic report for GitHub issues.
2. **Terminal-Independent Background Daemon Lifecycle (`systemd --user` + Detached Fallback):**
   - Ships `/usr/lib/systemd/user/radivine.service` (`WantedBy=default.target`, `SuccessExitStatus=0 143`) so login autostart works out of the box on Linux Mint Cinnamon (where `graphical-session.target` is inactive).
   - Propagates graphical session variables (`DISPLAY`, `XAUTHORITY`, `XDG_SESSION_TYPE`, `XDG_CURRENT_DESKTOP`, `DBUS_SESSION_BUS_ADDRESS`) into `systemd --user` before interactive starts/restarts.
   - Automatically falls back to a detached `process_group(0)` daemon logging to `~/.local/state/radivine/daemon.log` when `systemd --user` is unavailable.
   - Detects and safely resolves duplicate `systemd` + XDG autostart conflicts and legacy `graphical-session.target` unit overrides (backing up user files to `.bak`).
3. **Format-Preserving TOML Persistence (`toml_edit`):**
   - Preserves user comments, formatting, and unknown keys in both `config.toml` and `actions.toml`.
   - Detects external file modifications (`mtime` + content digest) before overwriting, supports `.bak` backups, and writes atomically (`.tmp` + `fsync` + `rename`).

---

## Installation & Upgrade (`radivine_0.1.1_amd64.deb`)

Download `radivine_0.1.1_amd64.deb` and `SHA256SUMS.txt` from the release assets below:

```bash
# 1. Verify the SHA-256 checksum
sha256sum -c SHA256SUMS.txt

# 2. Install or upgrade the Debian package
sudo apt install ./radivine_0.1.1_amd64.deb

# 3. Open Radivine Control Center (or launch "Radivine" from your application menu)
radivine-control
```

---

## License

Dual-licensed under **`MIT OR Apache-2.0`**. See [`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
