# Radivine official website — repository audit and implementation plan

**Status:** Planning complete. W0 and W1 were subsequently authorized and implemented locally; see the [W0 contract](website/W0-contract.md) and [W0/W1 verification report](test-reports/W0-W1.md). W1.5 was subsequently authorized and implemented locally; see its [motion contract](website/W1.5-motion.md) and [visual review](test-reports/W1.5.md). W2 was subsequently authorized and completed locally; see the [content/install verification report](test-reports/W2.md) and [current media handoff](website/W2-media.md). W3 was subsequently authorized and implemented locally; see its [fidelity contract](website/W3-demo.md) and [verification report](test-reports/W3.md) for remaining native/manual acceptance limits. W4–W5 and publication remain unauthorized. Planning-only scope statements below describe the original audit task.

**Audit date:** 2026-10-27 UTC.

**Source baseline:** `35a5e5fff816ed968d833e8922e10b03e500cdc1` on the existing `feat/website` branch; initial working tree clean.

**Product baseline:** Radivine **v0.3.0**, Linux X11, published 2026-10-26.

**Agreed technology:** Astro + Tailwind CSS; static GitHub Pages hosting; source in `website/`; English; responsive, accessible; Light / Dark / System website themes.

## 1. Recommendation and scope

Build a small, product-focused static site around **the actual file-action wheel**, with a real application visual first, an explicitly labeled browser simulation, a prominent Linux X11 compatibility qualifier, and versioned installation instructions. Use the existing green vine identity and “Branch into action.” tagline. Avoid suggesting an application search launcher, dock, cloud service, or universal Linux desktop integration.

The site should answer, in order: what the gesture does; what useful actions ship; whether it works on the visitor's desktop; how to install and start it; and how to contribute. Downloads and compatibility must work without JavaScript. The demo, theme preference, and copy controls are progressive enhancements.

This task changes only this planning document. It does not authorize creating `website/`, changing Rust source, packaging, assets, CI, release tooling, repository settings, or publishing. Proposed website phases below are independent approval boundaries, not new authorized application milestones. Existing `AGENTS.md` remains binding, including its authorization and asset-protection rules; its M6/v0.1.0 state paragraph is historical relative to current source and releases.

## 2. Evidence rules

Use separate dimensions for **implementation**, **verification**, and **commitment**:

- **Implemented:** a shipping code path exists in this checkout. A trait, enum, design outline, menu label, or test stub alone is insufficient.
- **Automated:** identify unit/property, fake-platform, headless integration, isolated Xvfb, or ignored/explicit-run tests. Passing headless tests do not prove a compositor or file manager works.
- **Recorded manual:** a repository report records owner testing on a named environment. This audit did not reproduce that desktop session.
- **Documented claim:** documentation says it works, but the inspected evidence does not establish the complete claim.
- **Proposed/deferred:** future design or capability with no shipping path; no delivery date or version commitment implied.
- **Manual verification required:** retain the qualification until a dated environment-specific result is recorded.

“Supported” in the inventory means implemented within the stated conditions, not a guarantee for every distribution, file type, external command, or hardware configuration. Source/tests take precedence over broad README wording. Published release metadata establishes availability, not desktop compatibility.

### Evidence index

Paths are relative to this document. Symbols and test names below locate the relevant behavior without relying on unstable line numbers. When publishing evidence links on the website, use a full audited commit SHA in GitHub `blob` URLs; release links use their immutable version tag.

| ID | Evidence inspected | What it establishes |
|---|---|---|
| E01 | [Workspace manifest](../Cargo.toml), [lockfile](../Cargo.lock), [changelog](../CHANGELOG.md), [stable notes](../RELEASE_NOTES_v0.3.0.md) | Workspace version, dependencies, release chronology and product changes. |
| E02 | [README](../README.md), [product vision](PRODUCT_VISION.md), [architecture](ARCHITECTURE.md) | Product intent and published claims; some passages predate current behavior. |
| E03 | [Platform entry point](../crates/radivine-platform/src/lib.rs) `connect_with_config`; [X11 backend](../crates/radivine-platform/src/x11/mod.rs); [doctor](../crates/radivine-platform/src/x11/doctor.rs) `probe_with_connection` | Shipping wheel backend is X11; Wayland drag explicitly unavailable; XWayland pointer degradation. |
| E04 | [Application configuration](../apps/radivine/src/config.rs) `WheelSection::default`, `to_wheel_config`; [example configuration](../packaging/config.example.toml) | Application defaults, configuration validation/mapping, safe pass-through default. |
| E05 | [Wheel configuration](../crates/radivine-wheel/src/config.rs), [session](../crates/radivine-wheel/src/session.rs), [concentric geometry](../crates/radivine-wheel/src/concentric.rs), [navigation](../crates/radivine-wheel/src/concentric_nav.rs) | Branching/concentric behavior, fallback geometry, keyboard and pointer state machines. |
| E06 | [Session controller](../apps/radivine/src/session.rs), [daemon/CLI](../apps/radivine/src/main.rs), [XDND](../crates/radivine-platform/src/x11/xdnd.rs), [arming](../crates/radivine-platform/src/x11/arming.rs) | Gesture wiring, drop-time validation, safe source protocol, IPC, X11 lifecycle. |
| E07 | [Action registration](../crates/radivine-actions/src/lib.rs) `default_menu`, `register_builtins_with_options`; [built-ins](../crates/radivine-actions/src/builtins.rs); [custom actions](../crates/radivine-actions/src/custom.rs); [LogAction](../crates/radivine-core/src/action.rs) | Real built-ins versus logging-only demo entries; shell-free executable custom actions. |
| E08 | [Context](../crates/radivine-core/src/context.rs), [enricher](../crates/radivine-core/src/enrich.rs), [registry](../crates/radivine-core/src/registry.rs) | Context derivation, bounded file inspection, menu pruning and enablement. |
| E09 | [Convert actions](../crates/radivine-actions/src/convert.rs), [formats](../crates/radivine-convert/src/format.rs), [engine](../crates/radivine-convert/src/engine.rs), [options](../crates/radivine-convert/src/options.rs), [settings](../crates/radivine-convert/src/settings.rs), [image backend](../crates/radivine-convert-image/src/lib.rs) | PNG/JPEG/WebP pipeline, presets, metadata/resize policy and resource limits. |
| E10 | [Atomic output](../crates/radivine-convert/src/output.rs), [worker](../apps/radivine-worker/src/), [job contracts/client](../crates/radivine-jobs/src/), [worker acceptance tests](../apps/radivine-worker/tests/c2_acceptance.rs) | Durable jobs, no-overwrite output, cancellation, retries and recovery. |
| E11 | [Control Center](../apps/radivine-control/src/ui/), [service management](../apps/radivine-control/src/service.rs), [persistence](../apps/radivine-control/src/persistence.rs), [C4 tests](../apps/radivine-control/tests/c4_acceptance.rs), [layout settings tests](../apps/radivine-control/tests/nested_settings.rs) | Eight pages, Advanced Convert, live Jobs, lifecycle/settings behavior. |
| E12 | [Viewer](../apps/radivine-viewer/src/), [preview acceptance](../apps/radivine/tests/m5_preview_acceptance.rs) | Read-only Full/Head/Tail, virtualized file access and search. |
| E13 | [Action tests](../crates/radivine-actions/tests/actions_tests.rs), [M4 tests](../apps/radivine/tests/m4_actions_acceptance.rs), [C3 tests](../apps/radivine/tests/c3_convert_acceptance.rs), [conversion corpus tests](../crates/radivine-convert-image/tests/corpus_regression.rs) | Visibility/enablement, execution contracts, conversion handoff, mixed batches, malformed image handling. |
| E14 | [Wheel scenarios](../crates/radivine-wheel/tests/scenarios.rs), [concentric scenarios](../crates/radivine-wheel/tests/concentric_scenarios.rs), [geometry tests](../crates/radivine-wheel/tests/concentric_geometry.rs), [properties](../crates/radivine-wheel/tests/properties.rs), [allocation test](../crates/radivine-wheel/tests/alloc_test.rs) | Deterministic interaction and geometry invariants. |
| E15 | [Nested acceptance](../apps/radivine/tests/nested_acceptance.rs), [X11 tests](../crates/radivine-platform/tests/x11_tests.rs), [render tests](../crates/radivine-render/tests/) | Fake-platform safety cases, guarded live Xvfb protocol checks, rendering/allocation verification. |
| E16 | [N5 report](test-reports/N5.md), [N6 report](test-reports/N6.md), [stable release verification](test-reports/RELEASE_v0.3.0.md), [C5 report](test-reports/C5.md), [benchmarks](BENCHMARKS.md) | Historical automated runs, limited versus broader owner sign-offs, package install/upgrade evidence and benchmark conditions. |
| E17 | [Debian metadata](../packaging/debian/control), [build script](../scripts/build-deb.sh), [Nemo integration](../packaging/radivine.nemo_action), [daemon service](../packaging/radivine.service), [worker service](../packaging/radivine-worker.service) | Actual package architecture, dependencies, installed files, opt-in lifecycle and build procedure. |
| E18 | [Current CI](../.github/workflows/ci.yml), [dependency guard](../scripts/check-dep-graph.sh), [testing strategy](TESTING_STRATEGY.md), [contribution guide](../CONTRIBUTING.md) | Existing checks, test safety and contributor workflow. |
| E19 | [Decisions](DECISIONS.md) ADR-015/016/025/042/045, [platform integration](PLATFORM_INTEGRATION.md), [original implementation plan](IMPLEMENTATION_PLAN.md), [conversion planning](planning/v0.2.0/), [nested planning](planning/nested-wheels/) | Accepted design decisions and future outlines; planning status alone does not prove shipment. |
| E20 | [Branding](../assets/branding/), [app icons](../assets/icons/), [wheel icons](../assets/wheel-icons/), [fonts](../assets/fonts/), [notices](../THIRD_PARTY_NOTICES.md), [screenshots](screenshots/), [N3 visual index](test-reports/screenshots/n3-concentric/INDEX.md) | Existing visual material, protected assets, provenance and licensing. |
| E21 | [Security policy](../SECURITY.md), [conduct](../CODE_OF_CONDUCT.md), [MIT](../LICENSE-MIT), [Apache](../LICENSE-APACHE) | Contribution/security destinations and license; policy version table requires maintainer review. |

