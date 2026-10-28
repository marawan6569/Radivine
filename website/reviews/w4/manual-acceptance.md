# W4.5 manual acceptance package

Prepared 2026-10-28 against website `6b1f256` and Radivine **v0.3.0**, source `1aa009a08bab66646898016f1ba953051eac46f3`. W4 remains **NO-GO**. This package contains instructions, not new desktop/device observations. W5 is not authorized.

References: [plan](../../../docs/website-plan.md), [W3 fidelity contract](../../../docs/website/W3-demo.md), [W3 report](../../../docs/test-reports/W3.md), [W4 readiness report](../../../docs/test-reports/W4.md), [native media handoff](../../../docs/website/W2-media.md).

## Status and evidence rules

- **PASS**: a named human completed the specified check, with observations and evidence.
- **FAIL**: observed behavior violates the stated criterion; record reproduction and severity.
- **BLOCKED**: attempted but prevented by a named prerequisite; record the attempt and remedy.
- **NOT TESTED**: no human observation recorded. This is every human check's initial status.

Record native and browser results separately, and one result per environment/theme/layout where required. A passing automated test never changes a human status. Do not infer millisecond timing or pixel identity from a video. Do not use xdotool, scripted pointer movement, or remote desktop automation.

Existing automated evidence only: W4 recorded 82 Chromium tests at each base, 29 demo unit tests including six Rust differential traces, and 40 axe route/state/theme audits without violations. These do not establish screen-reader speech, actual zoom, touch, Safari or desktop fidelity. Existing media are browser captures; historical native screenshots and N3 sandbox exports do not establish this acceptance.

## 1. Prepare the Linux desktop and browser

1. Use Linux Mint Cinnamon **X11**, preferably the primary recorded Mint 22.3/Nemo setup. Record actual versions rather than relabeling another system. Run these read-only commands in your graphical user session:

   ```sh
   radivine --version
   radivine doctor
   printf 'Session: %s\n' "$XDG_SESSION_TYPE"
   cinnamon --version
   nemo --version
   ```

   Require v0.3.0 and X11 for the baseline. Capture output, display resolution/scaling, compositor state, and Control Center Wheel settings. Record deviations as BLOCKED for baseline comparison, not website failures. Use the default menu and application wheel defaults; record existing customizations before changing settings. Keep `input.passthrough=false`. Start the daemon from Control Center → Dashboard → Start Daemon if needed; do not use sudo or a system service. Restore your previous settings after testing.

2. Create disposable neutral samples in a terminal:

   ```sh
   w45_samples=$(mktemp -d "${TMPDIR:-/tmp}/radivine-w45.XXXXXX")
   printf 'Field notes\nFern beside the path\nEnd of sample\n' > "$w45_samples/field-notes.txt"
   printf 'Review sample\n' > "$w45_samples/todo.txt"
   cp "$w45_samples/field-notes.txt" "$w45_samples/inspection.txt"
   printf '%s\n' "$w45_samples"
   ```

   In an existing image editor, save a small, static, non-sensitive PNG as `fern.png` in that directory (or copy a known static PNG there). Do not rename another format to PNG. Record image dimensions and `sha256sum "$w45_samples"/*` before/after the native runs. Use only these copies for native actions. The browser uses its own sample fixtures; it does not open these files.

3. From the repository, use the pinned Node requirement in `website/package.json` (24.22.1 ≤ version <25). Build and serve static output:

   ```sh
   cd website
   npm ci
   SITE_BASE=/Radivine/ npm run build
   SITE_BASE=/Radivine/ npm run serve:static
   ```

   Open `http://127.0.0.1:4321/Radivine/#try`. Leave the server running. Record `git rev-parse HEAD`, browser/version and viewport. After project-base acceptance, stop the server with Ctrl+C, rebuild with `SITE_BASE=/ npm run build`, serve with `SITE_BASE=/ npm run serve:static`, and repeat navigation/download/guide/demo smoke checks at `http://127.0.0.1:4321/`. These commands do not publish.

