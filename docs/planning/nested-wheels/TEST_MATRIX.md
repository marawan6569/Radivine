# Nested wheels verification matrix

Status: **Proposed acceptance tests, not feature test results.**
Baseline `e6a342e`; both layouts share the same fixture menu/action registry.
Acceptance IDs below are stable handoff identifiers. Future reports must mark
PASS / FAIL / NOT RUN / BLOCKED with evidence, never silently count skips as PASS.

## T1. Configuration and compatibility

| ID | Scenario and required assertion | Milestone |
|---|---|---|
| T-CFG1 | Missing file, missing `[wheel]`, and missing `layout` all map to Branching; existing numeric/default values unchanged. | N1 |
| T-CFG2 | Exact strings `branching`, `concentric` round-trip. Unknown, empty, mixed-case, whitespace-padded string and TOML integer/bool/array/table/datetime for layout warn and reset only layout. Preserve nondefault trigger, passthrough, preview and unknown/jobs values. | N1 |
| T-CFG3 | Strict validation rejects all invalid values; valid candidate preserves `version = 1`. Broken TOML syntax retains existing startup fallback, distinctly reported. | N1 |
| T-CFG4 | `to_wheel_config` maps enum without serde/runtime dependencies in wheel; app defaults and pure engine defaults tested separately. | N1 |
| T-CFG5 | Save selector using `apply_app_and_jobs_config_to_toml_doc` and atomic write in tempdir: comments, unknown keys, mode bits, `[jobs]`, custom settings retained; `actions.toml` and `convert.toml` bytes unchanged. Simulated write failure leaves original parseable/intact. | N4 |
| T-CFG6 | External edit gives existing conflict prompt; reload and backup-overwrite paths retain expected content and backup. Restore Defaults sets Branching. | N4 |
| T-CFG7 | Save with idle/offline/active daemon, failed IPC and repeated alternating saves. Disk value reflects last save; running session retains initial layout/config/registry; next session uses latest acknowledged defaults. Correct pending/error banner. | N4 |
| T-CFG8 | Headless Wheel UI exposes exact labels/help; irrelevant child controls disabled but retained; config save during a conversion batch neither cancels jobs nor changes presets. | N4 |

## T2. Pure geometry and input scenarios

| ID | Required cases / assertions |
|---|---|
| T-GEO1 | Counts 1..12 for every open level; equal spans, clockwise order, item zero at top, full-circle coverage minus separators; n=1 has no seam; n=0 placeholder no targets. |
| T-GEO2 | All centers equal; exact annular radii from I1; gaps 6 lp; no ancestor covered by a child hub; child placement independent of parent angle. |
| T-GEO3 | Viewports at minimum fit, ±0.01 lp; corners/edges/interior, negative viewport origin, scales 1/1.25/1.5/2/4. Fit or explicit pre-session fallback, never clipped actionable geometry. |
| T-GEO4 | Use shipped app root 40/128: four rings R=326, required 668×742; six R=458, required 932×1006. 1366×768@1 fits; 1280×720@1 falls back; 3840×2160@2 fits; 1920×1080@2 falls back. |
| T-GEO5 | Minimum target rectangles all lie within sector interiors. Small/nonfinite/invalid radii, extreme gap/hysteresis config and zero-sized viewport fail safely; no NaN/panic/division by zero. |
| T-GEO6 | Radial and angular endpoints ±epsilon, angle wrap 0/2π, full annulus, separators, outer padding. Every point has exactly one strict zone. Hub is tested once. Strict hits are independent of current hover. |
| T-GEO7 | Content boxes and marker bounds inside their material/reservation, long Unicode/missing-icon fixtures. No rendering outside damage; approach/caption never action hits. |
| T-NAV1 | Dwell at deadline−1/deadline/deadline+1; band threshold ±epsilon and ≥2 lp outward criterion; Dwell/Band/DwellAndBand/Click; zero durations; exactly one child-open event. |
| T-NAV2 | Follow root→child→grandchild→level4, and six levels in engine fixture; same submenu `ItemPath` chain, all ancestors visible. |
| T-NAV3 | Parent→gap→any child angle, including diagonal through sibling. Existing child survives; no Commit from movement/ticks; raw gap rejects. |
| T-NAV4 | Ancestor sibling pending at entry; deadline not extended by repeated same position; adopt at grace, delay child until both dwell/grace conditions hold; two-sample outward shortcut; stale timers cannot act on a replacement path. |
| T-NAV5 | Move inward across several levels, open-parent sector, quick hub crossing, hub deadline−1/+0; no collapse on mere radial crossing, hub timeout returns to root. |
| T-NAV6 | Outside/outer-margin/separator wandering keeps path, clears target; re-entry restores strict targets; no default action leaks through protected parent hover. |
| T-NAV7 | Drag placeholder→menu while pointer sits where a leaf appears: duplicate positions and ticks reject; ≥2 lp fresh move arms. Child opening cannot open/select grandchildren in same input. |
| T-NAV8 | Displaced root: zero-motion rejects, approach across visible sectors rejects, entering hub releases guard, then leaf movement works. Keyboard can enter directly; returning pointer still requires hub when applicable. |
| T-NAV9 | All Invoke keys, wrap, disabled focus reason, Back at root/no-op, digit bounds, keyboard-only operation without pointer slop, pointer takeover threshold, press+release/hub-marking/release modes. |
| T-NAV10 | Rapid alternating sibling movements at 1/8/16 ms intervals for 10,000 steps; open chain valid, ghost count bounded, no stale accept, no panic. |
| T-NAV11 | Interrupt open/close/dismiss every 10 ms through each tween. Retarget opacity continuous; fixed radii; one Finished; removed/retracting rings never hit. |
| T-NAV12 | Reduced motion: scale=1, pop=0, no radial animation, all fades ≤60 ms; existing Branching reduced-motion behavior preserved. |