## 3. Repository audit summary

### 3.1 Architecture and product boundaries

The workspace ships four cooperating binaries: `radivine` (wheel daemon and CLI), `radivine-control` (native Control Center and Advanced Convert), `radivine-viewer` (read-only text preview), and `radivine-worker` (on-demand durable conversions). The pure wheel produces scenes; the CPU renderer draws them; the platform supplies X11 input/windowing; the application connects those pieces to the context/action registry. Conversion codecs run through the separate worker. [E01, E03, E05–E12]

`radivine-wheel` forbids unsafe code, depends only on `smallvec` at runtime, and has no platform, renderer, or file-action dependency. `radivine-render` also forbids unsafe code and is platform independent. Website work must preserve these boundaries; a web demo is not justification to add web dependencies to either crate. [E05, E14, E18]

The most defensible positioning is **“A context-aware radial action wheel for Linux files.”** “Action launcher” is acceptable with that context; “application launcher” implies search/global shortcut management that is not implemented. The old product vision's statements that there is no settings GUI or conversion functionality no longer describe the shipping product. [E02, E07, E09, E11]

### 3.2 Release history and availability

Live GitHub connector reads on the audit date confirmed the public release list and `/releases/latest`; latest is `v0.3.0`, `draft=false`, `prerelease=false`. These reads verified metadata and asset digests, not a new binary install or anonymous package checksum run.

| Release | Published (UTC date) | Verified purpose / distribution |
|---|---|---|
| v0.1.0 | 2026-10-23 | Initial wheel/actions/preview and Debian package. |
| v0.1.1 | 2026-10-23 | Control Center and lifecycle/settings improvements. |
| v0.2.0 | 2026-10-25 | Conversion suite, durable worker, Advanced Convert and Jobs. |
| v0.3.0-rc.1 | 2026-10-25 | Concentric pre-release; not the default download. Actual public asset name is `radivine_0.3.0.rc1_amd64.deb`, which differs from Debian-version notation in some docs. |
| v0.3.0 | 2026-10-26 | Stable Concentric Rings; Branching remains default. |

