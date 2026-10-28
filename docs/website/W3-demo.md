# W3 — Interactive wheel contract

Authorized W3 only, 2026-10-28. Product reference: v0.3.0, `1aa009a08bab66646898016f1ba953051eac46f3`. The browser practice area is a simulation; it never reads local files, writes the clipboard, executes commands, or creates conversion jobs. W1.5's decorative hero and scrolling story remain separate.

## Behavior and fixtures

`website/src/demo/contexts.ts` reproduces the default registry order for four bounded contexts: one readable text file, one static PNG, two readable text files, and pending text inspection. PNG hides Preview and the same-format PNG target; JPEG, WebP and Advanced remain. Multiple text files retain Preview but disable its leaves with the actual single-file reason. Custom → Terminal / Scripts → SHA-256 / Make Executable are default logging examples. Properties assumes a supported file-manager handler; this is an explicit fixture assumption, not browser environment detection.

The serializable reducer has idle, active, result and cancelled phases. An active submenu path represents navigation within active. Events carry injected timestamps; only the controller reads the browser clock. The reducer owns hierarchy, hover, input source, press pairing, generation, exposure guards, inspection and intent deadlines. Context/layout/environment changes replace the session and invalidate every timer and press. A release rechecks coordinates and eligibility without counting as fresh pointer movement. No action function has side effects; outcomes are explanations.

Geometry is in `geometry.ts`, independent of DOM/rendering. It ports root spans, child spans with 70° bridge and 72° sector cap, candidate placement (12° rotation steps, decreasing radii, edge and ancestor separation), corridor convex hulls, tangent intent triangles, and concentric annuli. Defaults come from application `to_wheel_config`, not bare engine defaults: root 40/128, child 34/108, deep 26/84, gap 20, dwell 110 ms, grace 180 ms, hysteresis 3.5°, slop 6, outer band 0.65, hub close 150 ms, margin 12. Concentric children are 60 thick with 6 gaps and 2-pixel fresh-motion guards. Center is fixed for the session. Actual `ring_geometry` uses top-centered sectors at every level; it does **not** rotate each child to its parent's sector. That source behavior takes precedence over the plan's “parent-sector alignment” wording.

Source provenance and hashes are in `tests/fixtures/demo-source.json`; every build rejects changed product version, revision or relevant source bytes until fixtures are reviewed. Node tests use deterministic time, without animation or wall-clock sleeps. Source/test correspondence:

| Browser scenario | Rust evidence |
| --- | --- |
| Text / PNG / multiple / checking menus | `actions_tests`: `test_preview_visibility_and_enablement_matrix_ac_4_1`, `test_ac_c3_1_menu_structure_for_image_and_text_snapshot_parity`, `test_ac_c3_2_visibility_and_progressive_enablement_matrix` |
| Branching hover, outer band, intent, hub and keyboard | `scenarios`: `scenario_01` through `scenario_09`; `session.rs` Branching handlers |
| Concentric fixed center, inward travel and gaps | `concentric_geometry`; `concentric_scenarios`: normative trace, `t_nav3`, `t_nav5`, `t_nav6` |
| Sibling grace, two-sample adoption, fresh-motion guard | `concentric_scenarios`: `t_nav4`, `t_nav7`; `concentric_nav.rs` |
| Keyboard takeover, pairing and strict drop rejection | `concentric_scenarios`: `t_nav9`, `t_safe1_to_t_safe5`; `session.rs` release and drop handlers |
| Fallbacks | `plan_concentric`, `select_initial_layout`, application compositor preflight |

## Deliberate browser differences

- This is a TypeScript/SVG interpretation of bounded fixtures, not compiled Rust or a native application screenshot. Rendering, typography and finite animations use the website's semantic theme and motion system. No pixel-identical desktop claim.
- A labeled 900×800 virtual desktop keeps geometry deterministic. Its center anchor is fixed at (450,350), reserving the 668×742 concentric footprint. Root displacement, arbitrary desktop edges, monitors, custom menus/limits and crowded Branching fallback scoring are not offered. A synthetic drag activated away from the fixed concentric hub must enter the hub first; keyboard/list access can bypass that pointer guard.
- Branching's painted separators are explicitly non-actionable in the browser. Rust Branching hit spans include their visual gaps and use angular hysteresis for selection; Concentric has strict neutral separators. Browser release rejection is intentionally stricter in this narrow case, matching the website safety contract. Hover retention never turns a browser neutral zone into an action target.
- The desktop's platform drag arming/XDND protocols cannot run in a web page. A pointer-held sample plus a **fresh** Shift press simulates activation. A second Shift hides/cancels this session; the desktop would resume native dragging. There is no OS drag interception, real file input, native conversion or clipboard write.
- Pending inspection completes after an illustrative 1.8 seconds. Actual inspection depends on the file and environment. The fixture changes only enablement and retains geometry.
- Tab and Shift+Tab leave the practice surface normally. Scoped arrows cycle, Enter/Space activate, digits activate, Backspace goes up and Escape cancels. A synchronized native-button list supplies tap and assistive-technology access; touch is a website adaptation, not a desktop support claim.
- Layout fallback controls are named virtual desktop scenarios (insufficient space / no compositor / too-small configured targets), not detection of the visitor's OS. Mobile uses the same logical geometry for reference and full-size action buttons for interaction.
- Offscreen, focus loss during a synthetic drag, pointer cancellation and page hiding cancel stale sessions/timers. Reduced motion removes visual movement without changing intent deadlines.

## Fidelity gate

Source/unit/browser evidence is not real-desktop parity. Compare the result with current stable desktop captures before making that claim. The existing N3 image pack is a renderer/sandbox reference, and older Branching screenshots are historical; neither certifies a fresh v0.3.0 Mint/Nemo session. Record actual comparisons, artifacts, accessibility limits and remaining owner review in the W3 test report. W4 and publication require separate authorization.