4. Native Invoke: in a second terminal run `radivine invoke "$w45_samples/field-notes.txt"` (substitute PNG or both text paths as required). Alternatively select files in Nemo and use Open in Radivine. Browser: choose the matching **Sample context**, **Wheel layout**, then **Open wheel** / **Start Invoke practice**. Use an uncluttered central native position with sufficient space for Concentric; the browser's fixed virtual desktop is 900×800. Repeat comparisons with both layouts selected in Control Center and in the website.

5. Use Cinnamon's existing screenshot/recording controls or an already installed recorder. Record actions manually; screenshots alone cannot establish cancellation or timing. Include native version/settings and separate native/browser labels. Keep personal windows and paths out of the frame. Do not install capture or accessibility packages silently.

## 2. The six existing Rust traces

Source: [trace JSON](../../tests/fixtures/demo-native-traces.json), [emitter](../../scripts/capture-native-reference.mjs). The emitter uses the unchanged Rust engine in a temporary crate; it opens no desktop windows. There is no need to regenerate fixtures for manual acceptance.

All paths below are zero-based. Keyboard timestamps are 10/20/30/40 ms for `1`, `Backspace`, `1`, `1`; humans should enter the same sequence at a comfortable pace. For dwell, pointer `(450,280)` at 0 ms, ticks 109/110, pointer `(520,350)` at 120, ticks 299/300. Those coordinates are logical reference coordinates, not instructions to move the real mouse programmatically.

| ID / exact trace identity                         | Reference outcome                                                                                              | Human status |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | ------------ |
| T1 `branching / text / keyboard-geometry`         | Paths `[0]`, `[]`, `[0]`, `[0]`; last event commits Full Preview `[0,0]`.                                      | NOT TESTED   |
| T2 `branching / image / keyboard-geometry`        | Paths `[0]`, `[]`, `[0]`, `[0,0]`; JPEG hovered, no commit yet.                                                | NOT TESTED   |
| T3 `branching / text / dwell-and-inward-sibling`  | Preview opens at 110 ms; moving to root Copy closes child immediately; no commit.                              | NOT TESTED   |
| T4 `concentric / text / keyboard-geometry`        | Same paths/commit as T1; shared-center annuli.                                                                 | NOT TESTED   |
| T5 `concentric / image / keyboard-geometry`       | Same paths/no commit as T2; shared-center annuli.                                                              | NOT TESTED   |
| T6 `concentric / text / dwell-and-inward-sibling` | Preview opens at 110 ms; Copy hovered at 120 while child retained through 299; child closes at 300; no commit. | NOT TESTED   |

The automated comparisons enforce geometry within 0.002 logical pixels and angles within 0.00001 radians. Human checks below assess perceived order, intent and safe outcomes, not those tolerances.

## 3. Native-to-browser comparison checklist

Every row inherits the setup above. Run each row in **both layouts** unless specified. Save a short paired recording and a screenshot of the decisive state; record actual actions/outcomes, not just a tick. Native and browser status for every row initially: **NOT TESTED**.

