# Radivine v0.1.0 — Initial Release

**Branch into action.**  
*A fast, extensible, context-aware radial action wheel for your Linux desktop.*

---

## Highlights

- **Mid-Drag Activation in Nemo (`Drag + Shift`):** Press <kbd>Shift</kbd> while dragging one or more files in **Nemo** on Linux Mint Cinnamon (X11) to summon a branching radial wheel directly under your cursor in `< 27 ms`—without interrupting your drag or modifying your source files.
- **Universal Invoke Mode (`radivine invoke`):** Summon the radial wheel at any time via Nemo's **"Open in Radivine"** context-menu action, a desktop keyboard shortcut, or the CLI, with full pointer and keyboard navigation (<kbd>Tab</kbd>, Arrow keys, <kbd>1</kbd>–<kbd>9</kbd>, <kbd>Enter</kbd>, <kbd>Backspace</kbd>, <kbd>Esc</kbd>).
- **1-to-3-Level Hierarchical Radial Navigation:** Smoothly branch from the root wheel into child submenus (`File Preview`, `Custom -> Scripts`) with intent-triangle diagonal corridor protection and automatic screen-edge adaptation.
- **Standalone Read-Only File Preview (`radivine-viewer`):** Inspect text and log files in **Full Preview**, **Head** (first $N$ lines), or **Tail** (last $N$ lines) modes. Opens `5 GiB` log files in `< 1 ms` of file I/O using reverse `pread` scanning (never `mmap`), with virtualized scrolling, streaming `1 MiB` overlap search (<kbd>Ctrl+F</kbd>), Go-to-line, and `UTF-8` / `UTF-16 LE` / `UTF-16 BE` support.
- **Context-Aware Built-In & Custom Actions:** Asynchronous background file inspection (`1.5 s` timeout watchdog for hung network mounts), multi-target X11 `CLIPBOARD` file copy (`x-special/gnome-copied-files` + `text/uri-list` + `UTF8_STRING`), native file manager Properties dialog via D-Bus (`org.freedesktop.FileManager1`), and user-defined shell-free custom actions in `~/.config/radivine/actions.toml`.

---

## Supported Platforms

- **Primary Platform:** Linux Mint 22.x (Cinnamon desktop on X11, `x86_64` / `amd64`) with Nemo.
- **Compatible X11 Desktops:** Ubuntu 24.04+ and Debian-based X11 desktops (`libc6 >= 2.34`).
- **Wayland Sessions:** `radivine invoke` is supported via file-manager action or shortcut; global mid-drag X11 interception requires an X11 desktop session (`radivine doctor` reports session compatibility).

---

## Installation (`radivine_0.1.0_amd64.deb`)

Download `radivine_0.1.0_amd64.deb` and `SHA256SUMS.txt` from the release assets below:

```bash
# 1. Verify the SHA-256 checksum
sha256sum -c SHA256SUMS.txt

# 2. Install the Debian package
sudo apt install ./radivine_0.1.0_amd64.deb

# 3. Verify your desktop environment
radivine doctor

# 4. Start the daemon
radivine daemon
```

### Opt-In Autostart

Radivine does **not** force background autostart upon package installation. To enable automatic startup on login, copy either example from `/usr/share/doc/radivine/examples/`:

```bash
# XDG Autostart:
mkdir -p ~/.config/autostart
cp /usr/share/doc/radivine/examples/radivine-daemon.desktop ~/.config/autostart/

# Or systemd user service:
mkdir -p ~/.config/systemd/user
cp /usr/share/doc/radivine/examples/radivine.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now radivine.service
```

---

## Configuration & Custom Actions

Copy the example configuration files to `~/.config/radivine/` and run `radivine reload`:

```bash
mkdir -p ~/.config/radivine
cp /usr/share/doc/radivine/examples/config.example.toml ~/.config/radivine/config.toml
cp /usr/share/doc/radivine/examples/actions.example.toml ~/.config/radivine/actions.toml
radivine reload
```

---

## Known Limitations in v0.1.0

- **X11 Mid-Drag Interception Only:** Global `Drag + Shift` interception relies on X11 `XFixes` / `XInput2` / `XDND v5` protocols; on Wayland sessions, use `radivine invoke` (or the Nemo **"Open in Radivine"** action).
- **Read-Only Text Previews:** `radivine-viewer` previews text files (`UTF-8`, `UTF-16 LE/BE`, 8-bit text) and disables preview segments on binary files, directories, and special files with a clear reason caption.

---

## License

Dual-licensed under **`MIT OR Apache-2.0`** ([`LICENSE-MIT`](../LICENSE-MIT), [`LICENSE-APACHE`](../LICENSE-APACHE)). See [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md) for third-party attributions.