Concrete deterministic trace (use an app-default wheel config, Concentric,
viewport 1920×1080, C=(960,540); fixture root item 0 is submenu A with two leaves,
item 1 is leaf B, no default child):

1. Summon t=0, root 40..128, depth capacity 4. At t=10 move to C+(0,-80);
   hover A; radius below band 97.2, deadline 120. At t=119 child absent; t=120
   child 134..194 appears and path is `[[],[0]]`; no action.
2. t=125 move to radius 131 at top: gap, Reject, child retained. t=135 move to
   child item 0 center (radius 167): fresh motion arms `[0,0]`. Drop now selects
   exactly `[0,0]`; a separate run dropping at 131 selects nothing.
3. Separate run t=140 move to a different ancestor sector at radius 80: pending
   adoption deadline 320 (180 ms grace), Reject. Same sample at t=319 does not
   extend deadline. t=320 adopts B/closes child and enables direct B release.
4. Another run re-enters existing child at t=200 before deadline: pending
   cancelled, old child retained. Another enters hub t=200: t=349 descendants
   remain, t=350 root only. No tick in these runs emits Commit.

Use geometry query helpers for non-example trajectories, not Branching hardcoded
coordinates. Parameterize equivalent action routes across layouts; compare action
IDs, not pointer coordinates or layout-specific hover event timing.

Property tests use existing proptest dependency. Generate valid trees, viewports,
counts, points and monotonic event streams with enablement changes. Assert bounded
storage, valid path prefixes, no commits to disabled nodes/ancestors, no commits
from timers, targets inside visible material, no unexpected fallback after map,
finite geometry and future/absent deadlines after processing. Include reproducible
seeds and retain minimized failures as named regression cases.

## T3. Action safety, context and X11

