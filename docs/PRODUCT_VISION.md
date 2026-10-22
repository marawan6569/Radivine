# Radivine — Product Vision

> *Branch into action.*

Status: **Planning (pre-implementation)** · Last updated: 2026-10-22

---

## 1. What Radivine is

Radivine is an open-source, local-first, **context-aware radial action engine** for the desktop, written in Rust.

The user drags something (initially: files from Nemo), taps a modifier key (`Shift` by default) mid-drag, and a **radial wheel of actions grows out of the cursor**. Actions that have sub-options open a **child wheel next to the parent wheel** (the parent stays visible), forming a small "vine" of connected wheels. The user drops the dragged item onto the action they want, and Radivine runs it.

The interaction is inspired by *Tangerine: File Converter* for macOS (Shift-during-drag → radial wheel → drop onto a target), but **Radivine is not a file converter**. It is a general engine: wheels + contexts + actions.

**The wheel interaction is the product.** Everything else (file handling, previews, settings) is secondary and must never dominate the architecture.

## 2. Goals

| # | Goal | Measure of success |
|---|------|--------------------|
| G1 | Instant, natural radial interaction during a real drag from Nemo on Linux Mint Cinnamon (X11) | Proof-of-concept acceptance criteria in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) (M3) pass |
| G2 | Hierarchical wheels: child wheels emerge beside parents; parents stay visible; smooth navigation between them | Wheel engine acceptance criteria (M1) pass |
| G3 | Wheel engine independent of actions and platforms | `radivine-wheel` has no dependency on context/action/platform crates |
| G4 | Context-aware actions | Same drag shows different enabled actions for text vs. binary files |
| G5 | Extensible actions | A new built-in action is one `impl Action` + one menu entry; a user custom action is a TOML block |
| G6 | Lightweight background daemon | Engineering targets in [ARCHITECTURE.md §9](ARCHITECTURE.md#9-performance-targets) (idle ≈ 0 % CPU) |
| G7 | Honest cross-platform story | Explicit capability matrix; graceful fallbacks; no security bypasses |
| G8 | Privacy | Fully offline; no telemetry; input observation minimised (see [PLATFORM_INTEGRATION.md §3.6](PLATFORM_INTEGRATION.md)) |

## 3. Non-goals (initial releases)

- **Not** a file converter, file manager, or archive tool.
- **Not** a launcher, dock, or general global-hotkey manager.
- **No** binary plugin ABI, WASM plugin host, or scripting runtime in v1 (custom actions are declarative command templates).
- **No** settings GUI in v1 (TOML configuration; a GUI may come later).
- **No** cloud services, accounts, telemetry, or network access.
- **No** privileged input access (no `/dev/input`, no `uinput`, no `input` group, no root helpers) on any platform.
- **No** emulation of X11 global input on Wayland by weakening compositor security.

## 4. Target platforms

| Phase | Platform | Status |
|-------|----------|--------|
| Primary | Linux Mint 22.x, Cinnamon, **X11**, Nemo (dev machine: Mint 22.3, Muffin 6.6.3, Nemo 6.6.3, GTK 3.24.41, Xorg 21.1.12, XI 2.4, NVIDIA driver) | Target of the proof of concept |
| Secondary | Other X11 desktops with a compositing manager and GTK3 file managers (Thunar, Caja, Nautilus-on-X11) | Expected to work; verified per manual test matrix |
| Future | Wayland (KDE/wlroots/COSMIC via layer-shell; GNOME/Cinnamon-Wayland via extensions) | Degraded/alternative UX — see compatibility matrix in [PLATFORM_INTEGRATION.md](PLATFORM_INTEGRATION.md) |
| Future | Windows 10/11, macOS | Architecture prepared (platform trait, CPU-rendered premultiplied RGBA surface); not implemented |

## 5. Initial scope (v0.1)

1. Background daemon (`radivine`) started via XDG autostart.
2. **Drag mode**: Nemo file drag → fresh `Shift` press → wheel at the cursor → drop on an action.
3. **Invoke mode** (fallback + accessibility path): `radivine invoke <uris…>` (e.g. from a Nemo action in the right-click menu) → same wheel at the cursor, operated by click or keyboard.
4. Nested wheels with adjacent child placement, hover intent, hysteresis, animations, screen-edge handling, multi-monitor, HiDPI.
5. Example actions:
   - **File Preview** ▸ **Full Preview** · **Head** (first N lines, default 50) · **Tail** (last N lines, default 50)
   - **Copy** (copy the dragged files to the clipboard so they can be pasted in Nemo)
   - **Properties** (opens the file manager's properties dialog via `org.freedesktop.FileManager1`)
   - **Custom Actions** ▸ user-defined command templates from `actions.toml`
6. Text configuration (`~/.config/radivine/config.toml`, `actions.toml`).

The wheel engine is validated with **logging dummy actions first** (M1–M3); real actions come later (M4–M5).

## 6. Future possibilities (explicitly out of v0.1 scope)

- Additional contexts: dragged **text**, **URLs**, **images** (graphical preview), clipboard contents, application-specific contexts.
- Secondary wheels on different modifiers (e.g. `Shift+Alt` → "tools" wheel, as Tangerine does with Option+Shift).
- Dynamic submenus (e.g. "Open With ▸ <installed apps>") via a menu-provider interface.
- Live-follow `tail -f` in the viewer.
- Wayland backends (layer-shell overlay, portal-based triggers, drop shelf), Windows and macOS backends.
- GUI settings, theming, user-defined wheel layouts.
- Third-party action crates; later possibly an out-of-process plugin protocol.

## 7. Experience principles

1. **Appear where the hand is.** The wheel grows from the cursor; no window to find.
2. **Never steal the drag.** Releasing on the wheel cluster but not on an action (hub, gaps, disabled items) cancels cleanly — nothing happens to the file. Outside the cluster the drag passes through to the application underneath, as if Radivine weren't there. Tapping the trigger again hides the wheel and the drag continues normally.
3. **Forgiving navigation.** Moving from a parent segment to its child wheel must never accidentally collapse the child.
4. **Calm motion.** Short (each animation ≤ 160 ms, composed sequences ≤ ~210 ms), restrained, interruptible animations; honour reduced-motion settings.
5. **Legible at a glance.** Icons + short labels; the hub shows the full label/description of the hovered item and the reason an item is disabled.
6. **Brand-consistent.** Radivine's dark green (`#044C3C`) and leaf green (`#00A87C`) identity; child wheels connect to parents with a "vine" stem, echoing the logo.

## 8. Brand assets (inspected, unchanged)

| File | Notes |
|------|-------|
| `assets/branding/logo-primary.png` | 1448×1086 RGB (white background). **Byte-identical to `logo-horizontal.png`** (same MD5). |
| `assets/branding/logo-horizontal.png` | See above. |
| `assets/branding/logo-vertical.png` | 1254×1254 RGB, symbol above wordmark. |
| `assets/branding/logo-monochrome.png` | 1448×1086 RGB. |
| `assets/branding/logo-icon.png` | 1254×1254 **RGBA** symbol. Byte-identical to `assets/icons/app-icon.png`. |
| `assets/icons/app-icon.png` | 1254×1254 RGBA. |
| `assets/icons/app-icon-monochrome.png` | 1254×1254 RGBA black symbol. (The planning brief listed it under `assets/branding/`; it actually lives in `assets/icons/`.) |
| `assets/icons/app-icon-white.png` | 1254×1254 RGB symbol on white. |

Measured dominant colours: **dark vine green ≈ `#044C3C`** (`#004C3C`–`#084838`), **leaf green ≈ `#00A87C`** (`#00A478`–`#04986C`).

The symbol (central node with ring, radiating branches ending in hollow circle "nodes", and leaves) directly informs the wheel's visual language: hub = central node, submenu indicator = hollow node, parent→child connector = vine stem.

> [!NOTE]
> Most branding PNGs have opaque white backgrounds and none are SVG. For dark-mode UI (wheel hub, viewer, notifications) transparent and/or vector variants will eventually be needed. **This plan does not create or modify branding**; producing such variants is a task for the project owner/designer (tracked in [RISKS_AND_MITIGATIONS.md](RISKS_AND_MITIGATIONS.md), R-24).