Sources: [changelog](../CHANGELOG.md), [public release history](https://github.com/marawan6569/Radivine/releases), [latest release API](https://api.github.com/repos/marawan6569/Radivine/releases/latest), [stable release](https://github.com/marawan6569/Radivine/releases/tag/v0.3.0).

Current package: [`radivine_0.3.0_amd64.deb`](https://github.com/marawan6569/Radivine/releases/download/v0.3.0/radivine_0.3.0_amd64.deb), **16,149,118 bytes**, SHA-256 `4c0b8cc719963dec2f2457588a500a3530dd4aa88b01d0b3954eeb57edb37156`. The API digest matches E16. Also published: [`SHA256SUMS.txt`](https://github.com/marawan6569/Radivine/releases/download/v0.3.0/SHA256SUMS.txt) and versioned release notes. The inspected workflow builds packages but has no release-publication job; release assets and website deployment must remain separate processes.

### 3.3 Verification coverage and limits

- Release report E16 records 585 passing workspace tests and a separate 23-test isolated Xvfb run, plus package/container, supply-chain and historical endurance/fuzz checks. Treat these as historical reported results, not newly measured figures or a count of desktop-certified cases.
- `nested_acceptance` includes fake-platform tests and `live_xvfb_concentric_end_to_end_xdnd_session_and_compositor_guards`. X11 test code returns early outside an isolated display. A green headless run alone does not exercise those live paths. [E15]
- Long soak/fuzz/performance tests include ignored tests requiring explicit invocation. Existing CI runs formatting, dependency boundaries, Clippy, headless workspace tests, screenshot export, Debian packaging/lintian, and cargo-deny; it does not run all recorded manual or long-duration gates on every push. [E16, E18]
- C5 records owner acceptance of v0.2.0 on Mint 22.3 Cinnamon X11. N5 records a positive concentric Nemo smoke test and later RC1 owner release sign-off on a separate Mint laptop. Individual N5 reject-zone, multi-monitor, edge/corner and composited-performance checklist rows remain without detailed manual sub-case logs. Do not silently convert that general sign-off into an exhaustive compatibility certification. [E16]
- No complete manual matrix for XFCE, MATE, GNOME Xorg, KDE X11, native Wayland, ARM, Windows or macOS was found in the inspected material.

### 3.4 Documentation discrepancies to contain in website copy

| Claim or material | Audit finding | Website treatment |
|---|---|---|
| README “any XDND v5 source” and broad Ubuntu/Debian desktop support | Protocol implementation exists; real-world verification is concentrated on Nemo/Cinnamon. | Say “Nemo on Cinnamon X11 is the primary tested setup”; mark other sources/desktops as requiring verification. |
| README Wayland Invoke fallback | The actual wheel connection selects `X11Platform`; doctor marks XWayland pointer positioning degraded. No native Wayland backend exists. | Explain conditional XWayland Invoke experimentation; do not advertise native Wayland wheel support. |
| Platform document's Wayland/layer-shell/Windows/macOS matrix | It mixes current X11 behavior with future design possibilities. | Rebuild the public matrix from shipping code and recorded environment tests. |
| Custom menu examples “Open in Terminal”, “Compute SHA-256”, “Make Executable” | Default registrations are `LogAction`, which logs and returns `Outcome::Done`. Real TOML actions can replace them; packaged examples use different IDs for real commands. | Explicitly distinguish logging demo entries from executable user-configured actions. Never claim that hovering/clicking those defaults runs their named operation. |
| `PRODUCT_VISION.md` describes GUI/concentric as future and outside-cluster pass-through | GUI/concentric shipped; capture-all is the safe default. | Use current source and ADR-016/045; preserve older docs unchanged in this task. |
| README source prerequisites “Rust 1.85+”, libwebp 1.5.0 | CI uses Rust 1.95.0; current notices describe libwebp 1.6.0. A tested MSRV is not established by the edition alone. | Recommend the CI-tested toolchain, or a separately verified newer stable; do not promise 1.85 compatibility. |
| Packaged `convert.example.toml` | The example's 256 MiB source limit differs from `ResourceLimitsConfig::default()` (512 MiB); example preset/table naming also needs checking against the current loader. | Prefer Control Center-generated configuration; verify every published TOML snippet with the current parser. Do not describe example comments as runtime defaults. |
| Release/README low-level security phrasing | Packaged worker unit does not set `PrivateNetwork=yes`; source reader named `open_source_nofollow` follows valid symlinks and opens with `RDONLY` and `CLOEXEC`. | Describe tested no-overwrite outputs and out-of-process execution; avoid copying inaccurate flag or sandbox claims. |
| Security policy “supported versions: 0.1.x” | Stale relative to v0.3.0. | Link reporting policy; ask maintainer to confirm supported-version policy before publishing support lifetime claims. |
| Existing screenshot labels and contents | Control Center dashboard visibly says v0.1.1; wheel screenshots omit newer image-dependent menus; concentric images show sandbox/debug fixtures. | Caption historical/illustrative images honestly and obtain current captures for release-specific claims. |
| Fast latency / large-file numbers | Benchmarks have specific machines, layouts, data, warm/cold and Xvfb conditions. | Keep numbers off the hero; if used later, attach version, conditions, metric and benchmark link. |

These are planning findings, not authorization to repair application code or existing documentation.

## 4. Feature and compatibility inventory

### 4.1 Implemented and supported within the primary X11 scope

| Feature | Actual behavior / scope | Implementation and verification evidence |
|---|---|---|
| Mid-drag wheel activation | Fresh Shift press during file drag; second press can hide the wheel and resume native dragging. Modifier configurable. Safe default captures the active overlay; outside release cancels. | E04/E06; `m3_drag_acceptance`, arming/XDND tests E15; recorded Nemo smoke/owner results E16. |
| Branching hierarchy | Default layout; adjacent child wheels, connector stems, hover-intent protection, screen adaptation. | E05; `scenarios`, `properties` E14; render and M2/M3 acceptance E15/E16. |
| Concentric hierarchy | Optional outward rings with fixed session center; reject zones and layout fallback. Default configuration needs 668×742 logical viewport; required size varies with configured radii/depth. | E05/E06; concentric geometry/scenarios and `t_safe1_t_safe2_concentric_drag_commit_and_non_actionable_zone_drops` E14/E15. |
| Invoke mode | CLI/IPC or packaged Nemo “Open in Radivine” action; mouse and keyboard navigation on X11. A desktop shortcut is manually configured by the user. | E03/E06/E17; `m2_acceptance`; wheel navigation tests E14. |
| Context-aware menu | Visibility uses quick file context; enablement can be “Checking…”, enabled or disabled with a reason after bounded inspection. Multi-file context supported; not every selected file is inspected before display. | E07/E08; `MAX_ENRICHED_ITEMS = 64`; action matrix and C3 tests E13. |
| Read-only text preview | Full, Head, Tail; single eligible local text file; virtualized reads and streaming search; not an editor or graphical image preview. | E07/E12; M5 acceptance and viewer file-I/O/search tests. |
| Copy files to clipboard | X11 file URI/clipboard selection, for later paste; does not itself copy bytes into a destination folder. No Cut built-in. | E07; `test_copy_and_properties_actions` E13; clipboard protocol tests E15. |
| Quick image conversion | `Convert → Image → PNG / JPEG / WebP`; local static images; single/batch, mixed selections with reported skips. Same-target Quick Convert can be disabled or skipped. | E09; `test_ac_c3_1_menu_structure_for_image_and_text_snapshot_parity`, progressive enablement and C3 handoff tests E13. |
| Advanced Convert | Separate native dialog, built-in/custom presets, explicit quality controls, resize, metadata/alpha options, destination policy; same-format re-encoding available. | E09/E11; `ac_c4_5_advanced_convert_selection_handoff_zero_pixel_decode_validation_and_job_submission`; corpus tests E13. |
| Background jobs | Separate worker, SQLite WAL history, cancellation, linked retries, crash reconciliation, on-demand startup and idle exit. Recovery is not automatic continuation of all unfinished conversions. | E10; worker C2 and Control Center C4 lifecycle/cancel/retry tests. |
| Source/output safety | XDND uses Private/reject semantics rather than Move/Delete; conversion writes separate output with no-overwrite commit/collision handling. User custom commands retain their configured effects. | E06/E10; source snapshots E15 and output/worker tests. Avoid “nothing can ever change your files.” |
| Native Control Center | Dashboard, General, Wheel, Custom Actions, Conversion, Jobs, Diagnostics, About; daemon start/stop/reload, opt-in login startup. Settings preserve TOML comments/unknown keys. | E11/E17; C4, v0.1.1 acceptance, login regression and nested settings tests. |
| Executable custom actions | Validated TOML argv templates, whole-argument placeholders, filters, timeouts/notifications. External program must exist. | E07; `test_custom_actions_parsing_security_validation_and_execution_ac_4_5_and_4_6` E13. |
| Application themes and motion | Dark/light/auto desktop theme handling; reduced-motion settings. Website System theme will use browser preferences independently. | E04/E05, renderer theme and Control Center theme source. |

### 4.2 Partial support and limitations

| Capability | Qualification | Evidence / manual check |
|---|---|---|
| Properties | Shipping D-Bus request to `org.freedesktop.FileManager1.ShowItemProperties`; needs a compatible session service. Mock D-Bus/backend tests do not prove every file manager supplies it. | E07/E13; test each additional desktop/file manager. |
| Non-composited X11 | Opaque/shaped Branching fallback exists; Concentric falls back to Branching when compositing is unavailable. | E03/E05/E06/E15; verify visual quality on actual target WM. |
| Small screens / custom geometry | Concentric may fall back for insufficient space or undersized targets. 668×742 is the application-default footprint, not a universal constant for every configuration. | `LayoutFallback` E05; `to_wheel_config` E04. |
| Multi-monitor / HiDPI | RandR/Xft scale paths and automated geometry/Xvfb coverage exist; global Xft scaling is not evidence of complete per-monitor mixed-DPI fidelity. | E03/E15/E16; live mixed-DPI/hotplug and edge cases still need environment-specific checks. |
| Keyboard during foreign file drag | Best effort due to foreign pointer/keyboard grabs. Invoke is the reliable keyboard-oriented path. | E03 doctor `keyboard_in_drag = Degraded`; do not guarantee Esc/navigation on every drag source. |
| Arabic/non-Latin text | System font fallback and shaping have recorded tests; widget containers remain LTR. Runtime font availability matters: this audit's environment failed three fallback tests. Not evidence of a translated Arabic UI or full RTL layout. | E11; `arabic_font_fallback` tests; E16 and release limits; current-run details in §12. |
| Image conversion limits | Runtime defaults include 512 MiB source size, 256 MiB allocation cap, 16,384 px per dimension, 100 MP; animation rejected. Effective settings can differ. | E09 `ResourceLimitsConfig::default`; malformed/animated corpus E13. |
| Metadata policy | Default EXIF stripping, auto-orientation and ICC preservation; options vary by target format. Does not promise universal metadata retention. | E09; EXIF/RIFF/backend tests E13. |
| No-overwrite file output | Linux filesystem commit path and typed errors; do not claim all network/removable filesystems have been certified. | E10; verify chosen non-local filesystems before naming them. |
| Optional conversion backends | Discovery/routing framework and stub tests exist; external vips adapter is deferred, not a shipping codec expansion. | E19 ADR-042; E09 backend/catalog source. |
| Desktop service fallback | systemd user service is preferred; detached-process/XDG fallback exists. Minimal or non-systemd distributions are not thereby certified. | E11/E17; test desktop environment import and lifecycle explicitly. |

### 4.3 Unsupported or not shipped as product capabilities

- Native Wayland wheel backend; global Drag + Shift on Wayland; working layer-shell/portal trigger or Drop Shelf. Design outlines are future research. [E03, E19]
- Windows/macOS application backends or installers; published ARM packages; RPM, AppImage, Flatpak, Snap, AUR package or hosted apt repository in the inspected release/packaging setup. Source-build possibility is not a support guarantee. [E01, E03, E17]
- Built-in GIF/APNG/animated WebP processing, AVIF/HEIC, SVG/PDF conversion, document/audio/video conversion. [E09, E13]
- Universal application search, built-in global shortcut manager, native Nautilus/Dolphin/Thunar/Caja integration files, a tray indicator, general plugin runtime, binary plugin ABI or WASM action host. [E07, E17, E19]
- Viewer editing, graphical image preview and live-follow `tail -f`. [E12, E19]
- Actual terminal launch, chmod or SHA-256 computation from the default logging-only registrations. Equivalent real custom argv actions can be configured; that is a separate supported mechanism. [E07]

### 4.4 Public compatibility matrix — proposed initial truthful copy

Keep distribution, desktop, session protocol, file manager, CPU and verification level visible as separate fields. Avoid a single green “Linux supported” badge. “Not verified” is different from “does not work.”

| Environment | Drag + Shift | Invoke wheel | Other tools / installation | Evidence and public label |
|---|---|---|---|---|
| Linux Mint 22.3, Cinnamon/Muffin, X11, Nemo, amd64 | Implemented; owner-tested | Implemented; primary workflow | Published .deb; Control Center/viewer/worker; owner release sign-off | **Primary tested setup.** E03/E16/E17. Preserve N5 sub-case caveats. |
| Other Mint 22.x Cinnamon X11 versions | Expected through same backend | Expected | .deb candidate | **Documented target; exact version requires verification.** README breadth is not a test result. |
| Ubuntu 24.04 amd64 | Depends on X11 desktop and drag source | X11 path exists | Clean .deb install/upgrade recorded in Ubuntu container | **Package installation verified; desktop integration requires verification.** E16. |
| Debian / Ubuntu-family XFCE, MATE, GNOME Xorg, KDE Plasma X11 | XDND implementation is a candidate | X11 implementation is a candidate | Runtime package dependencies must be checked | **Documented compatibility claim; not individually certified by this audit.** E02/E03/E17. |
| Other X11 file managers / distributions | Unverified by named source | CLI exists; integration manual | Source-build path; no dedicated package promise | **Community testing needed.** XDND version alone is insufficient. |
| X11 without compositor | Branching fallback implemented | Branching fallback implemented | Concentric unavailable for that session | **Degraded rendering; manual desktop check required.** E03/E15. |
| Wayland session with XWayland | Unavailable by capability policy | Experimental/conditional X11 connection, possibly restricted pointer placement | Control/viewer compile with Wayland support; worker is non-graphical; not a full Wayland acceptance result | **No native Wayland wheel support; manual verification required.** E01/E03. |
| Pure Wayland with no usable X11 display | Unavailable | Unavailable through current shipping wheel backend | Control/viewer are separate GUI paths; verify individually; worker not proof of wheel availability | **Wheel unsupported.** `connect_with_config` always selects X11 outside test-only headless mode. |
| ARM Linux; Windows; macOS | No supported shipping path established | No supported shipping path established | No matching published installer | **Unsupported release targets.** Do not render enabled download buttons. |

### 4.5 Installation inventory and required content

1. **Primary:** download versioned amd64 `.deb` and matching checksum file from GitHub Releases; checksum verification followed by `sudo apt install ./radivine_0.3.0_amd64.deb`. Show Linux X11 and architecture requirements before the download. Checksums verify downloaded bytes against the published file; they are not a signed release provenance guarantee.
2. **First launch:** application menu → Radivine Control Center → Start Daemon; login startup is opt-in. Mention `systemctl --user enable --now radivine.service` as an alternative, not a command to run against the user's desktop during website development. Nemo “Open in Radivine” is installed with the package. [E11/E17]
3. **Source:** link a version-pinned clone/build guide using the lockfile (`cargo build --release --locked`); explain that a build alone does not install desktop entries, Nemo actions or services. Recommend the release CI toolchain as the established reference (1.95.0); separately validate actual toolchain minimum. Package building uses `./scripts/build-deb.sh` and needs C/build/packaging tools and Python Pillow. [E01/E17/E18]
4. **Package details:** four binaries, user service units, desktop entry, Nemo action and examples. Declared dependencies: `libc6 >= 2.34`, `libgcc-s1 >= 3.0`, `fonts-dejavu-core`, `libfontconfig1`; recommended libraries/tools are not proof of successful GUI operation on every base system. [E17]
5. **Upgrade/removal:** link recorded v0.2.0/RC-to-v0.3.0 package checks and preserved user config/job history; document `sudo apt remove radivine`. Do not promise arbitrary version migrations or migrations never tested. [E16/E17]
6. **Fallback:** link the release page when a direct asset is unavailable; retain readable instructions if clipboard APIs or JavaScript are unavailable. No curl-to-shell installer, browser distro auto-detection, automatic installation, or unsupported package buttons.

### 4.6 Planned versus potential application work

Publish completed v0.1/v0.1.1/v0.2/v0.3 milestones from releases. GUI settings, conversion and concentric rings must not appear in an upcoming column. No next-version implementation commitment is established by this audit.

Potential items supported by existing design notes: Wayland research (portal/layer-shell hypothesis, Drop Shelf and DE-specific integrations), additional contexts (text/URLs/clipboard), dynamic Open With providers, live-follow text preview, third-party action protocols, Windows/macOS exploration, deferred tray indicator and external vips adapter. Label these **Exploring** or **Deferred**, link the relevant ADR/design section, and omit dates/target versions unless a maintainer explicitly confirms them. Historical wording about v0.2.x vips delivery is not a renewed commitment. [E19]

## 5. Website product architecture and UX

### 5.1 Sitemap

All paths below are relative to the configured site base, initially expected to be `/Radivine/`. The repository name is case-sensitive. Keep the navigation to **Try it**, **Download**, **Compatibility**, **Guide**, **Roadmap**, and **GitHub**; contribution details are linked from GitHub/footer and relevant sections.

| Route | Purpose and content |
|---|---|
| `/` | Product introduction, real wheel hero, gesture explanation, demo, feature stories, concise compatibility and release summaries, contribution invitation. |
| `/#demo` | Directly linkable practice area; no separate app or full-screen takeover. |
| `/download/` | Current stable package, requirements, checksum and installation, first launch, source-build link, older releases, upgrade/removal guidance. |
| `/compatibility/` | Full evidence-qualified matrix, X11/Wayland explanation, layout fallbacks, file-manager caveats and a reproducible community test-report checklist. |
| `/guide/` | Short getting-started guide: start daemon, drag/invoke, navigate/cancel, configure; link deeper guides and upstream engineering docs. |
| `/guide/configuration/` | File locations, Control Center, a parser-verified custom action example and external-command dependency notes. |
| `/guide/troubleshooting/` | `radivine doctor`, service/IPC checks, X11 session selection, fallback reasons, missing Properties provider, unsupported input; link issue reporting. |
| `/roadmap/` | Shipped release timeline plus clearly separate Exploring/Deferred items; evidence links, last reviewed date, no invented ETA. |
| `/contribute/` | Website/Rust/docs/testing contribution paths, build commands, issue/PR links, code of conduct, private vulnerability-reporting link, licenses. |
| `/licenses/` | Project license links and notices for redistributed Inter/Lucide assets; brief website storage/hosting privacy explanation. |
| `/404.html` | Helpful static missing-page page with base-aware links to home/guide/download. |

Use GitHub Releases for full release notes and archives rather than maintaining a second exhaustive changelog. No blog, account system, pricing, newsletter, live star counter or backend CMS is needed for the initial site. Keep content in Markdown/JSON so translation can be added later without adding an empty language switcher now.

### 5.2 Home page narrative and visual design

1. **Hero:** existing symbol/wordmark, “Branch into action.” and the descriptor “A context-aware radial action wheel for Linux files.” Supporting copy: “Bring actions to your cursor. Preview text, convert images, and run your own commands from a branching wheel.” Primary CTA **Download for Linux (.deb)**; secondary **Try the wheel**; adjacent text **“v0.3.0 · amd64 · X11. Primary tested setup: Linux Mint Cinnamon + Nemo.”** Use a verified current application screenshot or consented short recording. Offer Branching/Concentric visual comparison without implying Concentric is the default. Version comes from release data, not repeated hardcoded strings.
2. **Gesture introduction:** three short annotated steps: drag files in Nemo; tap Shift; choose an enabled action and release. Add “Prefer clicking or the keyboard? Use Open in Radivine.” Explain safe cancellation under the default setting. A static step sequence remains understandable without playing media.
3. **Practice area:** a bounded illustrated desktop surface with fixture files, context selector, Branching/Concentric controls, practice instructions and a readable outcome panel. Describe it as **“Browser simulation using sample files”** before interaction.
4. **Feature stories:** alternate screenshots and text for text preview, Quick/Advanced Convert, and custom actions/Control Center. Each story describes a concrete task and one relevant limit. Show Jobs as part of conversion rather than a disconnected infrastructure feature. Distinguish the wheel from the independent native windows it opens.
5. **Compatibility checkpoint:** primary tested setup, conditional X11 setups, and Wayland limitation visible in ordinary text; link the complete matrix. Keep this before the final download CTA.
6. **Development and contribution:** compact shipped-version timeline, link to exploratory roadmap and specific contribution routes. Finish with the package/source CTAs and project license/GitHub links.

**Distinctive direction:** a precise desktop-tool aesthetic with calm green surfaces, generous editorial spacing, fine radial guides and restrained connector stems. Use the branching node/leaf motif to connect explanations, with the real wheel as the main visual. Avoid stock artwork, invented desktop screenshots, giant decorative gradients, repeated feature-card grids, floating pricing-style panels, fake testimonials and performance counters.

**Palette:** seed tokens from the existing application palette (`#044C3C` vine, `#00A87C` leaf, dark canvas near `#0A1A16`, light canvas near `#F4FBF8`). Use semantic surface/text/border/action/focus/status tokens rather than hardcoding the palette in components. Application-muted colors may fail website text contrast; adjust website tokens after measurement. Use dark foreground on bright green controls when it passes contrast; do not assume white-on-green is accessible. Disabled, pending and unverified states need text/icons as well as color.

**Typography:** reuse Inter with a system-font fallback; use a system monospace stack for commands. Start body text at 16–18 px, comfortable line-height, prose width around 65–75 characters. Headings should be direct and compact. Optimize/serve fonts locally with the OFL notice, or start with system fonts if a smaller font asset is unavailable.

### 5.3 Existing assets, provenance and capture plan

| Existing asset | Inspection result | Intended use / condition |
|---|---|---|
| `assets/branding/logo-horizontal.png` and `logo-primary.png` | 1448×1086 RGB, about 1.09 MB each; opaque white background and generous margins. | Use unchanged on a white brand surface when appropriate; do not filter/invert or pretend it is transparent. |
| `assets/branding/logo-icon.png`, `assets/icons/app-icon.png` | 1254×1254 RGBA, 622,000 bytes each. | Existing transparent symbol for headers/dark surfaces. Pair with accessible HTML product name if needed; label that as site typography rather than a new official wordmark. |
| Other existing logo/icon variants | Mixed RGB/RGBA; the file named `app-icon-white.png` is RGB with an opaque background. | Inspect the actual pixels before choosing a variant; filename alone is insufficient. |
| `docs/screenshots/wheel-{root,2level,3level}.png` | Real desktop scenes with development paths, extra UI and older menu contexts. | Secondary historical/reference images until current clean captures are available; crop only in a separate website derivative after review. |
| `docs/screenshots/control-*.png` | 1120×720; inspected dashboard displays v0.1.1. | Do not label these as current v0.3.0 screenshots. Obtain fresh eight-page/Advanced Convert samples as needed. |
| `docs/screenshots/viewer-*.png` | 1020×700, dark/light preview examples. | Useful preview story candidates; check duplicates, sample text and version/provenance before publishing. |
| N3 concentric pack | 25 PNGs including contact sheet; indexed renderer/sandbox scenarios, custom labels and debug overlays. | Accurate engine visual references; caption “sandbox/rendered example,” not live Nemo capture. Use selected non-debug examples only. |
| `assets/wheel-icons/*.svg`, `assets/fonts/Inter-Variable.ttf` | Existing Lucide subset and ~857 KiB Inter TTF. | Reuse local icons and retain notices; subset/reformat only as permitted website derivatives, never replacing protected originals. |

Implementation should read originals through an explicit asset manifest, produce approved optimized copies only in `website/` or build output, and verify protected-file hashes before/after. No symlinks in the Pages artifact. Do not regenerate, rename or modify anything in `assets/branding/` or `assets/icons/`; preserve the existing banner as well. No new logo or AI-generated “Radivine UI” is needed.

Before launch, capture the actual stable version on a known Mint/Cinnamon X11 desktop with disposable neutral sample files: Branching text preview; Concentric image conversion path; Advanced Convert; Jobs; and Control Center layout setting. Record version, OS/desktop/session/scale, date, capture method and whether the frame is live UI, headless export or sandbox. Remove personal paths and unrelated applications through capture setup or approved website-only crops. Do not rewrite version labels in old images. The existing screenshot export can supply clearly labeled representative UI; it cannot establish live desktop compatibility.

## 6. Interactive wheel demonstration specification

### 6.1 Architecture choice and honesty boundary

Use one small TypeScript interaction controller with SVG for the wheel and HTML for controls, file fixtures, captions and outcomes. No React/Vue runtime is needed for this scope. Implement a bounded, deterministic subset of desktop behavior, with explicit parity fixtures and reviewer checks. A browser simulation is not an execution of the Rust daemon and must not claim pixel-perfect or exhaustive engine equivalence.

Keep all simulation code/data inside `website/`; do not add runtime dependencies to `radivine-wheel`, modify application source, or port desktop/worker integration into the browser. A WASM adapter would add build, renderer and accessibility work and should be considered only under a separately approved scope if fixture parity proves insufficient. If a behavior cannot be represented faithfully, omit that scenario or use a labeled static walkthrough until it is verified.

The demo never accepts actual OS file drops, requests filesystem access, uploads files, invokes native commands, changes the visitor's clipboard for simulated Copy, or performs image conversions. It uses named sample fixtures. Explicit download-command copy buttons elsewhere may use the clipboard after a user click. Unsupported external drops are rejected inside the demo area; listeners must not disable dragging or scrolling across the page.

### 6.2 State and behavior contract

State: selected fixture, invocation mode, requested/effective layout, logical viewport, menu generation, active submenu path, pointer/focus, pending enablement, dwell/intent timers, press origin, and result. Transitions: **idle → active → navigating → simulated result/cancelled → reset**. Use an injected clock for tests and a single input reducer; avoid independent DOM and keyboard state machines.

- **Default is Branching.** Child wheels open beside parents with a stem, neutral bridge and hover protection. Concentric opens full-circle outward annuli around one session center with top-centered sectors at each level. W3 source inspection confirmed that `ring_geometry` does not rotate children to their parent sector. Do not draw a generic pie chart where clicking any wedge immediately runs an action. [E05/E14]
- **Use application defaults, not engine-only defaults or example comments:** root radii 40/128 logical px; child radii 34/108; child gap 20; hover dwell 110 ms; submenu grace 180 ms; angular hysteresis 3.5°. Retain engine settings actually inherited by `to_wheel_config`, including activation slop, opening band and hub-close behavior. Verify those values together against a pinned source snapshot. [E04/E05]
- **Menu data:** derive fixture expectations from `default_menu`, visibility and enablement. For a PNG fixture, show Convert/Image targets and hide known non-text Preview. For one text file, show Preview/Full/Head/Tail and omit image-only Convert. Preview over multiple eligible text files is disabled with “Select a single file to preview.” Do not show every feature in every context. [E07/E08/E13]
- **Pending versus disabled:** a checking fixture may demonstrate pending-to-enabled progression; mark its timing as illustrative. Preserve visible disabled items and their actual reasons. A disabled/pending leaf cannot commit. Hidden items do not occupy sectors. [E07/E13]
- **Leaf activation:** drag practice commits only on release over the final eligible leaf; Invoke click honors valid press/release pairing; activating a submenu navigates rather than executes. Default submenus have no default action. Re-evaluate the current coordinates/generation at release; never use an obsolete hover selection. [E05/E06/E15]
- **Neutral zones:** the hub, separators, concentric inter-ring gaps, protected transition/approach zones, caption and outside area never commit an action. Distinguish Branching travel corridors, which preserve navigation, from actionable wedges. Safe outside release cancels the simulation. [E14/E15]
- **Concentric guards:** preserve fixed center after summon, inward navigation, sibling-switch intent/grace and fresh-motion exposure guard. Do not convert transient visual geometry into an action target. Include a displaced-center example only when its approach guard is implemented and tested. [E05/E14]
- **Keyboard:** within the focused Invoke practice surface, arrows cycle, Enter/Space activate, Backspace goes up, digits 1–9 activate the corresponding item, Escape cancels. Digits are activation, not just highlight. Keep Tab/Shift+Tab as browser focus navigation; disclose this web accessibility adaptation beside desktop instructions. Never install document-wide shortcuts or trap focus. [E05]
- **Drag practice:** start a synthetic pointer-held fixture drag inside the stage, then accept a fresh Shift press, show the wheel, and simulate release. Include an obvious Invoke/Start button so the gesture is optional. A second Shift press can hide the simulation; explain that the desktop application resumes native dragging. Do not claim the browser has intercepted an OS drag. Handle pointer cancellation/window blur by cancelling and releasing capture.
- **Results:** “Simulation: the desktop app would open Full Preview” or “Simulation: a WebP job would be queued.” Explain that actual Quick Convert is asynchronous; never display a fabricated completed native job. LogAction examples report “Example logs an action; configure a command to perform this operation.”
- **Fallbacks:** model Concentric’s default 668×742 logical footprint and compositor/target-size conditions. A wide stage may use a labeled virtual desktop with scaled presentation; preserve its logical coordinates and avoid unreadable tiny targets. On narrow screens offer tap/Invoke step-through plus the accessible action list. Explain when a virtual viewport is being used. Do not imply the desktop wheel supports touch because the website does.
- **Reduced motion:** preserve state/intent timing while removing travel/scale effects. If the simulation hides or leaves the viewport, stop animation work and cancel stale input/timers. Layout/context changes reset the practice session rather than remapping an active target silently.

### 6.3 Minimum parity fixtures and release gate

Store fixture version, source revision, logical viewport, application-derived configuration, expected menu/action IDs, timestamped inputs and expected events/results. Map each case to existing Rust test names and an approved visual reference:

| Fixture | Expected result | Rust evidence |
|---|---|---|
| Text, one file / image, one file / multiple text files | Correct visible tree, reasons and sample context | `test_preview_visibility_and_enablement_matrix_ac_4_1`, `test_ac_c3_1_menu_structure_for_image_and_text_snapshot_parity`, `test_ac_c3_2_visibility_and_progressive_enablement_matrix` (E13). |
| Branching and concentric submenu travel | Open/retain/close correct hierarchy; no accidental commit | `scenarios`; `t_nav_normative_concrete_deterministic_trace`, `t_nav3_radial_gap_diagonal_crossings_preserve_child_and_reject_drop` (E14). |
| Release in hub/gap/caption/outside; disabled leaf; moved before release | Cancel/reject, source unchanged in real app tests | `t_safe1_to_t_safe5_strict_zone_rejection_enablement_default_child_generation_and_layout_parity` (E14); `t_safe1_t_safe2_concentric_drag_commit_and_non_actionable_zone_drops` (E15). |
| Invoke keyboard and pointer takeover | Correct activation/back/cancel; valid web focus exit | `t_nav9_invoke_keyboard_navigation_pointer_takeover_and_invoke_select_modes` (E14), with documented Tab adaptation. |
| Small virtual desktop / no compositor / reduced motion | Branching fallback or reduced animation, accurate caption | E05 layout selection; E15 session guards; `t_nav11_and_t_nav12_interrupt_tweens_every_10ms_and_reduced_motion` (E14). |
| Simulated conversion / default custom demo | Queued explanation / logging explanation, never fake native execution | C3 acceptance and actual `LogAction::run` (E07/E13). |

Before shipping, compare the representative interaction paths to an actual stable desktop session or a verified recording as well as these source/test fixtures. If simplified intent/animation behavior cannot pass the stated cases, the demo does not pass its fidelity gate. A maintenance check must flag fixture-source drift when the product version changes; it must not silently relabel old fixtures as a new version.

## 7. Technical architecture

### 7.1 Project structure and dependencies

Proposed structure only; none of these files are created by this task:

```text
website/
  package.json, package-lock.json, astro.config.mjs, tsconfig.json
  README.md                       # local build, content/release and asset procedures
  public/                         # unchanged brand copies, favicon, notices, robots output inputs
  src/
    content.config.ts             # validated Markdown/JSON content collections
    layouts/SiteLayout.astro, GuideLayout.astro
    pages/index.astro, download.astro, compatibility.astro, roadmap.astro
    pages/contribute.astro, licenses.astro, 404.astro
    pages/guide/index.astro, guide/[...slug].astro
    components/layout/            # Header, Footer, Navigation, ThemeSelect
    components/product/           # Hero, GestureSteps, FeatureStory, ScreenshotFigure
    components/data/              # ReleaseDownload, InstallCommands, SupportMatrix, RoadmapList
    components/demo/              # WheelDemo.astro, DemoControls, OutcomePanel, ActionList
    data/                         # evidence, features, compatibility, roadmap, release, assets JSON
    content/guides/               # reviewed English Markdown
    lib/urls.ts, schemas.ts        # URL normalization and shared validation
    scripts/theme.ts, copy.ts      # progressively enhanced controls
    demo/model.ts, geometry.ts, view.ts, controller.ts
    styles/tokens.css, global.css
  scripts/                        # content/link/asset/release verification helpers
  tests/unit/, tests/e2e/, tests/fixtures/
.github/workflows/website.yml      # NEW, future website-only workflow
```

Select a stable Astro release and a compatible supported Node LTS at implementation time; pin Node and lock dependencies. Use Tailwind 4 with `@tailwindcss/vite`, strict TypeScript, Astro's static output, and an npm lockfile (`npm ci`). No server adapter is required. Prefer ordinary `.astro` components and scoped CSS/Tailwind tokens, with small bundled TypeScript scripts for interaction; no global client router or framework hydration for static sections. The official [Astro styling guide](https://docs.astro.build/en/guides/styling/) documents the Vite plugin integration.

Suggested tooling: `astro check`/TypeScript, Prettier, ESLint for authored scripts, Vitest for meaningful state/schema/URL tests, Playwright plus axe for page behavior/accessibility, and a build-output link checker. Pin compatible versions when implementation begins. Keep Node tooling and cache/outputs in `website/`; do not alter the Cargo workspace or add generated web files to application packaging.

### 7.2 Component responsibilities

`SiteLayout` owns metadata, theme initialization, skip link and shared navigation. Product components consume validated feature/media records. `ReleaseDownload` consumes one reviewed release manifest and is reused by every CTA/command block. `SupportMatrix` renders all rows as static HTML before optional filtering. `RoadmapList` separates shipped facts from proposals. `ScreenshotFigure` requires provenance/caption/alt text. `WheelDemo` renders a useful static illustration and hierarchical action explanation before its isolated controller is loaded near visibility.

Data flows **reviewed repository evidence → build-time validation → static pages → optional local interaction**. No runtime GitHub calls are necessary for visitors. All filters preserve links and content when JavaScript is disabled. Avoid embedding technical support-status enums or fixture internals in visitor-facing wording.

### 7.3 Content and metadata models

Use Astro build-time content collections for guides and structured local records; validate all data with Zod or an equivalent shared schema. The [Astro content collections guide](https://docs.astro.build/en/guides/content-collections/) establishes `src/content.config.ts`, build-time loaders and schemas. Suggested contracts (design, not production code):

```ts
type Evidence = {
  id: string;
  kind: 'source' | 'test' | 'manual-report' | 'documentation' | 'release';
  path?: string; symbol?: string; revision?: string; url?: string;
  observedOn: string; productVersion: string; scope: string;
};
type Verification = {
  method: 'source-inspection' | 'unit-property' | 'fake-platform' |
          'headless-integration' | 'xvfb' | 'recorded-manual' | 'live-release-metadata';
  evidenceIds: string[]; environment?: string; date: string;
};
type Feature = {
  id: string; title: string; summary: string;
  implementation: 'implemented' | 'partial' | 'absent';
  since?: string; constraints: string[]; verification: Verification[];
};
type CompatibilityRow = {
  id: string; os: string; osVersion?: string; architecture: string;
  desktop?: string; desktopVersion?: string;
  session: 'x11' | 'wayland-xwayland' | 'wayland-native' | 'other';
  compositor?: string; fileManager?: string;
  capabilities: Record<'drag' | 'invoke' | 'concentric' | 'properties', {
    availability: 'available' | 'conditional' | 'unavailable' | 'unknown';
    qualification: string; verification: Verification[];
  }>;
  packageStatus: 'published' | 'install-tested' | 'source-only' | 'none' | 'unknown';
  publicLabel: string; lastReviewed: string; manualChecks: string[];
};
type RoadmapItem = {
  id: string; title: string; summary: string;
  status: 'shipped' | 'approved' | 'exploring' | 'deferred';
  evidenceIds: string[]; issueUrl?: string; lastReviewed: string;
  shippedVersion?: string; releasedOn?: string;
  approvalEvidenceId?: string; targetVersion?: string;
};
type Release = {
  version: string; tag: string; publishedOn: string; verifiedOn: string;
  releaseUrl: string; sourceRevision: string; channel: 'stable' | 'prerelease';
  assets: { name: string; url: string; bytes: number; sha256: string;
            os: 'linux'; architecture: 'amd64'; package: 'deb' }[];
  checksumsUrl: string; notesUrl?: string; verification: Verification[];
};
type MediaAsset = {
  id: string; sourcePath: string; sourceHash: string;
  width: number; height: number; alt: string; caption: string;
  kind: 'live-desktop' | 'headless-export' | 'sandbox' | 'brand';
  productVersion?: string; environment?: string; capturedOn?: string;
  license: string; derivativePath?: string;
};
```

Page/guide frontmatter: `title`, `description`, `slug`, `lang: en`, `updatedOn`, `productVersion`, `evidenceIds`, optional `socialImageId`, and draft status. Canonical URLs are computed from route/config, not hand-entered per article. Keep authored Markdown free of embedded arbitrary scripts; use components for commands and callouts.

Validation must reject missing evidence references, duplicate IDs, invalid dates/URLs/checksums, unsupported package architectures, draft/prerelease selection as the default stable download, and unknown media paths. A “recorded manual” result requires an environment/report. “Shipped” roadmap items require a real released version; “approved” or a target version requires approval evidence. No target dates are required for exploratory items. Do not collapse the availability and verification fields into a single “supported” boolean.

### 7.4 Release-content maintenance

Start with a checked-in, reviewed release manifest for v0.3.0. Populate actual asset names/URLs/digests from GitHub API metadata and cross-check with `SHA256SUMS.txt` and package metadata in the release checklist. Generate all install commands from this record. Link the pinned source revision; distinguish stable from pre-release and expose older versions through GitHub Releases.

A maintainer updates the manifest, feature/compatibility evidence, media and demo fixtures in a normal PR when a stable release ships. A read-only validation script may compare remote metadata and report a new release or mismatch; do not make builds automatically publish new claims or fetch “latest” at visitor runtime. The default build remains reproducible from checked-in data when the API is unavailable. Deployment smoke checks independently verify live download/checksum links. Record verification dates and make a failed link check actionable without deleting the last good download record.

### 7.5 Theme management

Use a labeled native select or accessible three-option control: **System**, **Light**, **Dark**, default System. Persist only the preference in a namespaced localStorage key. A minimal early head script validates the stored value, catches storage failures, resolves `prefers-color-scheme`, and sets an effective `data-theme` and `color-scheme` before paint. Keep the user's preference separate from the effective theme; react to OS changes only while System is selected and sync preference across tabs.

CSS media queries provide a System fallback when JavaScript is unavailable. Invalid/blocked storage falls back to System. Use semantic CSS variables and one Tailwind dark selector tied to the effective data attribute. The [Tailwind dark-mode documentation](https://tailwindcss.com/docs/dark-mode) supports selector-driven theme overrides. Disable unnecessary theme transition animation during initialization and honor reduced motion. Theme changes do not alter the sample file context or pretend to change the native application's settings. Verify first paint, reload/navigation, OS changes, storage errors, form controls and all three settings.

### 7.6 GitHub Pages URLs and base-path handling

Initial expected origin is `https://marawan6569.github.io`, with Astro `base: '/Radivine'`, static output and directory-style trailing-slash routes. This is a proposed URL, not a claim that Pages is already enabled. Set one deployment configuration source for origin/base; keep it overridable for a future custom domain, which would use its own origin and root base.

Use a tested `withBase(path)` helper for every internal route, public asset, download-page anchor, navigation item and 404 link. Normalize leading/trailing slashes exactly once. Use imported assets for generated image URLs; never double-prefix Vite/Astro output URLs. Leave external GitHub links unchanged. Generate absolute canonicals/social URLs/sitemap entries from the same origin and base. Test both `/` and `/Radivine/`, nested direct loads, hash links, encoded asset names, social images and the custom 404. Avoid a SPA history fallback. The official [Astro GitHub Pages guide](https://docs.astro.build/en/guides/deploy/github/) requires repository-base handling for project sites; [Astro configuration](https://docs.astro.build/en/reference/configuration-reference/) defines static output and URL configuration.

Keep robots/sitemap scope correct for a project site: `robots.txt` under `/Radivine/` is not the origin-root robots policy. Do not assume control of `marawan6569.github.io/robots.txt`. Produce a valid project sitemap and correct canonicals regardless; establish origin-root crawler policy only if the owner actually controls that site.

### 7.7 Future deployment workflow — isolated from Rust CI/releases

Add **only a new** `.github/workflows/website.yml` after implementation authorization. Do not edit `.github/workflows/ci.yml`, release scripts, tag semantics, package artifacts or existing branch protections. The current Rust workflow triggers on all PRs and main/feature pushes; it will still run on website-only changes. That extra CI activity is expected; removing it is a separate maintenance proposal.

Proposed workflow contract:

1. Trigger on `pull_request` and `push` to `main` for `website/**`, the new workflow, and explicitly consumed branding/icon/font/screenshot/license/release inputs; include `workflow_dispatch` for a controlled rebuild. Record the exact input paths in `website/README.md`. Do not use application release tags, a generic `workflow_run`, or a schedule to publish automatically.
2. **Build/check job:** checkout; pinned compatible Node; npm cache keyed to `website/package-lock.json`; `npm ci`; format/lint/type/content checks; unit tests; production build; internal link/asset checks; Playwright smoke/axe tests. Use `working-directory: website` for run steps and explicit `website/dist` paths for actions, because action `uses` steps do not inherit shell working directory. Only `contents: read`; no release write permissions, deployment credentials or repository mutations in PR jobs.
3. Upload test results as ordinary artifacts. Upload the Pages artifact from **only `website/dist`** for an eligible main-branch build. It must contain no source/config secrets, Rust binaries, `target/`, Node cache, node_modules or symlinks.
4. **Deploy job:** `needs` the verified build; run only for main-branch push/manual dispatch from the canonical repository. Use job-scoped `pages: write` and `id-token: write`, GitHub's Pages configure/upload/deploy actions, and the `github-pages` environment with the deployment output URL. Resolve supported action versions and pin reviewed commit SHAs during implementation. Main-branch guard must also cover manual dispatch so selecting a feature branch cannot publish.
5. Give deployment its own concurrency group (for example `radivine-website-pages`) separate from Rust jobs/releases; serialize deploys and cancel superseded PR checks independently. Do not cancel an in-progress application release or let a fork PR publish. Use the normal `pull_request` event, not `pull_request_target` with untrusted builds.
6. Before first publication, confirm Pages source is GitHub Actions, environment branch rules and the intended URL with the maintainer. Preview/build artifacts are reviewable before publishing. After explicit publication authorization, deploy, verify nested routes/assets/themes/downloads anonymously, and record URL, source SHA and checks. Rollback redeploys a previously approved website commit; it does not rewrite Git or touch application releases.

GitHub's [custom Pages workflow documentation](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages) documents artifact deployment, dependent jobs, environment and deployment permissions. This plan intentionally specifies responsibilities without creating an executable workflow.

## 8. Responsive design, accessibility, SEO and performance

### Responsive behavior

- Use content-driven CSS grid/flex layouts: single column at narrow widths, two-column hero/stories where text and images fit, bounded reading width at large sizes. Test 320, 390, 768, 1024 and 1440 CSS px plus landscape, 200%/400% zoom and long content.
- Collapse navigation into a keyboard-operable disclosure without a focus trap; use a normal link flow for non-JS fallback. Do not let a sticky header hide anchors/focus.
- Render support data as a semantic table with caption and scoped headers; provide a compact per-environment presentation or labeled overflow region on mobile without losing evidence/qualifications. Do not hide limitation columns on small screens.
- Stack commands with copy buttons that do not cover text; permit horizontal scrolling within code blocks. Reserve image aspect ratios and do not crop away relevant menu states. On mobile make the accessible demo action list and Invoke controls primary; show static comparison frames if wheel targets cannot remain usable.

### Accessibility requirements

Target **WCAG 2.2 AA**, assessed through automated checks plus manual keyboard/screen-reader review; do not advertise certified conformance based on axe alone. Use landmarks, one H1, logical headings, skip link, visible focus, meaningful link labels, form labels, image alt text/captions and native controls. Ensure keyboard access and no traps; custom demo interactions cannot block Tab navigation or capture browser shortcuts globally. Provide button/tap alternatives to drag gestures. The [WCAG 2.2 recommendation](https://www.w3.org/TR/WCAG22/) is the reference for contrast, keyboard, dragging alternatives, focus and target-size requirements.

Measure 4.5:1 body-text contrast, 3:1 large text and relevant UI boundaries/focus indicators. Prefer 44×44 CSS px touch controls while meeting applicable WCAG minimum target/spacing rules. Support forced colors, increased text size, reduced motion and high zoom. Do not rely on green/gray differences to convey support or disabled states.

Expose the demo as a labeled region with native controls and a synchronized hierarchical HTML action list; treat the SVG as decorative when it duplicates that accessible representation. Keep disabled reasons readable, pending states named, and outcomes in a polite live region. Do not announce every pointer frame. Provide focus restoration after reset/cancel. Test NVDA/Firefox or Chrome and VoiceOver/Safari, plus keyboard-only use. Touch operation demonstrates the website, not a new desktop capability.

If a video is added, use a poster, explicit play/pause, text transcript and captions for spoken instructions; do not autoplay essential motion. The user must understand the product without video, hover, fine pointer input or JavaScript.

### SEO and privacy

English `lang`, unique titles/descriptions, canonical URLs, Open Graph/Twitter metadata, social preview with existing branding, sitemap, favicon and correct static 404. Use accurate `SoftwareApplication` structured data for the released Linux desktop application, repository/license/download/version fields supported by the chosen vocabulary, and no fabricated reviews, ratings or cross-platform OS list. Validate structured data and absolute URLs before launch. Base-aware static HTML must include the main product copy, compatibility table and install steps for crawlers.

Link licenses/notices and contribution/security reporting. No analytics, tracking embeds, cookies, third-party font CDN or account flows are needed initially. State that theme preference is stored locally and that GitHub hosts the site and download destinations; do not make an absolute “no data ever collected” claim about hosting. Keep external network requests out of the demo. Any later analytics proposal requires a separate privacy/content decision.

### Performance budgets and implementation constraints

Treat these as website targets to measure during implementation, not existing results:

| Metric | Initial acceptance target |
|---|---|
| Initial JavaScript excluding opt-in demo | ≤20 KiB gzip on the home page; no framework runtime |
| Demo code/data loaded near interaction | ≤60 KiB gzip, excluding images; split from global shell |
| Initial home transfer | ≤500 KiB compressed at a representative mobile viewport; full-resolution gallery/video deferred |
| Visual stability | CLS ≤0.1; explicit dimensions for images, demo and media |
| Loading/responsiveness | Target LCP ≤2.5 s and INP ≤200 ms in field data when available; use controlled lab interaction tests as an interim proxy |
| Lab audit | Median of three mobile Lighthouse runs ≥90 performance, with accessibility findings reviewed separately |

Responsive AVIF/WebP screenshot derivatives with PNG fallbacks, lazy loading below the hero, one prioritized hero image, local fonts with `font-display: swap`, and a minimal icon subset. Avoid shipping the 25-image sandbox pack or original megabyte logos as a default gallery payload. Use approved build-time image processing; protected originals stay byte-identical. [Astro's image guide](https://docs.astro.build/en/guides/images/) covers build-time responsive image handling.

Animate only while a state transition is active; stop offscreen work. Avoid expensive backdrop filters and constant animation loops. Reserve server-generated HTML for all content; no visitor API dependency, service worker or full client-side navigation framework is required.

## 9. Proposed implementation phases and acceptance criteria

**No phase below is authorized by this planning task.** Names W0–W5 distinguish website work from existing Rust milestone numbers. Finish and verify each approved phase, commit focused changes, then stop at its scope boundary unless the user explicitly authorized multiple phases. Deployment remains a separate publication decision.

| Phase | Scope / deliverable | Acceptance criteria and review gate |
|---|---|---|
| **W0 — Content and visual contract** | Agree positioning, release snapshot, evidence-qualified support labels, sitemap and desktop/mobile visual composition; asset manifest and current capture checklist. | Every featured claim has evidence; default logging actions and Wayland caveats explicit; screenshot versions/provenance identified; primary CTA requirements agreed; no product roadmap promises invented. Current hero image is approved or an honest existing reference is selected. Review content/design before building. |
| **W1 — Static foundation** | Scaffold only `website/`; Astro/Tailwind/Node lock, tokens, layouts, navigation, metadata, theme and base helpers. | Static root/subpath builds pass; Light/Dark/System works before paint and across navigation; keyboard/skip/focus flow usable; 320 px and zoom usable; no-JS nav/content fallback; protected hashes unchanged. No deployment or changes to Rust/package/release workflow. |
| **W2 — Product content and installation** | All planned core pages, source/evidence models, release manifest, compatibility and roadmap, real media/captions, install/first-run instructions. | Schemas and local links pass; stable asset name/digest cross-checked live; commands tied to manifest; static content complete with JS disabled; mobile matrix retains caveats; no stale version labels passed off as current; no unsupported download buttons. Review product truth before demo polish. |
| **W3 — Faithful interactive demo** | Branching and Concentric simulation, fixture contexts, pointer/Invoke practice, keyboard/tap/action list, disabled/pending/cancel/fallback/outcome paths. | Minimum parity cases in §6 pass with fake clock; visual and desktop/recording comparison reviewed; inaccessible tiny wheel avoided; no real file/clipboard/native execution; no global shortcut interception or focus trap; label/browser adaptations visible; reduced-motion/offscreen cleanup verified. |
| **W4 — Release candidate validation** | E2E/axe/link/base/SEO/performance checks; manual browser/accessibility/media/download checks; proposed website workflow in a reviewable change, publication held. | Tests in §10 pass or limitations are explicitly accepted; protected/source/CI diffs reviewed; package UI and website version align; no known broken internal routes; budgets measured; website test report and deployment/rollback instructions committed. Existing Rust checks assessed without expanding application scope to fix unrelated failures. |
| **W5 — Authorized publication** | Enable/confirm Pages settings, merge through normal repo workflow, deploy approved website, run anonymous smoke checks and record live URL/SHA. | Only main deploys; fork PR has no deploy permission; artifact contains static website only; `/Radivine/` and nested guide/download/404/assets work; canonical/sitemap correct; download/checksum links work; website rollback documented; Rust workflow/release assets unchanged. Stop after deployment report. |

Focus commits on independently reviewed work: data/schema foundation; layout/themes; static content; demo state machine; accessibility/fidelity fixes; website workflow; validation documentation. Follow Conventional Commits. Do not push without explicit authorization. The deployment approval must refer to a concrete, tested build, URL and workflow; this document is approval material for planning only.

## 10. Testing and verification strategy

### Website automated checks to implement later

1. **Build/type/content:** `npm ci`, format/lint, `astro check`, schema validation and production build. Validate all evidence/media paths, release manifests, guide metadata, roadmap status rules and generated internal links/fragments.
2. **Unit tests where behavior matters:** base helper for root/project paths/external URLs; theme preference fallback/OS updates; demo reducer, timers, hit regions, stale state rejection and menu filtering; representative release/roadmap validation failures. Do not snapshot every presentational component or duplicate implementation details.
3. **E2E against built output:** Chromium, Firefox, WebKit; no-JS/static navigation/install flow; keyboard/arrow/digit/back/cancel; synthetic pointer drag; touch/Invoke alternatives; theme persistence/system changes/blocked storage; responsive layouts; clipboard denial fallback. Check server requests stay within the expected base and console has no unexpected errors.
4. **Accessibility:** axe on all routes/themes and representative demo states; manual focus/contrast/zoom/forced-color/reduced-motion review. Include screen-reader walkthrough of data tables, commands, disabled reasons and result announcements.
5. **SEO/link/artifact:** nested route hard loads, missing-page behavior, canonical/social/sitemap URLs, structured data, release notes/download/checksum links, image dimensions and license notices. Scan `dist` for secrets/absolute local paths, source maps with unwanted content, Rust/build artifacts and symlinks.
6. **Performance:** measure mobile initial transfer and JS budgets, three-run Lighthouse median, demo interaction under CPU throttling and long-session listener/timer cleanup. Native performance benchmarks are linked context, not a proxy for web performance.
7. **Workflow isolation:** run PR build with read-only permissions; inspect conditional deployment/main/manual-dispatch/fork guards; ensure cache/artifact/concurrency names are website-specific. Verify existing `.github/workflows/ci.yml` and release tooling unchanged. No live publication to test a PR.

Build twice in test configurations: root-hosted `/` and project-hosted `/Radivine/`. Serve output under the corresponding prefix, not just a development server that hides path mistakes. External link checking should distinguish rate limits/transient errors from missing release assets, with bounded retries and an explicit manual verification result.

### Required manual product and website checks

| Check | Purpose / evidence to record | Blocks what |
|---|---|---|
| Current stable capture on Mint 22.3 Cinnamon X11/Nemo | Version, configuration, scale, sample files, real wheel/convert/viewer behavior, capture provenance | Unqualified v0.3.0 hero/demo visual claims |
| Remaining N5 desktop reject-zone/edge/multi-monitor and composited-performance sub-cases | Exact case/result and environment; owner general sign-off is retained separately | New exhaustive safety/performance claims, not honest primary-tested wording |
| XFCE/MATE/GNOME Xorg/KDE X11 and each named file manager | Drag/invoke/Properties/clipboard/compositor/startup outcomes independently | Promotion from documented candidate to verified matrix row |
| XWayland Invoke and native Wayland GUI tools | Compositor/session, usable X11 display, position/input/window behavior; record wheel separately from control/viewer/worker | Any claim of operational Wayland fallback beyond current qualification |
| Distribution/architecture/filesystem variants | Actual install and GUI/runtime dependencies, CPU behavior, output semantics | New platform/package/portable filesystem claims |
| Source toolchain and TOML examples | Test actual supported compiler and parser behavior at the advertised release | Minimum Rust promise; copyable configuration guide snippets |
| Screenshot/privacy/license review | No personal paths/secrets; accurate version and live/export/sandbox labels; unmodified brand assets | Public asset readiness |
| Keyboard/screen reader/touch and Safari/mobile review | Complete download/guide/demo tasks, focus exit and announcements | Accessibility release sign-off |
| Pages configuration/domain and anonymous smoke check | Canonical URL, nested routes, actual assets/checksums, environment branch rules | Publication |
| Security support-version policy and roadmap wording | Maintainer confirms maintained versions and whether any proposal is actually approved | Security lifetime/approved-next-release claims |

These checks are not all prerequisites to publishing a carefully qualified site. A missing result blocks the corresponding stronger claim; keep it visibly unverified instead of manufacturing a compatibility guarantee. Actual first-deployment and website usability checks remain launch gates.

## 11. Risks, decisions and maintenance

| Risk / unknown | Mitigation and decision |
|---|---|
| Documentation drifts ahead of code or lags completed work | Version the evidence registry and review claims against shipping code/tests/releases; do not wholesale import old vision/planning docs. |
| Browser wheel diverges from Rust interaction | Bounded fixture specification, event-trace tests, current visual comparison, clear simulation label; block inaccurate scenarios instead of expanding product code. |
| Sparse desktop matrix looks like a broad support promise | Separate availability from verification and expose “manual check required”; collect reproducible community reports. |
| New release leaves stale downloads/demo/media | One release manifest plus a PR checklist that reviews feature/support/demo/media data together; verify live assets at publication. |
| Existing imagery misrepresents v0.3.0 | Current capture work with provenance; historical captions and sandbox labels until then. |
| Brand/background or bundle-weight constraints | Reuse transparent symbol, appropriate white logo surface and approved website-only derivatives; measure contrast and payload. No protected source changes. |
| GitHub project-base or custom-domain migration | Single URL configuration, root/subpath builds, deep-link/404 tests; domain decision is pending, not a reason to hardcode URLs in components. |
| Website deployment affects existing automation | New isolated workflow and artifact only; no edits to Rust CI/package/release paths; retain existing required checks. |
| Static deployment cannot run server-side jobs | Build-time content and local interaction only; GitHub hosts releases/issues. Future dynamic services require a separate architecture decision. |
| Compiler drift causes unrelated Rust lint failures | Use established CI compiler for application verification; report newer-toolchain failures without making unauthorized renderer changes. |
| Maintenance burden grows | Keep launch small, English-only, no CMS/runtime API; document release/content updates, evidence dates and asset approvals. |

Decisions to resolve at W0/W4 review: final navigation/copy and visual composition; which current desktop captures are publishable; confirmed origin/base/custom domain; which roadmap proposals, if any, have current approval; and security support-version wording. Sensible defaults above allow implementation planning without inventing answers. No user input is required to complete this research deliverable.

## 12. Planning-task verification and Git record

This section records only checks performed for this documentation task; it must not be read as a fresh desktop certification.

- Initial Git tree was clean; existing release/milestone history was already committed. Latest product tag targets `1aa009a`; the audited documentation baseline is `35a5e5f`.
- Live GitHub metadata checked: public repository/default branch `main`, release history, latest stable version and asset name/size/digest. Binary download/install and desktop acceptance were not repeated.
- Original branding, icon and screenshot files were read only. No website code, application source, packaging, existing CI or release workflows were modified.
- `cargo fmt --all --check`: passed with installed Rust 1.99.1. Cargo is installed under `/home/mfazora/.cargo/bin`, outside this shell's initial PATH; commands used its explicit path.
- `cargo clippy --workspace --all-targets -- -D warnings`: failed at the pre-existing `chunks_exact_mut(4)` renderer loop (`crates/radivine-render/src/lib.rs:1392`, `clippy::chunks_exact_to_as_chunks`). CI uses 1.95.0; no unrelated code change or lint suppression was made.
- `cargo test --workspace -- --test-threads=1`: failed headlessly (DISPLAY unset) at `apps/radivine-control/tests/arabic_font_fallback.rs`: `arabic_02`, `arabic_03`, `arabic_04` could not resolve Arabic glyphs. The expected DejaVu font paths and `fc-list` utility are absent here; the exact font-provider environment needs investigation before calling this a product regression. Earlier action, configuration, session and preview suites passed, but the full run stopped at this failing binary, so later workspace suites were not verified by that command. No system packages, font assets or application code were changed to repair the environment.
- Focused headless check: `DISPLAY="" cargo test -p radivine-wheel -p radivine-actions -p radivine-convert -p radivine-convert-image -p radivine-platform -- --test-threads=1` **passed**: 118 reported passing tests, 0 failed, 0 ignored. Xvfb/live desktop and long ignored soak/fuzz suites were not run in this planning audit. Tests guarded by an isolated-display check can report success after an early return; they are not counted as fresh live X11 evidence.
- All local evidence links resolve; all referenced evidence IDs are defined; Markdown fences and table columns validate; `git diff --check` passes; no `file://` references were introduced. File-scope review against `35a5e5f` shows only `docs/website-plan.md` changed. Application, protected assets, packaging and existing workflows are unchanged.

Git history for this task:

- `d2e205f` — `docs(website): audit shipped features and compatibility evidence`.
- Final proposal commit — `docs(website): define UX architecture and phased delivery plan`; its hash is included in the delivery report.

Planning commits are local on the existing `feat/website` branch. No push or publication is authorized. Stop after the completed plan is presented for review.
