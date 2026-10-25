# Radivine v0.3.0 RC1 — Public Testing

> [!WARNING]
> **PRE-RELEASE — FOR TESTING ONLY. FINAL DESKTOP ACCEPTANCE PENDING.**
>
> `v0.3.0-rc.1` (`radivine_0.3.0~rc1_amd64.deb`) is a public pre-release candidate published for external desktop and Live USB testing on **Linux Mint 22.3 Cinnamon (X11, `amd64`)**. Stable users should continue using [**Radivine v0.2.0**](https://github.com/marawan6569/Radivine/releases/tag/v0.2.0) until final owner desktop acceptance is complete.

**Branch into action.**  
*A fast, extensible, context-aware radial action wheel and desktop utility suite for Linux.*

---

## Highlights of v0.3.0-rc.1

`v0.3.0-rc.1` introduces the optional **Concentric (Nested Rings)** radial wheel layout (`wheel.layout = "concentric"`) alongside the unchanged default **Branching (Child Wheels)** layout (`wheel.layout = "branching"`), portable Arabic script shaping across the wheel renderer and Control Center, and hardened X11/XDND drop safety:

### 1. Optional Concentric (Nested Rings) Wheel Layout (`wheel.layout = "concentric"`)
- **Two Selectable Layouts:** Keep the classic **Branching (Child Wheels)** layout (`wheel.layout = "branching"`, default on new installs and upgrades) or switch to **Concentric (Nested Rings)** (`wheel.layout = "concentric"`) in **Control Center → Wheel → Wheel Layout** (or `~/.config/radivine/config.toml`).
- **Single-Center Multi-Ring Expansion:** Submenus (`File Preview`, `Convert ▸ Image`, `Custom Actions`) promote onto outward concentric rings (`R_hub = 50 lp`, root ring `50..118 lp`, outer ring `L >= 1` at `118 + 66·(L-1)..118 + 66·(L-1) + 60 lp`, inter-ring gap `6 lp`, minimum `30°` sector sweep with overflow pagination up to 6 rings) centered on a single immobile root origin (`C_0`).
- **Path-Continuity Markers & Breadcrumb Caption:** Renders angular path-continuity markers linking each open parent sector across the `6 lp` radial gap to its child ring, plus a fixed bottom caption pill (`Parent › Child › Action`) and a displaced-origin approach guide when clamped near screen edges.
- **Deferred Active-Session Reload:** Changing `wheel.layout` in Control Center or via `radivine reload` while a wheel session is open preserves the active session's snapshotted layout (`layout_pending = true`) and applies the new layout on the next summon.

### 2. Reserved `668 × 742` Viewport Contract & Automatic Small-Screen Fallback
- **4-Ring + Fixed Caption Reservation:** Before mapping the overlay, Concentric mode reserves 4 rings (`R_reserve(4) = 326 lp`, `±334 lp` horizontal/top with `8 lp` screen margin) plus the fixed bottom caption box (`Cy + 326..388 lp`, `+408 lp` bottom extent with `20 lp` margin), requiring a minimum monitor workarea of **`668 × 742` logical pixels**.
- **Automatic Session Fallback:** Standard laptop and desktop viewports (`1366×768@1x`, `1920×1080@1x`, `1920×1080@1.25x`, `3840×2160@2x`) support Concentric mode with zero clipping. Smaller logical viewports (`1280×720@1x`, `1920×1080@1.5x`) automatically fall back to `Branching` (`layout_fallback_reason = "insufficient_space"`) for that session without modifying `~/.config/radivine/config.toml`.

### 3. Strict Non-Actionable Drop & Click Safety Envelope (`T-SAFE1`–`T-SAFE10`)
- **Nine Non-Actionable Reject Zones:** Releasing a file drag or clicking in the central hub (`r < 50 lp`), inter-ring radial gaps (`6 lp`), angular separator gaps, outer ring margin (`r > R_out(D)`), fixed bottom caption box, displaced-center approach corridor (`> 4 lp` anchor offset), protected pending-switch sibling sectors (`90 ms` outer-ring steering protection), submenu parent sectors (on drag release), or outside the cluster reports `XdndStatus(accept=0)` and finishes with `XdndFinished(success=0)`—never executing an unintended action or dropping files into underlying Nemo windows (`input.passthrough = false` default).
- **Drop-Time Coordinate Re-Evaluation:** `XdndDrop` re-evaluates the final pointer position before committing and never executes a stale hovered action.

### 4. Portable Arabic Font Fallback & Debian Runtime Dependencies
- **Arabic Script Shaping in Wheel & UI:** `radivine-render` shapes Arabic filenames and custom action labels via system `DejaVu Sans` (`DejaVuSans.ttf` / `DejaVuSans-Bold.ttf`) while keeping bundled `Inter-Variable.ttf` isolated for deterministic Latin typography; `radivine-control` and `radivine-viewer` use Fontconfig (`egui_system_font_provider` + `harfrust`) for contextual Arabic glyph joining and Lam-Alef (`لا`) ligatures.
- **Explicit Debian Package Dependencies:** `radivine_0.3.0~rc1_amd64.deb` declares `Depends: libc6 (>= 2.34), libgcc-s1 (>= 3.0), fonts-dejavu-core, libfontconfig1` and `Recommends: nemo, libx11-6, libegl1, libgl1, systemd, fonts-noto-core`.

---

## Anonymous Download & Installation (No GitHub Login Required)

- **Primary Target Desktop:** **Linux Mint 22.3 Cinnamon (X11, `amd64`)** (Installed or Live USB session)
- **Compatible Base:** **Ubuntu 24.04 LTS+** (`amd64`, `glibc >= 2.34`)
- **Package Filename:** `radivine_0.3.0~rc1_amd64.deb` (`16,152,808` bytes / `15.40 MiB`)
- **SHA-256 Checksum:**
  ```text
  78ded9478b980b01165b264059d1192715a8f33f10d1cfca04f528e06a84297a  radivine_0.3.0~rc1_amd64.deb
  ```

### Quick Install on Linux Mint 22.3 Cinnamon (Live USB or Installed System)

Open a terminal in your Linux Mint 22.3 Cinnamon session (no GitHub account or token required):

```bash
# 1. Download the pre-release .deb and SHA256SUMS.txt directly
mkdir -p /tmp/radivine-rc1 && cd /tmp/radivine-rc1
curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.3.0-rc.1/radivine_0.3.0~rc1_amd64.deb
curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.3.0-rc.1/SHA256SUMS.txt

# 2. Verify the SHA-256 checksum
sha256sum -c SHA256SUMS.txt

# 3. Install the package via apt
sudo apt install -y ./radivine_0.3.0~rc1_amd64.deb

# 4. Start the background radial wheel service and open Radivine Control Center
systemctl --user daemon-reload
systemctl --user enable --now radivine.service
radivine-control
```

---

## Testing Guide for Linux Mint 22.3 Cinnamon Live USB

1. **Enable Concentric (Nested Rings) Layout:**
   - In **Radivine Control Center**, open the **Wheel** page, change **Wheel Layout** from `Branching (Child Wheels)` to **`Concentric (Nested Rings)`**, and click **Save Changes** (or run `radivine status` to confirm `active_layout: concentric`).
2. **Create Disposable Test Files (Safe Live USB Testing):**
   ```bash
   mkdir -p /tmp/radivine-test/src /tmp/radivine-test/dst
   printf 'Radivine RC1 test file\n' > /tmp/radivine-test/src/sample.txt
   printf 'محتوى تجريبي عربي\n' > "/tmp/radivine-test/src/تجرية_ملف.txt"
   nemo /tmp/radivine-test/src /tmp/radivine-test/dst &
   ```
3. **Test `Drag + Shift` & Invoke Mode in Nemo:**
   - Drag `sample.txt` in Nemo and press <kbd>Shift</kbd> to summon the Concentric radial wheel.
   - Hover **File Preview** to open the outer ring (`Full Preview`, `Head`, `Tail`), or test releasing on the center hub, inter-ring gap, caption pill, or outside the wheel over `/tmp/radivine-test/dst` to verify clean cancellation with zero accidental file moves.
   - Right-click any file in Nemo and select **Open in Radivine** to test Invoke mode, keyboard navigation, and Arabic filename rendering (`تجرية_ملف.txt`).
   - Switch between **Concentric (Nested Rings)** and **Branching (Child Wheels)** in **Control Center → Wheel** to compare both layouts live.

---

## Upgrade from v0.2.0 & Rollback Instructions

### Upgrading from `v0.2.0` (`0.2.0 -> 0.3.0~rc1`)
- Installing `./radivine_0.3.0~rc1_amd64.deb` over `v0.2.0` preserves `~/.config/radivine/config.toml`, `~/.config/radivine/actions.toml`, `~/.config/radivine/convert.toml`, and `~/.local/state/radivine/jobs.sqlite3` byte-for-byte. Existing configurations without `wheel.layout` remain on `"branching"` until changed in Control Center.
- Reload user units after upgrading:
  ```bash
  systemctl --user daemon-reload
  systemctl --user restart radivine.service
  ```

### Rolling Back to Stable `v0.2.0` (or Switching Back to Branching)
- **Instant Layout Rollback (No Reinstall Required):** Set **Wheel Layout** to **`Branching (Child Wheels)`** in `radivine-control` (or set `layout = "branching"` under `[wheel]` in `~/.config/radivine/config.toml` and run `radivine reload`).
- **Full Package Downgrade to Stable `v0.2.0`:**
  ```bash
  curl -fLO https://github.com/marawan6569/Radivine/releases/download/v0.2.0/radivine_0.2.0_amd64.deb
  sudo apt install -y --allow-downgrades ./radivine_0.2.0_amd64.deb
  systemctl --user daemon-reload
  systemctl --user restart radivine.service
  ```

---

## Known Limitations

- **Pre-release Status:** Final N5 owner desktop acceptance across all checklist items is in progress; `v0.3.0-rc.1` is published for testing only and is not marked as the stable Latest release.
- **X11 Display Server Required for Radial Wheel Overlay:** Global `Drag + Shift` (`XDND`) interception and pointer-tracking overlay require an **X11 session** (`XDG_SESSION_TYPE=x11`).
- **Minimum Viewport for Concentric Layout (`668 × 742` Logical Pixels):** Concentric mode reserves 4 rings plus the bottom caption pill (`668 × 742` logical px). Monitors with smaller logical workareas (such as `1280×720@1x` or `1920×1080@1.5x`) automatically fall back to `Branching` for that session.
- **Static Raster Images Only (`PNG`, `JPEG`, `WebP`):** Built-in conversion supports static `PNG`, `JPEG`, and `WebP` images (`<= 16,384 × 16,384` px, `<= 100 MP`).
- **Left-to-Right Widget Container Alignment (`egui 0.37`):** While `cosmic-text` / `harfrust` performs contextual Arabic glyph shaping and inline bidirectional run reordering, `egui 0.37` widget container layout alignment remains LTR.

---

## License

Dual-licensed under **`MIT OR Apache-2.0`**. See [`LICENSE-MIT`](https://github.com/marawan6569/Radivine/blob/main/LICENSE-MIT), [`LICENSE-APACHE`](https://github.com/marawan6569/Radivine/blob/main/LICENSE-APACHE), and [`THIRD_PARTY_NOTICES.md`](https://github.com/marawan6569/Radivine/blob/main/THIRD_PARTY_NOTICES.md).