| ID / samples and exact interaction                                                                                                                                                                                                                             | Expected native behavior                                                                                                                                                                                                          | Expected browser behavior / pass criterion                                                                                                                                                                                                                                                              | Evidence                                                                                                      |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| N1 / text; Invoke; press `1`, Backspace, `1`, `1` (T1/T4). Close viewer, repeat using arrows then Enter, and again Space on Full Preview.                                                                                                                      | Preview child opens, back returns root, reopen then Full Preview opens a read-only viewer. Branching uses adjacent wheels; Concentric uses fixed-center rings.                                                                    | Same hierarchy and final selection, with a simulation explanation instead of a viewer. PASS only if submenu keys do not execute an action early and each activation happens once.                                                                                                                       | Key sequence narration, open/back frames, native viewer and browser result.                                   |
| N2 / PNG; Invoke; press `1`, Backspace, `1`, `1` (T2/T5). Then select WebP; separately reopen and select Advanced.                                                                                                                                             | Convert → Image → targets; PNG and text Preview absent; WebP queues a real asynchronous job; Advanced opens its dialog. Inspect Jobs and output, never overwrite originals.                                                       | Same menu order, no action at the end of the four-key trace; WebP explains queuing, Advanced explains desktop handoff. No real file/job created.                                                                                                                                                        | Three-level menus, native Advanced/Jobs, browser explanations, original hashes.                               |
| N3 / text; hover root Preview at its middle radius until child opens; move inward/rootward to Copy and hold, without clicking (T3/T6).                                                                                                                         | Branching child closes on sibling movement; Concentric briefly retains child before closing after grace. Neither commits.                                                                                                         | Same qualitative order and retention distinction. PASS if no accidental result and correct final root selection. Do not demand human 110/180 ms measurements.                                                                                                                                           | Continuous video with cursor visible and layout recorded.                                                     |
| N4 / text; hover Preview, cross its connector (Branching) or inter-ring gap (Concentric) toward Head, then click Head. Repeat entering Custom → Scripts and return with Backspace.                                                                             | Safe traversal retains intended hierarchy, Head opens preview; returning closes only the appropriate level.                                                                                                                       | Same selected action/path with explanation. Concentric rings stay centered and sectors top-centered; Branching child placement may differ with actual screen space.                                                                                                                                     | Traversal video and nested/back screenshots.                                                                  |
| N5 / two text files; Invoke both paths or select both in Nemo; open Preview; try Full, Head and Tail using pointer and keyboard.                                                                                                                               | Leaves visible but disabled; single-file explanation; no preview opens.                                                                                                                                                           | “Select a single file to preview”; no result via click, digit or Enter. PASS requires understandable reason and no activation.                                                                                                                                                                          | Disabled state, attempted activation and reason in each UI.                                                   |
| N6 / inspection.txt; reopen native several times while observing inspection; browser choose checking sample and open Preview immediately, then wait.                                                                                                           | Pending items cannot activate; enablement may resolve too fast to observe; geometry stable during enrichment.                                                                                                                     | Checking state rejects activation, becomes enabled after illustrative 1.8 seconds without reordering. PASS compares semantics, never equal duration. If native pending cannot be observed, mark native BLOCKED and retain engine evidence separately; do not slow or alter application code.            | Recording from invocation through enablement; explicit observation limitation if transient.                   |
| N7 / text; in Nemo begin dragging, freshly press Shift, traverse Preview → Head and release on Head. Browser hold its sample, move, freshly press Shift, traverse and release. Repeat PNG Concentric, entering hub first in browser if activated away from it. | Overlay arms on fresh Shift; eligible release invokes chosen action once.                                                                                                                                                         | Synthetic drag yields exactly one explanation; no native files consumed. PASS if opening submenu alone never commits.                                                                                                                                                                                   | Whole gesture recording, selected leaf/result, unchanged source hashes.                                       |
| N8 / text; begin drag with Shift already held; release/repress Shift to arm. In a new armed drag press Shift again; finish by cancelling Nemo's drag with Escape over the source window.                                                                       | Pre-held Shift does not count as fresh arming; second press hides overlay and resumes native drag.                                                                                                                                | Pre-held Shift does not arm; second press cancels simulation. PASS allows this disclosed difference, no stale result afterward.                                                                                                                                                                         | Continuous key/gesture narration; no destination file move.                                                   |
| N9 / text; for each attempt reopen drag, hover an eligible leaf, then release in hub, outside, caption, connector bridge or Concentric ring gap/separator. Separately press Escape during Invoke.                                                              | Neutral releases reject/cancel without action; Escape closes. Source files remain in place with capture-all default.                                                                                                              | No stale hovered action commits; Escape/Cancel exits. **Branching painted separators are an exception below**, not a native neutral-zone assertion.                                                                                                                                                     | One labeled attempt per zone, before/after sample directory and hashes.                                       |
| N10 / text; Invoke Copy, then Properties, then default Custom → Scripts → Compute SHA-256, reopening between actions.                                                                                                                                          | Copy operates on native clipboard; Properties requires a supported file-manager handler; default Custom logs only. Do not run this case with executable custom overrides.                                                         | Copy/Properties are explanations only; Custom clearly says logging example, no fabricated checksum. PASS if menu/result meaning matches conditions; missing native Properties handler is BLOCKED with Doctor details.                                                                                   | Outcome text, native Properties window/log if available, safe clipboard inspection without private content.   |
| N11 / text; request Concentric; in browser open Layout fallbacks & simulation details and select each virtual scenario: insufficient space, no compositor, too-small targets; return to normal.                                                                | Source-defined fallback is Branching when constraints prevent Concentric. Real reproduction requires a separate disposable desktop/configuration; do not disable your live compositor or change monitor setup just for this test. | Each named scenario shows requested/effective layout and reason; normal scenario restores Concentric. For native evidence, review a version/configuration-proven verified recording or reproduce manually in a disposable X11 session; absent evidence remains NOT TESTED/BLOCKED, never inferred PASS. | Scenario/reason frames, native viewport/compositor/target settings if reproduced; flag simulated versus real. |
| N12 / text; browser start synthetic drag then change focus/window, scroll practice offscreen, return and reopen; repeat context/layout change while open.                                                                                                      | Native focus/window lifecycle is not claimed equivalent to browser visibility events; native Escape cancellation is covered by N9.                                                                                                | Stale sessions/timers do not fire on return; new controls and menu agree. PASS if next action happens once and focus remains usable.                                                                                                                                                                    | Browser lifecycle recording; native comparison marked adaptation, not a claimed native test.                  |

