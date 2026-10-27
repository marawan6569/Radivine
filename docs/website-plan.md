# Radivine official website — repository audit and implementation plan

**Status:** Research and proposal only; website implementation and deployment require separate authorization.

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
| Release/README low-level security phrasing | Packaged worker unit does not set `PrivateNetwork=yes`; source reader named `open_source_nofollow` follows valid symlinks and opens with `RDONLY | CLOEXEC`. | Describe tested no-overwrite outputs and out-of-process execution; avoid copying inaccurate flag or sandbox claims. |
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
| Arabic/non-Latin text | System font fallback and shaping tested; widget containers remain LTR. Not evidence of a translated Arabic UI or full RTL layout. | E11; `arabic_font_fallback` tests; E16 and release limits. |
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

## Audit verification note

Formatting (`cargo fmt --all --check`) passed using the installed toolchain. Strict Clippy on Rust 1.99.1 failed at the pre-existing `chunks_exact_mut(4)` loop in `crates/radivine-render/src/lib.rs:1392` (`clippy::chunks_exact_to_as_chunks`, elevated by `-D warnings`). CI pins Rust 1.95.0. No Rust code was changed to address that unrelated failure. Local evidence links resolve. Full workspace testing has been started; its outcome will be recorded with the completed architecture proposal.
