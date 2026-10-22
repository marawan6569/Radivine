# Radivine — Risks and Mitigations

Status: **Living document** · Last updated: 2026-10-22
Severity: Critical / High / Medium / Low. Likelihood: Very low / Low / Medium / High / Certain.
Priority = severity × likelihood (P1 highest). Owner of every risk: the implementer, escalating to the project owner when a mitigation changes scope.

---

## 1. Top risks (P1–P2)

| ID | Risk | Category | Sev. | Likelihood | Prio | Mitigation | Contingency / trigger |
|----|------|----------|------|-----------|------|------------|-----------------------|
| **R-01** | Nemo's GTK drag does not retarget to an overlay mapped mid-drag | Technical (core feasibility) | Critical | **Retired (M0 S-1 PASS)** | **Resolved** | Verified 100% retargeting in `1.24–2.80 ms` across Nemo 6.6.3 Icon, List, and Compact views (`spikes/xdnd-overlay/`); 0-px `WarpPointer` nudge (`pointer_nudge = true`) guarantees immediate retargeting even with zero user pointer motion | See [`docs/spikes/S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md) |
| **R-31** | User presses `Shift` over a Nemo folder and releases mouse button with **zero pointer motion** → GTK3 executes native `MOVE` into folder if overlay not yet targeted | Security / data safety | Critical | **Retired (M0 S-1/S-6 PASS)** | **P1 (Mitigated)** | Issue 0-pixel `WarpPointer(NONE, NONE, 0, 0, 0, 0, 0, 0)` (`pointer_nudge = true` by default) immediately after `MapWindow` so X server synthesizes `MotionNotify` at `(0,0)` and GTK3 retargets to the rejecting center hub in `1.24–2.80 ms` | Never disable `pointer_nudge` unless testing raw `MapNotify` behavior; see [`docs/spikes/S-1.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-1.md) |
| **R-32** | Under `passthrough = true`, user moves pointer outside wheel cluster over a Nemo folder while still holding `Shift` and releases mouse button expecting menu cancel → Nemo executes native `MOVE` into underlying folder | Usability / data safety | High | Medium | **P1** | Document trade-off in `ADR-016` and `PLATFORM_INTEGRATION.md`; support `input.passthrough = false` (capture-all mode where releasing outside the wheel safely cancels and a second `Shift` press toggles the wheel off); when `passthrough = true`, clearly dim/dismiss wheel on `XdndLeave` | Consider defaulting `input.passthrough` or adding visual indicator when pointer leaves wheel cluster; see [`docs/spikes/S-4.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-4.md) |
| **R-19** | Hover-intent/hysteresis tuning feels wrong → children collapse or linger, users frustrated | Usability (the product) | High | Medium | **P1** | Explicit rules (dwell + band + intent triangle + grace + hysteresis), scenario tests, sandbox UX sign-off (M1b), all timings configurable; fixed child `Bridge` hit-margin overlap with parent drawn ring (`WHEEL_ENGINE_DESIGN.md` §6.2) | Iterate defaults with recorded user trials; add per-user "slower/faster" preset |
| **R-06** | Overlay stays visible after a drag ends (missed Leave/Drop, source crash) | Reliability | High | Low (M0 S-6 PASS) | **P1** | Multiple independent end signals verified in S-6: `XdndLeave`, `XdndDrop`, XFixes owner change/destroy/client-close (`< 2 ms` disarm on `SIGKILL` of source), raw button release + 1 s watchdog, 120 s max session; dismiss force-unmaps after 500 ms; IPC `radivine dismiss` | Bug reports with `radivine doctor`; add more signals if a source misbehaves |
| **R-10** | Wayland cannot offer the drag-mode interaction | Compatibility | High | Certain | **P2** | Honest capability matrix; Invoke mode, Drop Shelf, DE extensions later; no security bypasses | Communicate clearly in README/first-run notification |
| **R-11** | Linux Mint/Cinnamon defaults shift to Wayland, shrinking the X11 audience over time | Strategic | High | Medium | **P2** | Keep engine/renderer/core platform-independent; Invoke mode works everywhere; plan Wayland backend (layer-shell first) after v0.1 | Prioritise the Wayland W-A spike and a W-B (Drop Shelf) prototype once v0.1 ships |
| **R-03** | `Shift` presses not observed during the drag | Technical | High | **Retired (M0 S-2 PASS)** | **Resolved** | Verified in S-2: XI2.4 raw key events (`371–547 µs` trigger-to-map) and `QueryPointer` polling fallback (`411 µs`) both work 100% under active GTK3 grab | See [`docs/spikes/S-2.md`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/spikes/S-2.md) |
| **R-23** | Scope creep (previews, settings, conversions) delays the wheel | Project | Medium | High | **P2** | Milestone gates; "wheel wins" rule; ADR required for new features | Cut peripheral features |

## 2. Other risks