### Intentional adaptations versus defects

Accept only the documented differences: fixed virtual geometry and website styling/finite animations; scoped keys with normal Tab exit; accessible HTML list and touch alternative; synthetic drag rather than XDND; fixed illustrative inspection time; no actual conversion, command, clipboard or filesystem action; Properties fixture assumes a handler; named virtual fallbacks; browser-only visibility cleanup. Branching painted separators reject browser releases even though native Branching hit spans include visual gaps; Concentric separators remain neutral in both. Record this difference explicitly when comparing N9.

Wrong menu order, early submenu commits, enabled disabled actions, stale-target activation, missing cancellation or unannounced unusable controls are **not** adaptations. Record FAIL and severity. Arbitrary monitor edges, custom menus and exhaustive desktop environments are outside this bounded demo; do not claim them verified. Native fallback/brief pending evidence gaps need reviewer disposition, not fabricated passes.

## 4. Accessibility and device acceptance

Target WCAG 2.2 AA; manual acceptance is not certification. Run the website checks in Light and Dark, including active Branching/Concentric, disabled, pending and result states. Repeat the essential download → guide → demo → exit task on both bases. Record each combination separately in the result template.

| ID / initial status | Human procedure                                                                                                                                                                                                                                                                       | Expected outcome / evidence                                                                                                                                                                                                                                      |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A1 NOT TESTED       | Put mouse aside. Tab from page start; use skip link, navigation, Download, every guide, Compatibility and Roadmap. Open mobile-width navigation, Escape it; Shift+Tab backwards.                                                                                                      | Logical order, visible unobscured focus, skip reaches main, disclosure returns focus to trigger, no trap. Record video and any unreachable control.                                                                                                              |
| A2 NOT TESTED       | Keyboard-open demo, arrows, Enter/Space, digits, Backspace; use list/back/cancel/reset; Tab and Shift+Tab out while active and after result. Press arrows outside practice.                                                                                                           | Focus transfers usefully after replacement, exits normally, outside keys retain browser meaning; no hidden focus or duplicate action. Record focus before/after each transition.                                                                                 |
| A3 NOT TESTED       | With screen reader, traverse headings/landmarks, compatibility table by cells, installation command regions, sample/layout fields; open nested demo, disabled and pending context, then result and cancellation.                                                                      | Names/roles/state, table headers and reasons are understandable; new keyboard selection/result announced without pointer chatter, duplicate SVG reading or unexplained silence. Capture spoken output/transcript and AT/browser versions.                        |
| A4 NOT TESTED       | Without relying on color, inspect disabled/pending reasons and selected/focused controls in both themes, including SVG and overlapping labels. Measure doubtful pairs with an available contrast picker.                                                                              | Ordinary text ≥4.5:1, large text ≥3:1, essential controls/focus boundaries ≥3:1 against adjacent colors; state conveyed beyond color. Save sampled colors/locations and frames; resolve W4 axe review items, including symbolic Back control's descriptive name. |
| A5 NOT TESTED       | On physical Android/Chrome and iPhone/Safari where available, tap navigation, Download/guide, sample/layout, Open wheel and full-size list through Preview and Convert; back/cancel/reset; portrait and landscape. Repeat VoiceOver on iPhone and TalkBack on Android when available. | No drag/Shift or tiny radial target required, no accidental scrolling/activation, usable labels/targets, no clipped essential controls. Record device, orientation, touch/AT video. Emulation is not a substitute.                                               |
| A6 NOT TESTED       | Enable OS reduced motion before loading, scroll hero/story, use both demo layouts; toggle preference live and repeat. Test System theme with OS light/dark, then explicit Light and Dark.                                                                                             | Travel/scale effects stop, content and selection work, intent deadlines still function, theme remains legible and explicit choice persists. Record OS settings and short before/after videos.                                                                    |
| A7 NOT TESTED       | At 1280px-wide desktop window use actual browser zoom 200%, then 400%; test all routes, navigation, tables, commands and list. Separately use 200% text-only zoom in a supporting browser.                                                                                            | No lost content/actions or overlapping labels; page reflows, intentional tables/commands can scroll by keyboard, focus visible. Record actual zoom indicator plus viewport; do not substitute 320px emulation.                                                   |
| A8 NOT TESTED       | Use available high-contrast/forced-color mode; repeat download/guide/demo task. Read longer commands and compatibility qualification columns.                                                                                                                                         | Focus and control boundaries remain visible, command/table regions have names and can scroll, qualifications remain associated with rows. Capture screenshots and spoken table reading.                                                                          |
| A9 NOT TESTED       | On physical mobile load fresh page, scroll to practice and interact repeatedly; watch opening/closing, rotate, leave/return to page.                                                                                                                                                  | No disruptive jumps, persistent animation, blocked scrolling or delayed/duplicate activation. Record video and network/device conditions; investigate any apparent CLS budget regression rather than calling video a metric.                                     |