| ID | Required outcome |
|---|---|
| T-SAFE1 | Release with no movement, in hub, radial gap, separator/boundary, outer margin, approach, outside, closed/retracting ring and pending sibling: Reject/no action. Repeat with destructive **fake** action registered to detect accidental dispatch. |
| T-SAFE2 | Disabled leaf, disabled ancestor, Pending enrichment, disabled→enabled and enabled→disabled immediately before drop. Eligibility and status update immediately; app rechecks before dispatch. |
| T-SAFE3 | Parent with default leaf: direct eligible parent drop selects that immediate leaf only; gap/protected transition rejects; disabled/nonleaf default rejects; Invoke submenu activation opens it. |
| T-SAFE4 | Invoke mismatched press/release, path slot replaced while button held, geometry generation changes: no unintended leaf commit. Disabled/gap clicks no-op, outside/hub/Escape cancel. |
| T-SAFE5 | Same leaf/action ID/context produces identical dispatcher request in both layouts; no invocation from hover/dwell/open/collapse, even during rapid sweeps. |
| T-SAFE6 | Layout save/reload during Summoning, Active, LeftGrace, pending payload and dismissal: no overlay hide/reprepare, no lost capture, no ID remapping, next activation takes new preference. |
| T-SAFE7 | Prefetch refused→provisional menu→drop→real menu reordered, action invisible/disabled/absent. Captured ID is executed only if still valid; never the new action at old path; invalid selection finishes false. |
| T-SAFE8 | Nested display/scale/compositor-change guard: settings deferred, monitor/compositor change rejects until source end without layout-driven handoff. Connection loss dispatches nothing; recovery on next session. |
| T-MENU1 | Preview/Copy/Properties/Custom/Convert IDs, order, labels, descriptions and visibility equal for text, image, mixed, folder, unreadable, remote and large selections. Reuse BasicContext and registry fixtures. |
| T-MENU2 | More… wrapping and max-depth disablement identical to baseline; engine sanitize does not remap app ActionIndex. Test counts 0,1,7,8,9,12,13 and limit-depth menus. |
| T-MENU3 | Enablement updates preserve geometry and item positions. Disabled parent makes all descendants ineligible; caption reason updates with current injected time. |
| T-MENU4 | Action config reload while visible: old session uses old action object/index; next session new action/menu. Existing jobs retain old request/settings snapshot. |
| T-MENU5 | Convert→Image→target and Advanced… in Drag/Invoke, same preset/arguments/output policy; Copy/Preview/Properties unchanged, custom whole-argument substitutions unchanged. |
| T-X111 | Real Xvfb window: one overlay, unchanged target window ID as rings open/close; no new window per ring; no grab in Drag. |
| T-X112 | Safe default full ShapeInput on the chosen composited monitor from map through drop. Reserved-disc pass-through variant has no holes/shrink on collapse; anchor/corridor included; caption excluded. |
| T-X113 | Fake source verifies bit-exact Private/None status/finish for every zone. No MOVE/DELETE. Reject-before-render ordering, unsolicited status on timer/enablement changes, drop-time recheck. |
| T-X114 | GTK-style prefetch/INCR/refusal, zero-position trigger/drop, stale source, source destruction, re-entry, timeout, no stuck grabs or orphan overlay. |
| T-X115 | Every corner/edge on monitors with nonzero/negative physical origins; coordinate conversion at fractional scales; no ring spans two monitor windows. |
| T-X116 | Invoke grabs/retries/ungrabs and keyboard routing unchanged. Select/cancel in both layouts repeatedly; other windows receive input after session ends. |
| T-X117 | Requested nested with no compositor chooses Branching before mapping; requested/effective status accurate; legacy opaque fallback unchanged, no claim of full-screen cancellation. |
| T-X118 | In a fixture folder under the captured region, release on all reject zones plus reload/collapse: source checksum, inode, mtime, size **and path** unchanged; native target receives no drop. Explicit toggle handoff and opt-in pass-through are separate expected native-drop tests. |

## T4. Rendering and regression

| ID | Acceptance |
|---|---|
| T-REN1 | 1/2/3/4/6-level scenes, same center, single hub, no disc shadow over ancestors, no stems between rings, path markers and breadcrumb correct. |
| T-REN2 | Stable screenshots at t=0/30/60/130/250 and interruption points; dark/light; 1/1.25/1.5/2×; n=1,2,3,8,12; long Latin/Arabic/Unicode labels, no icon and unknown icon. No overlapping action content or unreadable auto-shrunken fonts. |
| T-REN3 | Full fresh raster equals incremental raster after hover/path/disabled/breadcrumb/geometry-kind changes, appearance/retraction/replacement and caption removal. Includes n=1/2 sectors where sampled-angle bounds can under-cover. |
| T-REN4 | Every changed pixel lies inside returned damage; rectangles within framebuffer, count ≤4; identical scene produces zero damage. Branching hover retains existing <30% framebuffer damage gate. |
| T-REN5 | Goldens use pinned bundled fonts, exact viewport/scale/time and renderer dependency versions. Same environment byte comparison; portability changes require reviewed pixel diff, never blanket tolerance or automatic rebaseline. |
| T-REN6 | New kind/guide/content fields included in hashes. Rings removed from hits immediately, pixels erased by final fade; no ghost behind replacement content. |
| T-REN7 | Sandbox and Control Center exports go to new artifact directories. Existing screenshot files and all protected assets remain untouched during development. |
| T-REG1 | Original wheel unit/scenario/property/allocation tests remain unchanged and pass under default and explicit Branching; baseline event+geometry traces identical. |
| T-REG2 | Branching renderer output matches baseline fixture pixels for all existing representative scenes; no changed timing/default/corridor/bridge/pop behavior. |
| T-REG3 | Full M1a–M6, C1–C5, v0.1.1 and existing clipboard/font regressions pass. Distinguish display-gated early returns, ignored tests and unavailable service tests from executed coverage. |

