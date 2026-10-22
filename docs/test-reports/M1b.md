# Milestone M1b Test & Verification Report (`radivine-render` & `tools/wheel-sandbox`)

- **Date:** 2026-10-22
- **Crates Verified:** `radivine-render` (`crates/radivine-render`), `wheel-sandbox` (`tools/wheel-sandbox`)
- **Environment:** Linux Mint 22.x (Cinnamon, X11 `DISPLAY=:1`), `Linux 6.8.0-147-generic x86_64`, `rustc 1.95.0 (59807616e 2026-04-14)`
- **Overall Status:** **PASS (All Acceptance Criteria `AC-1b.1` – `AC-1b.6` Verified)**

---

## 1. Acceptance Criteria Verification Summary

| Criterion | Requirement (`IMPLEMENTATION_PLAN.md` §4 / M1b) | Verification Method & Evidence | Status |
|---|---|---|---|
| **AC-1b.1** | `Renderer::new` succeeds with `AssetSource::Embedded` and `AssetSource::Dir` for both `Theme::dark()` and `Theme::light()` at `1.0×` and `2.0×` scale factors; `#![forbid(unsafe_code)]` enforced. | Verified by `test_ac_1b_1_renderer_init_embedded_and_dir_dark_and_light` in `crates/radivine-render/tests/render_tests.rs`. Embedded `Inter-Variable.ttf` (`assets/fonts/`) and 17 Lucide SVGs (`assets/wheel-icons/`) load cleanly with third-party licenses in `assets/third_party/`. | **PASS** |
| **AC-1b.2** | Deterministic software rendering of 1-level, 2-level, and 3-level wheel clusters, quadratic Bézier stems, bridge arcs, submenu rim node indicators, disabled item styling, and caption pill (`disabled_reason` in muted amber). | Verified by `test_ac_1b_2_snapshot_renders_1_2_3_levels_and_disabled_caption` (including bit-exact determinism assertion across two independent `Renderer` instances) and visual screenshots `01`–`07` in `docs/test-reports/screenshots/`. | **PASS** |
| **AC-1b.3** | Incremental damage tracking returns `<= 4` physical-pixel rectangles per frame (`0` when `Scene.generation` is unchanged) and damages `< 30%` of the frame area on a single-segment highlight transition. | Verified by `test_ac_1b_3_damage_single_segment_highlight_under_30_percent` in `crates/radivine-render/tests/render_tests.rs` (`0` damage rects on unchanged scene; `<= 4` rects covering `< 6%` of a `1920 × 1080` frame on segment hover). | **PASS** |
| **AC-1b.4** | Interactive `tools/wheel-sandbox` (`winit` + `softbuffer`) runs on X11 and demonstrates the required menu (`File Preview` → `Full Preview` / `Head` / `Tail`, `Copy`, `Properties`, `Custom Actions` → `Open in Terminal` / `Scripts` → `Compute SHA-256` / `Make Executable` / disabled `Sign with GPG`). | Executed both in deterministic screenshot export mode (`--export-screenshots docs/test-reports/screenshots`) and live on `DISPLAY=:1` (`--auto-quit-ms 1000`). Supports simulated Drag mode (`Hold LMB + Shift` or `G`) and Pointer/Invoke mode (`Right-Click` / `Space` / `I`), plus keyboard navigation (`Tab`, `Arrows`, `Enter`, `Backspace`, `1`–`9`, `Esc`). | **PASS** |
| **AC-1b.5** | Visual inspection of root wheel, multi-level child wheels, connecting stems + bridge sectors, screen-edge boundary adaptation, dark/light themes, debug overlay (hit zones, outer fast-open band, safe navigation corridor / intent triangle, damage rectangles), and visible action selection feedback. | Verified via 7 exported composite PNG screenshots in `docs/test-reports/screenshots/` and console/on-screen toast logging of selected action ID and menu breadcrumb path. | **PASS** |
| **AC-1b.6** | `criterion` benchmarks for a warm 3-level cluster meet frame-time budgets (`1920 × 1080 @ 1×`: p50 `< 2.5 ms`, p95 `< 4.0 ms`; `3840 × 2160 @ 2×`: p50 `< 4.0 ms`, p95 `< 6.0 ms`) and are recorded in `docs/BENCHMARKS.md`. | Measured **`211.88 µs` (`0.212 ms`)** at `1920 × 1080 @ 1×` (11.7× under budget) and **`638.58 µs` (`0.639 ms`)** at `3840 × 2160 @ 2×` (6.2× under budget). Recorded in `docs/BENCHMARKS.md`. | **PASS** |

---

## 2. Visual Verification Screenshots (`docs/test-reports/screenshots/`)

Generated deterministically by `cargo run -p wheel-sandbox -- --export-screenshots docs/test-reports/screenshots`:

1. `docs/test-reports/screenshots/01-root-wheel-dark.png`:
   - 4-item root wheel (`File Preview`, `Copy`, `Properties`, `Custom Actions`) in Dark theme (`#0A1A16` ring, `#044C3C` vine-green hub with file icon + emerald `3` count badge, `#00A87C` leaf-green highlight + `+4 lp` radial pop on `Copy`, submenu rim node indicators on `File Preview` and `Custom Actions`, and caption pill below).
2. `docs/test-reports/screenshots/02-child-wheel-file-preview.png`:
   - `File Preview` child wheel (`Full Preview`, `Head`, `Tail`) open above the dimmed parent wheel (`0.72` ancestor opacity), connected via the quadratic Bézier vine stem and 70° neutral bridge arc, with `Full Preview` hovered and caption placed above the child wheel.
3. `docs/test-reports/screenshots/03-three-level-nested-scripts.png`:
   - 3-level nested hierarchy simultaneously visible (`Root` → `Custom Actions` → `Scripts` → `Compute SHA-256` hovered), showing progressive outer-radius scaling (`116 lp` → `96 lp` → `84 lp`), dual connecting stems, disabled item styling (`Sign with GPG`), and caption pill below the depth-3 wheel.
4. `docs/test-reports/screenshots/04-screen-edge-adaptation.png`:
   - Summoned near the top-right screen corner (`(1185, 125)` on `1280 × 820`), demonstrating root clamping and angularly shifted child placement (`Placement` fallback) with a curved Bézier stem and left-aligned caption pill so all elements remain inside the viewport.
5. `docs/test-reports/screenshots/05-light-theme-and-disabled-item.png`:
   - Light theme (`Theme::light()`) with `Custom Actions` open and hovering the disabled `Sign with GPG` item, showing the muted amber `disabled_reason` (`No GPG signing key configured`) in the caption pill and `Drop Accept: NO` in the HUD.
6. `docs/test-reports/screenshots/06-debug-overlay-corridors.png`:
   - Debug overlay (`D` / `F2`) visualizing the XShape input-region discs and parent-child corridor polygon (cyan), outer fast-open band `r_in + 0.65*(r_out - r_in)` (emerald), hub dead-zone `r_in` (rose), hover-intent tangent cone (amber), and incremental damage rectangle (magenta).
7. `docs/test-reports/screenshots/07-action-selected-toast.png`:
   - Immediately after committing `Full Preview` (`id="radivine.preview.full"`, `path=[0, 0]`), showing the `Committing` flash on the selected segment and the top-right **"Action Executed"** confirmation toast banner with Action ID and `File Preview › Full Preview` menu path.