For physical phones, the QA server above binds only to loopback. On a trusted private LAN, you may separately run `npm run preview -- --host <desktop-LAN-IP> --port 4322` from `website/` after the project-base build, then open `http://<desktop-LAN-IP>:4322/Radivine/` on the phone. Replace the placeholder with your actual private address; stop preview with Ctrl+C afterward. Do not expose this preview to the Internet or change firewall rules automatically. If device access is unavailable, mark BLOCKED and arrange an isolated device lab. Use the QA static server and Playwright for 404/base assertions; Astro preview here is only for human touch/accessibility tasks.

Tools: existing browser zoom/devtools/accessibility inspector and OS preferences need no added packages. Check `command -v orca` on Linux before using Orca; if present, record its version and test Firefox/Chromium speech. Orca is useful Linux evidence but does not silently replace W4's required NVDA + Firefox/Chrome on Windows and VoiceOver + Safari on Apple devices. Use already available NVDA, VoiceOver, TalkBack, screenshot/recording and contrast tools; ask the owner to arrange a device/tool if absent and mark BLOCKED. Do not install system packages as part of this package. Playwright WebKit is not Safari device acceptance.

## 5. Complete Firefox and WebKit coverage in isolation

W4's Ubuntu 26.04 host could not launch Firefox (missing `libgtk-3-0t64`) or WebKit (GTK4 and media/graphics dependencies). Exact library diagnostics: [browser-dependencies.json](../../../docs/test-reports/media/W4/browser-dependencies.json); full list in W4 §2. These failed before page execution, so neither engine has a website result.

Safest local completion: a disposable Ubuntu 24.04 VM with a clean copy of the repository and pinned Node/dependencies. Keep the host desktop, display socket, credentials and package manager isolated. Install browser dependencies **only inside that disposable environment**, under its owner's control. An ephemeral website-only CI runner is also suitable: the [existing held website workflow](../../../.github/workflows/website.yml) already configures both bases and all three engines. Do not push or trigger it during W4.5, modify Rust CI, or remove publication holds. See the [deployment runbook](../../../docs/website/W4-deployment.md).

Inside the prepared disposable environment, from `website/`:

```sh
npm ci
npx playwright install --with-deps chromium firefox webkit
npm run check
npm run test:content
npm run test:demo
SITE_BASE=/Radivine/ npm run build
SITE_BASE=/Radivine/ npm run check:links
SITE_BASE=/Radivine/ SITE_BROWSERS=chromium,firefox,webkit npm test
# Save playwright-report/ and test-results/ outside the checkout before the next run.
SITE_BASE=/ npm run build
SITE_BASE=/ npm run check:links
SITE_BASE=/ SITE_BROWSERS=chromium,firefox,webkit npm test
```