## T5. Existing suite map and execution

| Historical acceptance | Actual baseline implementation |
|---|---|
| M1a | `crates/radivine-wheel/tests/{unit_tests,scenarios,properties,alloc_test}.rs`, `benches/wheel_bench.rs` |
| M1b | `crates/radivine-render/tests/render_tests.rs`, lib cache test, `benches/render_bench.rs`, wheel-sandbox exports |
| M2/M3 | `apps/radivine/tests/{m2_acceptance,m3_drag_acceptance}.rs`, `crates/radivine-platform/tests/x11_tests.rs` |
| M4/M5 | `apps/radivine/tests/{m4_actions_acceptance,m5_preview_acceptance}.rs`, core/actions/viewer crate tests |
| M6 | `apps/radivine/tests/{m6_soak_test,m6_fuzz_suite}.rs` (long runs ignored by default) |
| C1 | convert/convert-image/webp crate tests, `crates/radivine-convert-image/tests/corpus_regression.rs`, `tools/convert-cli`, `tools/convert-corpus`, `fuzz/fuzz_targets/` |
| C2 | `apps/radivine-worker/tests/c2_acceptance.rs`, jobs/worker unit tests |
| C3 | `apps/radivine/tests/c3_convert_acceptance.rs`, actions conversion tests |
| C4 | `apps/radivine-control/tests/c4_acceptance.rs` |
| C5 | `apps/radivine-control/tests/{c5_benchmarks,c5_soak_test}.rs`, existing fuzz targets and package verification described in `docs/test-reports/C5.md` |
| v0.1.1 / fixes | `apps/radivine-control/tests/{v0_1_1_acceptance,issue_1_clipboard,arabic_font_fallback}.rs`, `apps/radivine/tests/v0_1_1_login_autostart_regression.rs` |

Headless release gate, in a temporary XDG/runtime environment with host session
D-Bus unavailable (do not merely blank DISPLAY and assume service isolation):

```bash
cargo fmt --all --check
./scripts/check-dep-graph.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
DISPLAY="" cargo test --workspace -- --test-threads=1
```

The environment wrapper must allocate owned temporary config/state/runtime dirs,
set all `RADIVINE_*` binary/socket/db paths to test instances, disable notifications
and systemd worker launch, and point `DBUS_SESSION_BUS_ADDRESS` at the test bus or
an unavailable endpoint. Never change HOME/CODEX_HOME. Some C2 tests explicitly
probe `systemd-run --user`; execute their service branch only in a disposable VM
with its own user service manager. Record that branch NOT RUN in headless checks.

Actual X11 gating is `radivine_platform::is_isolated_test_display`: DISPLAY must
be `:N` with N>1, the X lock PID must be an Xvfb process. There is no
`x11-integration` Cargo feature in the daemon. A future authorized test command is:

```bash
xvfb-run -a -s "-screen 0 1920x1080x24 +extension COMPOSITE +extension RANDR" \
  env RADIVINE_FORCE_ARGB=1 cargo test -p radivine-platform -p radivine \
  -- --test-threads=1 --nocapture
```

Run in the same temporary environment, build binaries first, install required
test tools before execution. Force-ARGB exercises the alpha path but is **not**
proof of real compositor behavior; use a test compositor where available and
the live desktop checklist below. Check logs for executed cases. The documented
`tests/e2e-x11/run_e2e.sh` does not exist at baseline; do not claim to have run it.
Existing fake-source M3 tests are usable; a new GTK harness, if necessary, must
be explicitly named as a new test artifact, not presumed available.