| ID | Risk | Category | Sev. | Likelihood | Prio | Mitigation |
|----|------|----------|------|-----------|------|------------|
| R-02 | Payload prefetch refused before `XdndEnter` → menu can't be context-specific at first frame | Technical | Medium | **Retired (M0 S-3 PASS)** | Resolved | Verified in S-3: prefetch before `XdndEnter` succeeds in `1.18–2.18 ms` (both `XFixesTs` and `CurrentTime`), including 500 files (`1.31 ms`) and 3,000 files via `INCR` (`3.22 ms`) |
| R-04 | Users rely on Shift-drag = move in Nemo; Shift activation surprises them | Usability | Medium | Low (M0 S-2 PASS) | P3 | Verified in S-2: only a **fresh** press after drag start triggers (`held_at_arm` check); pre-held Shift preserved; configurable trigger key; toggle-off with second press |
| R-05 | Radivine causes a file to be moved/deleted | Security / data safety | Critical | **Retired (M0 S-6 PASS)** | Resolved | Verified in S-6: `XdndActionPrivate` maps to `GDK_ACTION_COPY` in `gdkdnd-x11.c`, never requests `DELETE`, and leaves `checksums.before` + `stat.before` 100% identical across all runs (with 0-px `WarpPointer` nudge preventing zero-motion underlying folder drop) |
| R-07 | Stuck pointer/keyboard grab in Invoke mode | Reliability | High | Low | P3 | Grabs bound to the overlay (released on unmap/disconnect); ungrab on every exit path; 60 s inactivity watchdog; never grab in drag mode |
| R-08 | Accidental activation (Shift for other reasons mid-drag) | Usability | Low | Medium | P4 | Gating on `XdndSelection` ownership + button state; toggle-off; configurable key |
| R-09 | Non-GTK3 sources (Qt, Chromium, Electron, GTK4) behave differently (shape ignored, timestamps) | Compatibility | Medium | Medium | P3 | S-7 informational; capture-all fallback when pass-through ignored; per-source quirks table |
| R-12 | CPU renderer / full-screen MIT-SHM upload too slow at 2×/4K | Performance | Medium | **Mitigated (M0 S-5 PASS)** | P3 | S-5 proved full `3840×1080` (`16.59 MB`) `shm::PutImage` stalls NVIDIA Xorg for `32.6 ms` at p95, whereas damage-rect upload (`800×600` cluster = `358 µs` p50 / `953 µs` p95; `300×300` wheel = `42 µs` p50 / `48 µs` p95) meets `< 1 ms` target; **damage-rect upload is mandatory** |
| R-13 | Cinnamon applies effects to the overlay, or a monitor-sized override-redirect window causes desktop side effects | Compatibility | Low | **Retired (M0 S-5 PASS)** | Resolved | Verified in S-5: `_NET_WM_WINDOW_TYPE_DND` + `_NET_WM_BYPASS_COMPOSITOR=2` maps in `169 µs` with zero Muffin animations, shadows, focus changes, or client-list entries |
| R-14 | No compositing manager (other WMs) → no translucency | Compatibility | Low | Low | P5 | Shaped opaque fallback (M6) |
| R-15 | Network/hung filesystems block the UI | Performance / reliability | Medium | Medium | P3 | No FS access on UI thread; visibility from BasicContext only; enrichment deadline 1.5 s; FIFO-safe open |
| R-16 | Privacy concern: daemon observes keystrokes | Security / trust | Medium | Low | P4 | Raw key events selected only while a drag is armed; only trigger keycodes inspected; never logged; documented privacy statement |
| R-17 | Command injection through custom actions or filenames | Security | High | Low | P3 | argv-only execution, whole-element placeholders, no shell unless user writes it; filenames passed as `OsString` arguments |
| R-18 | DPI/scale mis-detected (fractional scaling, mixed monitors) | Compatibility | Low | Medium | P4 | `Xft.dpi` + config override; per-monitor scaling out of scope on X11 |
| R-20 | Overlapping wheels on small screens / deep menus | Usability | Low | Medium | P4 | Rotation/shrink placement, depth ≤ 4 default, overflow "More…", ancestors dimmed |
| R-21 | Overlay not accessible to screen readers | Accessibility | Medium | Certain | P3 | Keyboard-complete Invoke mode; caption text; future AccessKit/AT-SPI or accessible list window |
| R-22 | Dependency API churn (cosmic-text, x11rb, egui) | Maintenance | Low | Medium | P4 | Pin versions in `[workspace.dependencies]`; wrap third-party types inside our crates; Dependabot/Renovate |
| R-24 | Branding has no transparent/vector variants for dark UI | Design | Low | Certain | P4 | Use transparent `logo-icon.png`/`app-icon.png` where needed; owner to provide SVG/transparent variants; never modify existing files |
| R-25 | Implementer changes architecture silently | Project | Medium | Medium | P3 | GEMINI_HANDOFF rules; ADR requirement; CI dependency-graph check; milestone reviews |
| R-26 | Nemo/GTK version drift (e.g. future Nemo on GTK4) | Compatibility | Medium | Low | P4 | Re-run S-1 on major updates; E2E with GTK3 source; investigate GTK4 path in S-7 |
| R-27 | No keyboard navigation in drag mode | Usability / accessibility | Medium | Certain | P3 | Documented; Invoke mode is the keyboard path |
| R-28 | MIT-SHM unavailable (remote X, SSH forwarding) | Compatibility | Low | Low | P5 | Core `PutImage` fallback |
| R-29 | Licensing issues with bundled font/icons | Legal | Low | Low | P5 | Inter (OFL 1.1), Lucide (ISC); licence texts in `assets/third_party/`; `cargo deny` for crates |
| R-30 | `memmap`-style crashes on truncated files in the viewer | Reliability | Medium | Low | P4 | Viewer uses `pread`, no mmap; separate process |

## 3. Risk review cadence

- Re-assess at the end of every milestone; update likelihoods with spike/test evidence.
- Any risk whose contingency is triggered gets an ADR in [DECISIONS.md](DECISIONS.md).