Do not run another server on port 4321: Playwright starts the static server itself. Preserve reports/traces per base, command exit statuses, browser versions, lockfile/source SHA and actual test counts (baseline 82 per engine/base). Investigate failures; never suppress tests or count launch failures as passes. Dependency installation here is an instruction for a separate isolated environment, not authorization to change the current host. Human follow-up smoke statuses: Chromium **NOT TESTED**, Firefox **NOT TESTED**, WebKit **NOT TESTED**, Safari physical device **NOT TESTED**.

## 6. Evidence and decision template

Copy this block once per case/environment; attach files named, for example, `N4-native-concentric-light-YYYYMMDD.webm` and `N4-browser-concentric-light-YYYYMMDD.png`. Store reviewed evidence beside this document in a new dated directory or provide a stable owner-accessible link; do not commit personal recordings without privacy review.

```text
Case ID / subcase:
Status: NOT TESTED  (replace with PASS / FAIL / BLOCKED only after an attempt)
Tester / UTC date:
Evidence kind: HUMAN observation / AUTOMATED result (keep separate records)
Website commit / base URL / dependency lock revision:
Native version / package provenance / config deviations:
OS / desktop / session / compositor / file manager:
Browser / AT and versions / device model:
Resolution / scale / viewport / zoom / theme / reduced motion:
Samples / dimensions / before and after hashes:
Exact steps performed:
Expected native / browser outcomes:
Observed native / browser outcomes:
Intentional adaptation or unexpected mismatch:
Screenshot / recording / speech transcript / logs:
Issue / severity / reproduction / owner / retest evidence:
Reviewer / decision / explicitly accepted limitation:
```

Required evidence bundle: environment/version/settings proof; six trace-corresponding human paths (N1–N3); N4–N12 results and any explicit blocked dispositions; paired Branching preview and Concentric conversion recordings; native Advanced/Jobs/layout-setting captures with provenance; source file safety evidence; accessibility/device results A1–A9 with speech and actual zoom evidence; Firefox/WebKit results at both bases. Existing browser captures may support comparison but cannot stand in for these observations.

**Remaining blockers:** native representative fidelity acceptance; keyboard/assistive technology/actual zoom/physical touch and Safari acceptance; Firefox/WebKit cross-engine verification. No human pass has been recorded by preparing this file. Native transient pending or fallback conditions that cannot be reproduced must remain visible and receive a bounded reviewer decision supported by source/automated evidence; this does not waive the representative native comparison.

**GO for W5 consideration:** required comparisons and usability tasks have human evidence, cross-engine suites pass at both bases, material failures are fixed/retested, and reviewer signs the concrete source/evidence set. **CONDITIONAL GO:** the reviewer explicitly accepts only named non-blocking limitations (for example additional unclaimed desktops or field metrics), with owner and follow-up; critical accessibility, unsafe action behavior or missing representative fidelity cannot be silently waived. Otherwise retain **NO-GO**. Acceptance does not authorize publication: W5 still needs separate explicit approval; Pages/CDN smoke checks belong there.

## Preparation verification and local scope

Only this acceptance document is added. No manual desktop interaction, device/AT test, system package installation, workflow execution, application/branding change, push or publication is performed. Git history and the clean starting tree were inspected; W4 was already committed. Document validation and required repository-check outcomes are recorded below before the focused local commit.

Preparation checks: local Markdown links, fenced blocks and case inventory pass; Prettier and `git diff --check` pass; `cargo fmt --all --check` passes. Required full-workspace checks were attempted with Rust 1.99.1: Clippy again fails on existing `chunks_exact_to_as_chunks` diagnostics in renderer, conversion-image and X11 clipboard/shared-memory code; headless workspace tests stop at unchanged C5 visibility/caption p95 **5.0492 ms** versus **≤0.5 ms**. Later suites are not verified by that run. No application fixes or suppressions were made. These are preparation checks, not new browser or human acceptance evidence. The focused local commit is `docs(website): prepare W4.5 manual acceptance package`; obtain its hash with `git log -1 --oneline` after commit.