At N5 run M6 ignored soak with `RADIVINE_SOAK_SECONDS=3605`, M6 ignored fuzz with
`RADIVINE_FUZZ_SECONDS=600`, and C5 soak with `RADIVINE_C5_SOAK_SECS=3605` in release
mode. Add mixed-layout/reload paths to the wheel soak, not codec logic. C5 default
soak is only six seconds and is not a substitute for the one-hour gate. Preserve
existing C1–C5 fuzz/regression expectations; no changes to codec budgets.

## T6. Performance and memory gates

All new thresholds are **proposed release gates requiring owner plan acceptance**,
not measured claims. Historical BENCHMARKS provides baselines; rerun the same
workloads before/after on the reference machine and uncontended build. Use at
least 3 warmups, 30 independent measured trials for comparisons, and ≥1,000 frame
samples for p95. Report actual sample percentiles; Criterion's confidence interval
of an estimate is not a frame-latency p95, despite older table headings.

| ID | Metric | Gate |
|---|---|---|
| T-PERF1 | Branching engine, renderer and trigger regression | No reproducible slowdown >5% vs baseline with 95% confidence, and all existing absolute gates retained. Smaller/noisy changes reported, not called improvements. No new warm allocation; incremental active state ≤64 KiB. |
| T-PERF2 | Nested engine handle+tick+scene, ≤6×12 items | p95 <50 µs; no per-input/frame heap allocation after menu preparation with reserved output events. Initialization/preparation allocations measured separately. |
| T-PERF3 | Nested software raster, root/3/4/6 levels with animated highlights/caption/path changes | 1920×1080@1: p50 <2.5 ms, p95 <4 ms. 3840×2160@2: p50 <4 ms, p95 <6 ms (retain M1b's tighter raster gate). |
| T-PERF4 | Nested render + X11 present; trigger; status; drop | Frame p95 <4 ms@1 / <8 ms@2; no missed 60 Hz deadlines in a 30 s ordinary navigation trace. Trigger→first present p95 <50 ms. Position→Status p95 <2 ms. Cached drop→dispatch p95 <30 ms. Record Quick Convert job acceptance separately under unchanged C5 budgets. |
| T-PERF5 | Memory and idle | New layout incremental engine state ≤64 KiB excluding original tree/strings; no additional framebuffer. Daemon idle after buffer release <25 MiB and <+1 MiB vs same-build Branching; active nested <+8 MiB vs Branching at same surface; baseline ultrawide active <45 MiB remains a gate. Idle CPU <0.1%, no new periodic wakeups. |
| T-PERF6 | Stability | ≥1 h / ≥2,000 mixed sessions; RSS growth after warmup ≤2 MiB, zero FD/process leaks, zero unintended actions/source changes. Existing C5 worker soak memory gate remains unchanged. |

Benchmark full worst-case ring opening and replacement, not just settled scenes.
Measure renderer allocations after warming each fixture's text/icons separately;
new nested warm rendering must allocate zero. Keep cold-cache results visible.
Damaged area may legitimately exceed 30% for a large multi-ring transition; the
30% rule applies to the existing Branching single-hover case, while nested is
governed by correctness and frame budgets. Do not hide over-budget rings behind
empty-damage frames. If a gate fails, optimize within the chosen architecture or
stop for a documented owner decision; no silent budget increase or dropped test.

## T7. Mandatory owner desktop verification — Linux Mint Cinnamon X11

This checklist is for N5, not authorized execution during planning. Use the
existing isolated Development Environment. `dev-start.sh --stop-installed`,
`--enable-nemo`, `dev-nemo.sh`, and `dev-stop.sh --all` modify services/Nemo state;
the owner must explicitly authorize those operational effects. Inspect
`docs/DEVELOPMENT.md` and actual scripts before executing. Do not share the
installed daemon's display listener with a second dev daemon.

Record date, commit, kernel, Cinnamon/Muffin/Nemo/Xorg/driver versions, monitor
origins/sizes, scale, compositor, config, requested/effective layout and video or
screenshots. Copy disposable text/image/mixed files into a test directory with
an empty destination folder underneath each tested reject zone. Hash and record
path/size/inode/mtime before and after; never use valuable original files.

- [ ] Branching before/after: normal Drag+Shift, Preview/Copy/Properties/Custom,
  Convert→Image→target; same geometry/timings/leaf results as v0.2.0.
- [ ] Select Concentric in Control Center, save, verify next activation/status;
  repeated switching back/forth while a wheel is visible leaves that wheel
  intact. A running conversion batch continues.
- [ ] Fresh left/right Shift during Nemo icon/list/compact view drag; multi-file,
  Unicode/Arabic/spaces/percent filenames; pre-held Shift and normal drag remain
  native. No-motion release over a destination folder must not move files.
- [ ] Navigate 2/3/4-level menus using pointer only; traverse every child sector
  around the full circumference, move inward, change root branch, pause in gaps,
  reverse quickly, sweep 20 times, open/close under 60/120 Hz refresh.
- [ ] Release on disabled leaf, checking item, hub, separator, radial boundary,
  ring gap, outside cluster **inside captured monitor**, approach corridor and
  protected sibling: no action, source remains at original path, no native drop.
- [ ] Repeat those releases during ring appearance/retraction and immediately
  after a Control Center layout save or action availability change.
- [ ] Invoke via isolated CLI/Nemo dev action: click, hub marking, keyboard-only
  Tab/arrows/Enter/Space/digits/Backspace/Escape, disabled explanations, right-click
  cancellation; focus/grabs restored afterward.
- [ ] All four corners and edge midpoints; displaced-origin guide, required hub
  approach, caption readable. Default four rings on 1366×768; explicit fallback
  on 1280×720; shallow menu fallback is accurately explained.
- [ ] Actual multiple monitors including negative origin, unequal physical sizes
  and mixed DPI settings; activation selects pointer monitor; no cross-monitor
  ring movement; measure/document global Xft scale limitation. Native handoff
  beyond the overlay is explicitly tested separately from safe cancellation.
- [ ] Scale 1, 1.25/1.5 override and 2; dark/light; reduced motion; no clipped
  actionable sectors, obscured ancestors, label overlap or residual ghost pixels.
- [ ] Explicit pass-through with disposable files: reserved neutral disc does not
  shrink on collapse; outside that disc retains documented native handoff risk.
  Restore false and verify full-monitor safe capture. No selector changes it.
- [ ] With separately authorized display changes, change scale/topology/compositor
  while nested is open: no mixed coordinates or action dispatch; rejection guard
  drains safely; new session recovers. Do not equate server/window loss with a
  guaranteed safe native drag cancellation.
- [ ] 50 repeated sessions/toggles/cancels, source disappearance, missing payload;
  no stuck cursor/grab, leaked overlay or lingering process. Review measured frame
  trace, idle CPU/RSS and warm latency, not visual impression alone.
- [ ] C1–C5 smoke: Quick Convert output/no-overwrite/skip behavior, Advanced Convert,
  presets, Jobs progress/cancel/retry and persistence unchanged; clipboard and
  preview still work. All intended outputs separated from source files.
- [ ] Owner explicitly accepts full-ring model, fallback frequency, edge approach,
  label density, timings and visuals. Record acceptance text and commit, or
  enumerate required changes; keep ADR Proposed until acceptance.

## T8. Planning-session verification (2026-10-25)

This is a documentation-only investigation. At the inspected v0.2.0 source state:

- `cargo fmt --all --check`: PASS.
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS.
- `cargo test -p radivine-wheel -p radivine-render`: PASS, 30 tests total
  (26 wheel, 4 renderer), zero failures; doc tests contain zero tests.
- Planning checks: PASS for all seven deliverables, relative links/anchors,
  code fences, explicit acceptance-ID references, the capacity table arithmetic,
  representative minimum target cores, and changes restricted to this directory.
- Full workspace execution, desktop/Xvfb tests, services, benchmarks, long soaks,
  package builds and feature acceptance: **NOT RUN in this planning task**.
  The full suite includes service-lifecycle/X11 probes and is unnecessary for
  these Markdown-only changes; the task explicitly prohibits service/Nemo/user
  state changes. No feature exists yet to accept or benchmark.

Planning document link/anchor, arithmetic, diff-scope and consistency checks are
recorded in the final planning handoff. Historical reports are evidence of the
released baseline, not proof that the proposed feature passes these new gates.
