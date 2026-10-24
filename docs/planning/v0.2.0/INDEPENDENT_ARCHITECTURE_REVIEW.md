# Radivine v0.2.0 — Independent Architecture & Security Review (Source-Grounded Correction Pass)

- **Status:** Independent technical and security audit; **not an implementation authorization**
- **Date:** 2026-10-24
- **Inspected Git Commit (`git rev-parse HEAD`):** `872d5d4f59ec801cdff854a70dead958c1327ee2` (`tag: v0.1.1`)
- **Inspected Branch & Working Tree (`git status --short --branch`):** `## main...origin/main` (tracked working tree clean; untracked directories: `?? .idea/`, `?? docs/planning/`)
- **Inspected Planning Suite (`docs/planning/v0.2.0/`):**
  - [README.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/README.md) (lines 1–40)
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md) (lines 1–783)
  - [CONVERSION_BACKENDS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_BACKENDS.md) (lines 1–202)
  - [CONVERSION_PRODUCT_SPEC.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_PRODUCT_SPEC.md) (lines 1–222)
  - [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md) (lines 1–247)
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md) (lines 1–384)
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md) (lines 1–143)
  - [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md) (lines 1–224)
  - [IMPLEMENTATION_PLAN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/IMPLEMENTATION_PLAN.md) (lines 1–316)
  - [Radivine_v0.2.0_Architecture_Review.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/Radivine_v0.2.0_Architecture_Review.md) (lines 1–107)
  - `docs/planning/v0.2.0/IMPLEMENTATION_HANDOFF.md` — **Verified missing** from the workspace (listed in [README.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/README.md#L32) and [IMPLEMENTATION_PLAN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/IMPLEMENTATION_PLAN.md#L4), not present on disk).

---

## 1. Factual Corrections and Retractions

The initial pass relied on a compacted session summary that contained corrupted descriptions of several planning documents and source files. Following a direct line-by-line re-read of the repository source and all planning documents on disk, the following claims from the initial review are **formally retracted and corrected**:

### 1.1 Retraction A — ADR Numbering, Titles, and Owner Decisions (`D1`–`D9`)
- **What was wrong:** The first review used a scrambled table of ADR titles (`ADR-C01` through `ADR-C18`) and decision IDs (`D1`–`D9`) that did not match [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md).
- **Verified Fact:** [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L8-L28) defines exact identifiers `C01`–`C18` (e.g., `C01` = Conversion is an Action capability; `C02` = Crate boundaries: `radivine-convert`, `radivine-convert-image`, `radivine-jobs`, `radivine-worker`; `C03` = Capability-based backends; `C04` = Built-in image backend; `C05` = libwebp linkage; `C06` = Out-of-process, on-demand worker in its own systemd cgroup) and owner decisions `D1`–`D9` ([ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L204-L216)). Sections 3 and 4 below evaluate the exact original ADRs and owner decisions verbatim.

### 1.2 Retraction B — WebP Lossy Backend Strategy
- **What was wrong:** The first review falsely claimed that the Opus planning documents selected an `image 0.25`-only pure-Rust stack and overlooked that `image::codecs::webp::WebPEncoder` only supports lossless WebP.
- **Verified Fact:** The Opus planning documents **explicitly identified and solved this exact limitation**:
  - [CONVERSION_BACKENDS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_BACKENDS.md#L28-L39) (§2.1) explicitly notes: *"WebP encode: Lossless only. The only constructor is `WebPEncoder::new_lossless`... the `image` crate alone cannot produce lossy WebP."*
  - [CONVERSION_BACKENDS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_BACKENDS.md#L100-L129) (§4–§5) and [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L61-L85) (`ADR-C04`, `ADR-C05`) specify a **hybrid architecture** in `radivine-convert-image`: all untrusted image **decoding** uses memory-safe pure-Rust decoders (`image` 0.25 with `png`, `zune-jpeg`, `image-webp`), lossless WebP encoding uses `image-webp`, and **lossy WebP encoding uses `libwebp`** (vendored static linkage via `libwebp-sys` ≥ 1.3.2 behind a minimal safe wrapper, pending owner decision `D2` and C1 verification spikes `P1`/`P2`).
  - The false claim is retracted in full. The proposed hybrid pure-Rust decode + `libwebp` lossy encode design is preserved.

### 1.3 Retraction C — Existing `ActionRegistry` and `MenuSpec` Architecture
- **What was wrong:** The first review claimed `ActionRegistry` used a method named `build_menu_tree`, synthesized nested submenus from dot-separated `ActionId` segments, and required three registry hooks (`sort_key`, `hub_icon`).
- **Verified Fact:** In [`crates/radivine-core/src/registry.rs`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L13-L33), menus are defined declaratively using [`MenuSpec`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L13-L33) (`MenuSpec::Action(ActionId)`, `MenuSpec::Submenu { id, label, icon, description, children, default }`, `MenuSpec::CustomActions`). [`ActionRegistry::build_menu`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L182-L227) builds the [`MenuTree`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-wheel/src/menu.rs#L276-L320) and [`ActionIndex`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L92-L127) from the declarative tree configured by [`default_menu()`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/src/lib.rs#L31-L69) in `radivine-actions`. And [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L34-L43) (§1.2 `E1`–`E7`) proposes only `Action::description_for(&self, ctx: &BasicContext) -> Option<Cow<'_, str>>` (`E1`, called in [`ActionRegistry::build_action_leaf`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L317-L333)) and [`FileFacts::is_uninspected()`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L37) (`E2`) on `radivine-core`.

### 1.4 Retraction D — Existing `Enricher` Constants and Magic Sniffing Function
- **What was wrong:** The first review cited a 150 ms enrichment timeout, a 4 KiB sniff buffer, `MAX_INSPECTED_ITEMS`, and `detect_magic_mime`.
- **Verified Fact:** In [`crates/radivine-core/src/enrich.rs`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L22-L28):
  - [`MAX_ENRICHED_ITEMS`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L22) = `64`
  - [`SNIFF_BUFFER_BYTES`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L25) = `8192` (`8 KiB`)
  - [`DEFAULT_ENRICHMENT_TIMEOUT`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L28) = `Duration::from_millis(1500)` (`1500 ms`)
  - [`sniff_magic_mime`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L648-L670) already recognizes `image/png` (line 650), `image/jpeg` (line 653), `image/gif` (line 656), `image/webp` (line 659), `image/tiff` (line 662), `image/x-icon` (line 665), and `image/bmp` (line 668).

### 1.5 Retraction E — In-Process Watchdog Cleanup / Premature Resource Release (Prior B3 Recommendation)
- **What was wrong:** The first review's recommendation for **B3** suggested unlinking the timed-out item's `.rvtmp-*` file, waking/releasing its `ResourceGovernor` reservation, and spawning a replacement worker *before* the old worker process exited.
- **Why that was unsafe:** A timed-out Rust thread is still executing in the address space. It still holds its open `tmp` file descriptor, still holds its allocated pixel buffers in RAM, and could reach a filesystem syscall if not fenced. Deleting its active temp file, releasing its memory reservation while its buffers remain allocated, or spawning a second worker process before the first process exits violates single-writer and memory-budget invariants. Section 5 (`B3`) and Section 8 below provide the corrected process-level recovery design.

---

## 2. Accurate Current-Code Assessment (`v0.1.1`, `872d5d4`)

All citations below are verified directly against the `v0.1.1` source tree:

1. **Action Trait & Registry (`radivine-core`):**
   - [`Action`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/action.rs#L182-L201) defines `meta(&self) -> &ActionMeta`, `is_visible(&self, ctx: &BasicContext) -> bool` (synchronous, zero I/O), `enablement(&self, ctx: &Context) -> Enablement` (synchronous, zero I/O), and `run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError>`.
   - [`ActionRegistry::build_menu`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L182-L227) filters [`MenuSpec`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L13-L33) recursively (`build_children_from_specs`, lines 247–315), prunes empty submenus automatically (lines 290–294), applies [`MenuLimits::default()`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L54-L61) (`max_items_per_wheel = 8`, `max_depth = 4`), and builds [`ActionIndex`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L92-L127).
   - [`ActionRegistry::build_action_leaf`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L317-L333) currently copies `meta.description` into `node.description` (lines 327–329). The proposed `Action::description_for(&self, ctx: &BasicContext)` hook (`E1` in [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L36)) cleanly replaces those 3 lines without affecting existing actions.
2. **Context & Enrichment (`radivine-core`):**
   - [`BasicContext`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L56-L112) caps selections at [`MAX_CONTEXT_ITEMS`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L9) (`10,000`).
   - [`Item::from_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L132-L162) calls [`parse_file_uri_to_local_path`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L394-L421), which preserves non-UTF-8 Unix filenames losslessly via `OsString::from_vec(decoded_bytes)` (line 414).
   - However, [`local_path_to_file_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L360-L373) (noted as discrepancy `X10` in [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L61)) uses `path.to_string_lossy()` (line 362), and [`Commands::Invoke`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/main.rs#L58-L66) takes `paths: Vec<String>` and passes them through [`normalize_cli_paths`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/main.rs#L493-L502) → [`normalize_cli_arg_to_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L334-L356) → `local_path_to_file_uri`.
   - [`Enricher::start`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L120-L279) initializes items `0..64` to [`FileFacts::pending()`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L184-L193) and items `64..item_count` to `FileFacts { error: Some("Too many items to inspect".to_string()), .. }` (lines 126–139). It then inspects items `0..64` sequentially on a background thread (`radivine-enrich-{session_id}`), emitting an [`EnrichEvent::Updated`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L39-L47) snapshot after **each** inspected item (lines 205–241), bounded by [`DEFAULT_ENRICHMENT_TIMEOUT`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L28) (`1500 ms`).
3. **Session Commit & Dispatcher (`apps/radivine`):**
   - [`SessionController::commit_path`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/session.rs#L1492-L1592) hides the X11 overlay immediately (line 1499), re-checks `action.enablement(ctx)` (line 1554), dispatches to [`Dispatcher`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/dispatch.rs#L101-L185) only if `Enablement::Enabled`, and waits up to `FAST_COMMIT_EFFECT_WINDOW` (`20 ms`, line 1557) for immediate clipboard/notification effects. If `action.enablement(ctx)` is `Enablement::Pending` at commit time, `commit_path` skips dispatch and emits `"Action unavailable: Still inspecting file…"` (lines 1570–1579).
4. **Built-in Actions & Menu (`radivine-actions`):**
   - [`default_menu()`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/src/lib.rs#L31-L69) currently defines 4 top-level entries: `radivine.preview` (submenu), `radivine.copy`, `radivine.properties`, and `radivine.custom` (submenu).
   - [`dbus.rs`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/src/dbus.rs#L1-L492) implements a pure-Rust Unix-socket D-Bus client with [`call_desktop_notify`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/src/dbus.rs#L80-L130) and [`call_show_item_properties`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/src/dbus.rs#L42-L78).
5. **Control Center (`apps/radivine-control`):**
   - [`ControlPage`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/lib.rs#L15-L96) defines `Dashboard`, `General`, `Wheel`, `Actions`, `Diagnostics`, `About`.
   - [`apps/radivine-control/src/main.rs`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/main.rs#L47-L160) uses a hand-written argument loop and acquires [`ControlInstanceLock::acquire_or_notify_existing`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/single_instance.rs#L44-L82) on `control.sock`.

---

## 3. Correct `ADR-C01` Through `ADR-C18` Evaluation Table

All 18 proposed ADRs in [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L8-L187) are evaluated below using their exact original identifiers and titles. Every ADR remains in **Proposed** status.

| ADR | Exact Title in `ARCHITECTURE_DECISIONS.md` | Classification | Audit Assessment & Required Refinements |
| :--- | :--- | :--- | :--- |
| **ADR-C01** | Conversion is an Action capability, not a second action system | **Sound as proposed** | Integrates cleanly with [`Action`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/action.rs#L182-L201) and [`default_menu()`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/src/lib.rs#L31-L69); keeps heavy work out of [`Dispatcher`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/dispatch.rs#L101-L185). |
| **ADR-C02** | Crate boundaries: `radivine-convert`, `radivine-convert-image`, `radivine-jobs`, `radivine-worker` | **Sound as proposed** | Clean separation of codec-free core (`radivine-convert`), codecs (`radivine-convert-image`), IPC/job contract (`radivine-jobs`), and SQLite/scheduler executable (`apps/radivine-worker`). Includes explicit fallback if daemon binary delta exceeds `+300 KiB` (`Q-A`). |
| **ADR-C03** | Capability-based backends with data-only descriptors; deterministic routing | **Sound as proposed** | Data-only `BackendDescriptor` + pure `CapabilityCatalog::route` avoids instantiating codecs in the daemon and supports future optional backends cleanly. |
| **ADR-C04** | Built-in image backend: pure-Rust decode, `image` encoders, libwebp for lossy WebP only | **Requires experimental verification** | Architectural split (pure-Rust decode for all untrusted inputs; C `libwebp` only for lossy WebP encoding of Radivine-owned buffers) is strong. Properly gated on C1 spikes `P1`–`P7` ([CONVERSION_BACKENDS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_BACKENDS.md#L173-L186)). |
| **ADR-C05** | libwebp linkage: vendored static (owner decision D2) | **Requires experimental verification** | Pending owner decision `D2` and C1 spikes `P1`/`P9` (`libwebp` ≥ 1.3.2 version verification, build time, binary delta, BSD-3 notice in `THIRD_PARTY_NOTICES.md` and `packaging/debian/copyright`). |
| **ADR-C06** | Out-of-process, on-demand worker launched in its own systemd cgroup | **Sound with minor changes** | Static `radivine-worker.service` avoids the `radivine.service` cgroup trap. Refine the fallback when `systemd --user` is active but the unit file is not installed (see **B4**), and refine watchdog self-restart so the worker exits before cleanup/relaunch (see **B3**). |
| **ADR-C07** | SQLite (WAL) job store, single writer | **Sound with minor changes** | Single-writer `rusqlite` (`bundled`) in WAL mode is well-suited. Specify exact `PRAGMA synchronous=FULL` transaction boundary for `Committing` and directory `fsync` ordering before `Succeeded` (see **B6**), plus `temp_dev`/`temp_ino` columns (see **B2**). |
| **ADR-C08** | Job/item state machines; retry creates a new attempt; no auto-resume across sessions | **Sound as proposed** | Immutable terminal jobs with linked retry (`retry_of`) and `request_id` idempotency simplify recovery and auditability. |
| **ADR-C09** | Worker IPC: JSON lines, command + subscription connections, lossless paths | **Sound as proposed** | Consistent with [`apps/radivine/src/ipc.rs`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/ipc.rs#L1-L255); `WirePath` avoids lossy UTF-8 conversion across IPC and SQLite. |
| **ADR-C10** | No-overwrite output commit protocol | **Needs significant revision** | Revise three aspects of the normative protocol in [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L55-L110): (1) remove non-atomic Rung C exclusive-copy fallback (**B1**), (2) create temp file with `O_EXCL` and `fstat` *before* journaling `TempCreated(dir, temp_name, temp_dev, temp_ino)` (**B2**), and (3) open `tmp` with `O_RDWR` instead of `O_WRONLY` so Step 8 header validation can read from `tmp` (**B5**). |
| **ADR-C11** | Menu: `Convert ▸ Image ▸ target`; visibility by extension, enablement by sniffed MIME; `description_for` hook | **Sound with minor changes** | Preserves zero-I/O first-frame layout (`ADR-012`). Refine `ConvertAction::enablement` during progressive multi-file enrichment so fast drops do not fail with `Enablement::Pending` when `≥ 1` item is already confirmed convertible (see **A3**). |
| **ADR-C12** | Backend-independent options and presets (intents + validated per-format overrides) | **Sound with minor changes** | Good decoupling of user intent from codec parameters. Add an explicit same-format policy flag or mode on `ConversionOptions` so `ConversionEngine::plan()` can distinguish Quick Convert (skip same-format) from Advanced Convert (allow same-format re-encode/strip) (see **A2**). |
| **ADR-C13** | Configuration: `convert.toml` + `config.toml [jobs]`; daemon untouched | **Sound with minor changes** | Preserves forward/backward config compatibility. Reconcile the minor contradiction with `ADR-C15` regarding the worker reading `[notifications] enabled` from `config.toml` (see **A6**). |
| **ADR-C14** | Advanced Convert hosted by `radivine-control convert` | **Sound with minor changes** | Reuses `eframe` binary and `BrandPalette`. Resolve the trust-boundary wording discrepancy on header inspection (`R1`), specify cgroup detachment when launched from the daemon (`B4`), and specify how `"Open Jobs"` invokes the single-instance Control Center (`A5`). |
| **ADR-C15** | Worker sends notifications via the existing pure-Rust D-Bus client | **Sound with minor changes** | Ensures notifications arrive even if the daemon or Control Center is closed. Align `CONVERSION_ARCHITECTURE.md` §9 so the worker reads `[notifications] enabled` alongside `[jobs] notify` (**A6**). |
| **ADR-C16** | Optional backend framework ships; no optional backend in v0.2.0 (owner decision D3) | **Sound as proposed** | Keeps v0.2.0 scope disciplined while proving the `BackendDescriptor` / `Availability` / `ProbeSpec` contracts via tests. |
| **ADR-C17** | Animated sources rejected; same-format hidden in Quick; skips always reported | **Sound with minor changes** | Prevents silent frame loss or silent skips. Ensure `ConversionOptions` carries the Quick vs. Advanced same-format distinction into `ConversionEngine::plan()` (**A2**) and align mixed-selection wheel captions (**A7**). |
| **ADR-C18** | Metadata defaults: auto-orient, keep ICC, strip EXIF, drop XMP | **Requires experimental verification** | Privacy-first defaults (`ExifPolicy::Strip`, `OrientationPolicy::AutoOrient`, `IccPolicy::Preserve`). Lossy WebP ICC/EXIF RIFF muxing is properly marked `[VERIFY P2]` for C1. |

---

## 4. Accurate Owner Decisions `D1`–`D9` Evaluation Table

The exact owner decisions from [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L204-L216) are evaluated below:

| ID | Exact Decision in `ARCHITECTURE_DECISIONS.md` | Opus Recommendation | Independent Audit Verdict | Is This a Blocker for C1? |
| :--- | :--- | :--- | :--- | :--- |
| **D1** | Milestone re-sequencing: C1 core → **C2 worker** → C3 wheel integration → C4 Control Center UI → C5 hardening/release (5 milestones instead of 4) | Approve | **Recommend Approval.** Wiring the wheel in C2 before the durable worker exists would require a throwaway in-daemon execution path that violates requirement 4.8. | **Yes** (defines milestone sequence; C1 is the first milestone). |
| **D2** | libwebp linkage | Vendored static | **Recommend Conditional Approval** (vendored static `libwebp-sys` ≥ 1.3.2, gated on C1 spikes `P1`/`P9` and BSD-3 attribution). | **Yes** (needed before C1 Cargo dependencies are locked). |
| **D3** | Optional backend in v0.2.0 | Framework only | **Recommend Approval.** Shipping `external.vips` in v0.2.0 would expand scope and attack surface unnecessarily. | No (C1 implements the descriptor/probe framework either way). |
| **D4** | Menu depth | `Convert ▸ Image ▸ target` | **Recommend Approval.** Preserves muscle memory for future media categories and stays within `MenuLimits::default().max_depth = 4` ([`registry.rs:58`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/registry.rs#L58)). | No (affects C3). |
| **D5** | EXIF default | Strip | **Recommend Approval.** Strong privacy default; orientation is applied before stripping (`ADR-C18`). | **Yes** (sets `MetadataPolicy::default()` in `radivine-convert` in C1). |
| **D6** | Collision naming | `photo (1).webp` | **Recommend Approval.** Matches desktop conventions; race safety comes from `RENAME_NOREPLACE` / `linkat`, not the string format. | **Yes** (sets candidate formatting in `radivine-convert::output` in C1). |
| **D7** | Notification default | One per job, always | **Recommend Approval.** Configurable via `[jobs] notify = "always" \| "failures" \| "never"`. | No (affects C2/C3). |
| **D8** | Issue #1 timing | Fix separately before C4 | **Recommend Approval.** Keeps Custom Actions clipboard fix (`eframe` `clipboard` feature, `X11` discrepancy) isolated from conversion commits. | No (affects C4). |
| **D9** | Queued jobs after logout/reboot | Interrupted, manual Retry | **Recommend Approval.** Avoids surprising background CPU/disk load on login. *(Note: [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L225) §5 line 225 refers to this same decision as `D10`; align the label to `D9`.)* | No (affects C2). |

---

## 5. Confirmed Findings (Revalidation of `B1`–`B6` and `R1`–`R7`)

### B1 — Non-Atomic Exclusive-Copy Fallback ("Rung C") Violates Crash-Safe Output Guarantees and Uses an Undefined Journal State
- **Exact Source Document & Section:**
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L43) (§2 Threat `T10`, line 43)
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L90) (§3.3 Commit primitive ladder, Rung C, line 90)
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L468-L471) (§5.4 `CommitJournal`, lines 468–471)
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L189-L221) (§4.3 `items` schema, lines 189–202; §5 Recovery procedure, lines 217–221; §8 line 340)
- **Existing Code Symbol:** None (error in future C1/C2 specification).
- **Factual Observation:**
  1. [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L90) (§3.3 Rung C) specifies: *"journal `Writing{final}`, `openat(dir, final, O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW)`, copy the already-validated temp content into it (`std::io::copy` over the two fds), fsync, then unlink the temp"*.
  2. However, [`CommitJournal`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L468-L471) in `CONVERSION_ARCHITECTURE.md` only defines `TempCreated` and `Committing`—there is no `Writing` variant. Likewise, the SQLite `items` schema ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L189-L202)) and startup recovery ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L217-L220)) have no logic to delete a partially written `final_name`!
- **Inferred Risk:**
  - During `std::io::copy` into `final_name`, a 0-byte or partially written file exists at the final user-visible path (`photo.webp`). File manager thumbnailers (Nemo), user actions, `SIGKILL`, or power loss during the copy will observe a **corrupt final output file** at `final_name` that startup recovery will **not** clean up (and if `Committing` was already journaled at Step 11 with `tmp`'s `dev`/`ino`, recovery will mark the item `Interrupted` while leaving the corrupt `final_name` on disk, causing a subsequent Retry to create `photo (1).webp` beside the corrupt `photo.webp`).
- **Status:** **Confirmed (High Severity — Specification Blocker for C1).**
  - *Note on filesystem support:* As [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L88) correctly notes for Rung A, the Linux in-kernel `vfat` driver supports `renameat2(..., RENAME_NOREPLACE)` since Linux 4.9 (and in-kernel `exfat` does as well on modern kernels). Only FUSE mounts or network filesystems that reject both `RENAME_NOREPLACE` and `linkat` would ever reach Rung C.
- **Smallest Appropriate Correction:**
  - Remove Rung C from v0.2.0 (`ADR-C10`, `SECURITY_AND_FILE_SAFETY.md` §3.3, `IMPLEMENTATION_PLAN.md` `AC-C1.10`). Keep Rung A (`renameat2` `RENAME_NOREPLACE` / `renameatx_np` `RENAME_EXCL`) and Rung B (`linkat` + `unlinkat`). If both fail with `EINVAL`/`ENOSYS`/`EOPNOTSUPP`/`EPERM`, unlink `temp_name` and return `ConversionError::FilesystemUnsupported`.
- **Verification / Regression Test:**
  - Fault-injection test in `radivine-convert::output`: simulate Rung A returning `EINVAL` and Rung B returning `EOPNOTSUPP`; assert `convert_item` returns `Err(ConversionError::FilesystemUnsupported)`, `temp_name` is removed, and `final_name` is never created.

---

### B2 — Temporary-File Journaling Order and Missing Inode Verification Allow Unlinking Unrelated Files During Recovery/Cleanup
- **Exact Source Document & Section:**
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L63-L64) (§3.1 Steps 4–5, lines 63–64)
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L468-L471) (§5.4 `CommitJournal::TempCreated`, line 469)
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L198-L218) (§4.3 `items` schema line 198; §5 step 2 line 218; §8 line 340)
  - [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L175) (§4 line 175)
- **Existing Code Symbol:** None (error in future C1/C2 specification).
- **Factual Observation:**
  1. In [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L63-L64) (§3.1), Step 4 writes `journal(TempCreated{dir, temp_name})` to the database **before** Step 5 calls `openat(dir, temp_name, O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC, 0600)`.
  2. Neither `CommitJournal::TempCreated` ([CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L469)) nor the `items` table ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L198)) records `(temp_dev, temp_ino)` for the temporary file (`commit_dev` and `commit_ino` are only recorded at Step 11 `Committing`).
  3. In [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L218) (§5 step 2), startup recovery unlinks `temp_name` in `temp_dir` *"only if it is a regular file whose name matches the Radivine temp pattern"*—without verifying `(st_dev, st_ino)`.
  4. In [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L340) (§8) and [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L175) (§4), the `"Clean up leftover temporary files"` maintenance action scans destination directories of recent jobs and deletes files matching the `.rvtmp-*` name pattern without inode verification—which also contradicts `INV-SRC` ([SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L113): *"never calls `rename`/`unlink`/`truncate` on a path it did not create with `O_EXCL` in the current process"*).
- **Inferred Risk:**
  - If the worker crashes between Step 4 and Step 5 (before `openat`), or after `temp_name` was already unlinked/renamed before the DB state flushed, another process or user in a shared directory (`/tmp`, removable media, shared mount) can create or hard-link a regular file with that `.rvtmp-*` name, and worker recovery (or the manual cleanup tool) will delete it.
- **Status:** **Confirmed (High Severity — Security & Data-Safety Blocker for C1/C2).**
- **Smallest Appropriate Correction:**
  1. **Swap Steps 4 and 5 in `SECURITY_AND_FILE_SAFETY.md` §3.1:**
     - Step 4: `tmp = openat(dir, temp_name, O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC, 0600); st_tmp = fstat(tmp);`
     - Step 5: `journal(TempCreated { dir, temp_name, dev: st_tmp.st_dev, ino: st_tmp.st_ino })`
  2. **Record `temp_dev` and `temp_ino` in `CommitJournal::TempCreated` and SQLite `items`:**
     - Since `tmp` is the exact file later committed in Step 11, `(temp_dev, temp_ino)` captured at Step 4 is identical to `(commit_dev, commit_ino)` at Step 11 (`items` can store `temp_dev INTEGER, temp_ino INTEGER` at `TempCreated` and reuse them at `Committing`).
  3. **Require inode identity match on recovery unlink:**
     - During startup recovery ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L218) §5 step 2), open `temp_name` relative to `dir` with `O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, `fstat` the opened fd, and call `unlinkat(dir, temp_name, 0)` **only if** `S_ISREG(st_mode) && st_dev == temp_dev && st_ino == temp_ino`.
  4. **Scope the manual cleanup action safely:** Either remove the directory-scanning `"Clean up leftover temporary files"` action from v0.2.0 or restrict it to unlinking journaled `(temp_dir, temp_name, temp_dev, temp_ino)` records from the database (or require explicit user confirmation listing exact candidate paths, sizes, and mtimes).
- **Verification / Regression Test:**
  - Crash recovery test in `radivine-worker`: persist a `Running` item with `TempCreated { dir, temp_name, dev, ino }`, replace `temp_name` with a different regular file (new inode), run startup recovery, and verify the replacement file is **not** unlinked.

---

### B3 — Watchdog Timeout Recovery Must Enforce Process Termination Before Resource Release, Temp Cleanup, or Worker Replacement
- **Exact Source Document & Section:**
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L324) (§7.3 Time, line 324)
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L35) (§2 Threat `T2`, line 35)
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L460-L466) (§5.4 `ItemControl`, lines 460–466; §5.6 line 561)
- **Existing Code Symbol:** Mirrors the watchdog dilemma in [`Enricher::start`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L245-L279), but unlike read-only enrichment, conversion threads hold writable temp fds, journal callbacks, and large memory reservations.
- **Factual Observation:**
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L324) (§7.3) specifies: *"On expiry the item is marked `Failed(timeout)` and its result is ignored. Rust threads cannot be killed, so the worker enters **drain mode** (no new items), waits for the other items, then exits with code 75 (systemd `Restart=on-failure` restarts it; in spawn mode the next client relaunches it)."*
- **Inferred Risks (Three Distinct Failure Modes):**
  1. **Unfenced Late Mutation by the Timed-Out Thread:** Although the scheduler ignores the timed-out thread's return value, the timed-out thread holds `ctl: &dyn ItemControl` (`ItemControlImpl`) and `dir`/`tmp` file descriptors inside `ConversionEngine::convert_item`. If the thread was merely slow (e.g., stalled on I/O or a pathological resize) rather than in an infinite loop, it could advance to Step 11 (`ctl.journal(Committing)` + `commit_no_replace`) **after** the scheduler already marked the item `Failed(timeout)`, publishing an output file for an item recorded as `Failed`!
  2. **Drain-Mode Deadlock on `ResourceGovernor`:** The timed-out thread still holds its `Reservation` (`estimate.memory_bytes`) on its stack (and its `DynamicImage` buffer is still allocated in memory; releasing the `Reservation` early would lie to the memory governor and risk OOM). If a second already-running item thread is blocked in `ctl.reserve(estimate)` waiting for the timed-out thread's memory reservation (or waiting for an `exclusive: true` reservation), *"waits for the other items"* in drain mode will **block forever**!
  3. **Unsafe Premature Cleanup:** If the worker unlinks `temp_name` while the timed-out thread is still running, or if a replacement worker starts before the old worker process exits, two threads/processes can race on the filesystem and SQLite store.
- **Status:** **Confirmed (High Severity — Worker Lifecycle & Safety Blocker for C2).**
- **Smallest Appropriate Correction (Process-Isolated Watchdog Drain & Recovery Protocol):**
  1. **Epoch/Cancellation Fence in `ItemControlImpl`:** Give each item's `ItemControlImpl` an atomic state (`Running | Cancelled | TimedOut`). When the watchdog expires for an item, atomically transition its `ItemControlImpl` to `TimedOut` (which also makes `ctl.is_cancelled()` return `true` and causes `ctl.journal(...)` and `ctl.reserve(...)` to immediately return `Err(ConversionError::TimedOut)`). In `ConversionEngine::convert_item`, check `ctl.journal(Committing)?` immediately before `commit_no_replace`—guaranteeing a timed-out thread can **never** publish an output file even if it wakes up before process exit.
  2. **Do NOT Unlink the Timed-Out Item's Temp File or Release Its Memory Reservation While the Process Is Alive:** Leave its `Reservation` held (reflecting real RSS) and leave `(temp_dir, temp_name, temp_dev, temp_ino)` recorded in SQLite `items` with state `Failed { code: "timed_out", retryable: true }`.Wait for **startup recovery in the next worker process** (after kernel process termination has killed the stuck thread and closed its fds) to unlink the `.rvtmp-*` file safely using `(temp_dev, temp_ino)` verification.
  3. **Bounded Drain with Governor Wakeup (`≤ 5 s`):** On entering drain mode:
     - Stop dispatching new items.
     - Flip a worker-wide `draining` flag on `ResourceGovernor` and `notify_all()` on its `Condvar` so any other item thread blocked inside `ctl.reserve()` wakes immediately with `Err(ConversionError::Cancelled)` (mapped by the scheduler to `ItemState::Interrupted` so it remains part of the job or retryable) instead of deadlocking on the stuck thread's reservation.
     - Wait up to a **hard drain timeout of 5 seconds** on the scheduler's completion channel for other in-flight item threads (never calling `JoinHandle::join` on the timed-out thread). Any other in-flight item that has not finished when the 5 s drain timer expires is marked `Interrupted` in SQLite with its temp journal intact.
     - Close the SQLite `Connection` cleanly on the scheduler thread and call `std::process::exit(75)`.
  4. **Singleton-Safe Restart After Verified Process Exit:**
     - **systemd path:** `radivine-worker.service` (`Restart=on-failure`) waits for the old process to exit completely (releasing `worker.lock`), then starts the new worker, which runs startup recovery (§5) and resumes any `Queued` jobs (`same boot_id`, non-graceful exit).
     - **spawn path (when non-terminal `Queued` jobs remain):** Do **not** release `worker.lock` before exiting. Instead, if `Queued` jobs remain and the worker was not started by systemd, spawn a tiny `radivine-worker serve --wait-for-lock` child (with all non-stdio fds `O_CLOEXEC`, **except** `worker.lock` is `O_CLOEXEC` and NOT inherited, or the child simply blocks in `flock(worker.lock, LOCK_EX)` with a 10 s timeout). Because the parent still holds `flock(worker.lock, LOCK_EX)` until the kernel terminates the parent process during `exit(75)`, the child is kernel-guaranteed not to acquire `worker.lock`, open `jobs.sqlite3`, or run recovery until **after** the old worker process has completely exited!
- **Verification / Regression Test:**
  - Integration test in `radivine-worker` (`AC-C2.14` / `AC-C2.6`): configure `item_timeout = 1 s` and submit a 2-item job + a 2nd queued job using a test handler where item 1 holds a 400 MiB reservation and blocks forever on a private `Condvar` while holding an open `.rvtmp-*` file, and item 2 requests an exclusive reservation. Verify that:
    1. Item 2 wakes from `reserve()` immediately on watchdog expiry (no deadlock).
    2. The worker exits with code 75 within `≤ 6 s`.
    3. The replacement worker acquires `worker.lock` only after the old PID has exited, cleans up item 1's `.rvtmp-*` via `(temp_dev, temp_ino)` during startup recovery, and completes the remaining queued work.

---

### B4 — Detached Worker Fallback Can Remain Inside `radivine.service`'s Cgroup When the Static Unit Is Missing
- **Exact Source Document & Section:**
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L25-L38) (§2.2 The cgroup trap, lines 25–28; §2.3 Selected design, lines 33–38)
  - [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L86-L95) (`ADR-C06`, lines 86–95)
- **Existing Code Symbol:** [`packaging/radivine.service`](file:///home/marwanmohamed/Documents/work/rust/radivine/packaging/radivine.service#L1-L18) and [`generate_systemd_unit_content`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/service.rs#L827-L849) (which run `radivine daemon` with default `KillMode=control-group`).
- **Factual Observation:**
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L27) (§2.2) accurately explains that `systemctl --user restart|stop radivine.service` kills every process in `radivine.service`'s cgroup, including children that called `process_group(0)`.
  - However, in [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L35-L36) (§2.3), if `$XDG_RUNTIME_DIR/systemd/private` exists (meaning the user systemd manager **is** active and `radivine` is likely running inside `radivine.service`), but `radivine-worker.service` is **not** installed (e.g., a developer running a local build or a partial installation before C5 packaging), `WorkerLauncher` falls back directly to the `spawn` path (`process_group(0)`), placing `radivine-worker` inside `radivine.service`'s cgroup!
  - Similarly, `AdvancedConvertAction` ([CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L719) §7.5) spawns `radivine-control convert --stdin` with `process_group(0)` from the daemon; if the user restarts `radivine.service` while the Advanced Convert window is open, `KillMode=control-group` will kill the Advanced Convert window.
- **Status:** **Confirmed (Medium Severity — C2/C3/C4 Launcher Refinement).**
- **Smallest Appropriate Correction:**
  - In `WorkerLauncher` ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L33-L38) §2.3): when `$XDG_RUNTIME_DIR/systemd/private` exists:
    1. First try `systemctl --user start --no-block radivine-worker.service`.
    2. If `radivine-worker.service` is not installed (`Unit radivine-worker.service not found`), try launching via `systemd-run --user --collect --unit=radivine-worker-transient <worker_bin> serve` so it still runs in an independent transient systemd unit outside `radivine.service`'s cgroup.
    3. Only fall back to direct `Command::spawn` with `process_group(0)` when the user systemd manager itself is unreachable (in which case `radivine` is not running under `radivine.service` either).
- **Verification / Regression Test:**
  - Automated test in C2 (`T-LIFE-2` variant): with `radivine-worker.service` absent in a temporary `XDG_CONFIG_HOME/systemd/user`, verify `WorkerLauncher` places the worker into its own transient unit/scope when `systemd --user` is active, and falls back to `spawn` when `systemd --user` is unavailable.

---

### B5 — Output Header Validation Attempts to Read From an `O_WRONLY` Temporary File Descriptor (`EBADF`)
- **Exact Source Document & Section:**
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L64-L67) (§3.1 Step 5 line 64 and Step 8 line 67; §3.4 line 99)
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L510-L511) (§5.4 `OutputSink`, lines 510–511)
- **Existing Code Symbol:** None (error in future C1 specification).
- **Factual Observation:**
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L64) (§3.1 Step 5) specifies:
    `5  tmp = openat(dir, temp_name, O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC, 0600)`
  - Step 8 (line 67) then specifies:
    `8  tmp.flush(); validate: re-read output header via the tmp fd (format, width, height match expectations; size > 0)`
- **Inferred Risk:**
  - On Linux/POSIX, reading (`read(2)` or `pread(2)`) from a file descriptor opened with `O_WRONLY` deterministically fails with `EBADF`.
- **Status:** **Confirmed (High Severity — Deterministic Runtime Blocker for C1).**
- **Smallest Appropriate Correction:**
  - Change Step 5 in [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L64) §3.1 to open `tmp` with `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC` (mode `0600`).
  - Keep [`OutputSink<'a>`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L510-L511) exposing only `std::io::Write` (and `as_fd()` for future subprocesses) to `ConversionBackend::convert`.
  - In Step 8, after flushing the buffered writer, rewind/read from offset 0 of the same `O_RDWR` `tmp` file descriptor (via `FileExt::read_at` or `Seek::seek(SeekFrom::Start(0))`) to run `sniff_format` + header inspection without reopening by path.
- **Verification / Regression Test:**
  - Unit test in `radivine-convert::output` (`AC-C1.1`): convert a PNG to WebP and verify Step 8 validates the output header directly through the `O_RDWR` `tmp` file descriptor.

---

### B6 — Ordering and Durability Between SQLite WAL (`NORMAL` vs `FULL`), `fsync(dir)`, and Startup Recovery Can Leave Missing Outputs Marked `Succeeded` After Power Loss
- **Exact Source Document & Section:**
  - [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L71-L76) (§3.1 Steps 11–12, lines 71–76; §3.5 Durability policy, lines 104–110)
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L157-L161) (§4.2 lines 157–161; §5 lines 217–221; §7.2 line 313; §8 line 340)
- **Existing Code Symbol:** None (error in future C1/C2 specification).
- **Factual Observation:**
  1. [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L108) (§3.5) specifies: `fsync(dir) after commit: Once per destination directory at job end (and before marking the job terminal)`.
  2. [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L313) (§7.2) specifies: `item terminal results are coalesced into one transaction every ≤ 250 ms or 32 items, but Committing journal entries are written synchronously before the rename happens.`
  3. [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L217) (§5 step 2) specifies that startup recovery only inspects *"each item with a non-terminal state and a recorded journal"*.
  4. [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L160) (§4.2) notes that `Committing` is *"written in its own transaction with `PRAGMA synchronous=FULL` toggled, or followed by `wal_checkpoint(PASSIVE)`"*.
- **Inferred Risks:**
  1. **Power-loss hole between item `Succeeded` DB commit and job-end `fsync(dir)`:** In a 500-item batch, items 1..32 complete `renameat2(NOREPLACE)`, and after 250 ms the scheduler commits their state as `Succeeded` (a terminal item state!) to SQLite. When item 33 writes its `Committing` journal with `PRAGMA synchronous=FULL`, SQLite `fsync`s the WAL—making the `Succeeded` rows for items 1..32 power-loss durable in SQLite **before `fsync(dir)` has ever been called on the destination directory**! If power is lost while item 33 is running, the filesystem directory entries for items 1..32 (which were never `fsync`ed at the directory level) may be lost on reboot, while startup recovery ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L217) §5 step 2) skips items 1..32 because their state in SQLite is already terminal (`Succeeded`).
  2. **SQLite `wal_checkpoint(PASSIVE)` misconception:** In SQLite WAL mode under `PRAGMA synchronous=NORMAL`, `PRAGMA wal_checkpoint(PASSIVE)` does **not** `fsync` the WAL file if new frames were appended; only committing a transaction while `PRAGMA synchronous=FULL` is in effect guarantees the WAL is `fsync`ed before `renameat2` executes.
- **Status:** **Confirmed (High Severity — Crash-Consistency Blocker for C1/C2).**
- **Smallest Appropriate Correction:**
  1. **Never persist `ItemState::Succeeded` in SQLite before `fsync(dir)` has completed for that item's destination directory.**
     - Specifically: when the scheduler flushes a batch of completed items (every `≤ 250 ms` or `32` items, or at job end), it **first** calls `fsync(dir_fd)` once per unique destination directory touched in that batch, and **then** commits the SQLite transaction transitioning those items from `Running` (with `Committing` journal) to `Succeeded`.
     - That preserves the performance benefit of batched directory `fsync`s (at most 4 `fsync(dir)` calls per second per destination directory during a huge batch) while guaranteeing that every item marked `Succeeded` in SQLite has already had its file data **and** its directory entry `fsync`ed!
  2. **Use `PRAGMA synchronous = FULL` (not `wal_checkpoint(PASSIVE)`) for `Committing` journal writes:** Either run the worker SQLite connection with `PRAGMA synchronous = FULL` permanently (since item result transactions are already batched to ≤ 4 Hz in §7.2, the overhead is ≤ 4 WAL `fsync`s/sec plus the per-item `Committing` write), or explicitly toggle `PRAGMA synchronous = FULL;` around the `Committing` transaction.
- **Verification / Regression Test:**
  - Crash/recovery test in `radivine-worker` (`AC-C2.6`): inject an abort after `commit_no_replace` (`renameat2`) succeeds for item 1, both before and after `fsync(dir)`. Verify that if the output file is present with matching `(commit_dev, commit_ino)`, recovery marks item 1 `Succeeded`; if the output file entry was lost (simulated power loss before `fsync(dir)`), item 1 is still in `Running` + `Committing` state in SQLite and recovery marks it `Interrupted` (retryable), never leaving a missing output marked `Succeeded`.

---

## 6. Evaluation of Prior Non-Blocking Items (`R1`–`R7`) and Refuted Claims

| ID | Prior Review Claim | Audit Verdict | Evidence & Recommended Resolution |
| :--- | :--- | :--- | :--- |
| **R1** | UI-process header parsing in `radivine-control convert` (`CONVERSION_UX.md:207`) contradicts `SECURITY_AND_FILE_SAFETY.md:124` (*"Untrusted parsing happens only in `radivine-worker`"*). | **Confirmed Documentation Contradiction (Low-Medium)** | Actually look at [SECURITY_AND_FILE_SAFETY.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/SECURITY_AND_FILE_SAFETY.md#L124) line 124: the second sentence on that exact line already states: *"The Advanced window reads headers only, through the same pure-Rust header parsers and with the same limits."* Only the first sentence (*"Untrusted parsing happens only in `radivine-worker`"*) is overly broad. **Fix:** Reword the first sentence of `SECURITY_AND_FILE_SAFETY.md` §4 line 124 to *"Untrusted pixel decoding happens only in `radivine-worker`."* |
| **R2** | `JOB_SYSTEM_DESIGN.md` §3.3 mentions `Committing` as a stage, while `items` schema has no `stage` column. | **Overstated / Already Explained in Spec** | [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L123) (§3.3 line 123) explicitly explains: *"Stage is informational and persisted only at `Committing` (for recovery), otherwise kept in memory and streamed"*, and recovery ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L219) §5 line 219) checks `final_name IS NOT NULL` to detect that `Committing` was journaled. With **B1** (`Writing`) removed and **B2** (`temp_dev`, `temp_ino`) added, the schema is unambiguous. |
| **R3** | Strict quality monotonicity (`AC-C1.2`), rigid `max(200 MiB, 2 × source_size)` free-space precheck (`JOB_SYSTEM_DESIGN.md:323`), and `[TARGET]` performance budgets. | **Partially Confirmed (Low-Medium)** | 1. `AC-C1.2` ([IMPLEMENTATION_PLAN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/IMPLEMENTATION_PLAN.md#L69)) only tests monotonicity on **3 fixed deterministic reference images** (`q=70 < q=80 < q=90`), which is completely deterministic and controllable.<br/>2. **Confirmed issue on disk precheck:** Requiring `free ≥ max(200 MiB, 2 × source_size)` ([JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L323)) would reject converting a 50 KiB icon on a USB drive or partition with 150 MiB free. Change the precheck to `free ≥ max(16 MiB, 2 × source_size)` (or `source_size + 32 MiB`) and rely on `ENOSPC` handling during write. |
| **R4** | `radivine-worker → radivine-actions` dependency edge pulls `radivine-core` and `radivine-wheel` into the worker. | **Acceptable v0.2.0 Trade-Off (Low)** | Verified in [`crates/radivine-actions/Cargo.toml`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-actions/Cargo.toml#L1-L15): `radivine-actions` only depends on `radivine-core`, `serde`, `toml`, `thiserror`, `tracing`, `rustix`, and `percent-encoding` (no X11, no `egui`, no `fontdue`/`tiny-skia`). `ADR-C15` ([ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L167)) explicitly records this and defers extracting `dbus.rs` into a micro-crate. |
| **R5–R7** | Path contract tests, missing `IMPLEMENTATION_HANDOFF.md`, `D9`/`D10` typo, and C1 verification gates (`P1`–`P10`). | **Confirmed** | Incorporated into Sections 7, 8, and 9 below. |

---

## 7. Additional Findings With Supporting Evidence

Cross-checking the planning suite against the actual `v0.1.1` source code revealed the following additional issues:

### A1 — `radivine invoke` Corrupts Non-UTF-8 Paths Before They Reach the Daemon or `WirePath`
- **Exact Source Document & Section:** [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L61) (§1.3 Discrepancy `X10`, line 61; §6 line 610) and [IMPLEMENTATION_PLAN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/IMPLEMENTATION_PLAN.md#L124) (`AC-C2.16`, line 124; §C3 scope item 5 `"Invoke-mode (radivine invoke / Nemo action) parity"`, line 145).
- **Exact Existing Code Symbols:**
  - [`Commands::Invoke`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/main.rs#L58-L66) and [`normalize_cli_paths`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/main.rs#L493-L502) in `apps/radivine/src/main.rs`
  - [`normalize_cli_arg_to_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L334-L356) and [`local_path_to_file_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L360-L373) in `crates/radivine-core/src/context.rs`
- **Factual Observation:**
  - Discrepancy `X10` ([CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L61)) notes that `radivine_core::local_path_to_file_uri` uses `to_string_lossy()` and warns `WirePath` not to reuse it.
  - However, in **Invoke mode** (`radivine invoke <PATHS>` or the Nemo context menu action), CLI arguments are parsed into `paths: Vec<String>` ([`apps/radivine/src/main.rs:61`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/main.rs#L61) — causing `clap` to reject non-UTF-8 `OsString` arguments immediately), and [`local_path_to_file_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L360-L373) converts the path via `path.to_string_lossy()` (line 362) before sending the `file://` URI to the daemon!
- **Inferred Risk:**
  - Even though [`Item::from_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L394-L421) and `WirePath` preserve raw bytes, any non-UTF-8 filename passed via `radivine invoke` is either rejected by `clap` or replaced with `%EF%BF%BD` (`U+FFFD`) before the daemon ever builds `Item::local_path`.
- **Status:** **Confirmed Current-Code Bug Affecting C3 Invoke Parity (Medium-High Severity).**
- **Smallest Appropriate Correction:**
  - In C3 (when touching `radivine-core` and `apps/radivine`), change [`local_path_to_file_uri`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/context.rs#L360-L373) to percent-encode raw `OsStr` bytes on Unix (`use std::os::unix::ffi::OsStrExt; percent_encode(seg.as_bytes(), PATH_ENCODE_SET)`), add `normalize_cli_os_arg_to_uri(arg: &OsStr, base_dir: &Path)`, and change [`Commands::Invoke::paths`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/main.rs#L61) from `Vec<String>` to `Vec<OsString>`.
- **Verification / Regression Test:**
  - Unit test in `radivine-core::context`: round-trip a `PathBuf` containing raw byte `0xFF` (`OsString::from_vec(b"/tmp/photo-\xff.png".to_vec())`) through `local_path_to_file_uri` and `Item::from_uri`, asserting `item.local_path.unwrap() == original_path`.

---

### A2 — `ConversionOptions` Lacks a Same-Format Policy Field to Distinguish Quick Convert (Skip) From Advanced Convert (Re-encode)
- **Exact Source Document & Section:**
  - [CONVERSION_PRODUCT_SPEC.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_PRODUCT_SPEC.md#L35) (§2 line 35)
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L388-L396) (§5.3 `ConversionOptions`, lines 388–396; §5.4 `SkipReason::AlreadyTargetFormat`, line 443, and `ConversionEngine::plan`, line 487; §7.6 table row 2, line 727)
  - [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L82-L83) (§2 Flow 4, lines 82–83)
- **Existing Code Symbol:** None (gap in future C1 API sketch).
- **Factual Observation:**
  - In Quick Convert on a mixed selection `a.png, b.jpg, c.webp` → `WebP`, [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L727) (§7.6 line 727) requires `c.webp` to be skipped with `SkipReason::AlreadyTargetFormat`.
  - In Advanced Convert, [CONVERSION_PRODUCT_SPEC.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_PRODUCT_SPEC.md#L35) (§2 line 35) and [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L82-L83) (Flow 4: 287 JPEGs + 13 PNGs → JPEG) require same-format files to be converted (re-encoded/resized/metadata-stripped), **not** skipped!
  - Both Quick Convert and Advanced Convert pass `(sources, options: ConversionOptions)` into [`ConversionEngine::plan`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L487) (and into the authoritative per-item check in `convert_item` when a file's sniffed content matches `target`, line 730). Yet [`ConversionOptions`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L388-L396) has no field indicating whether same-format items should be skipped (`Quick`) or converted (`Advanced`)! (Checking whether quality or EXIF stripping is set does not work because Quick Convert's default `balanced` preset also sets `quality = Balanced` and `exif = Strip`.)
- **Status:** **Confirmed Specification Gap (Medium-High Severity — Affects C1 API).**
- **Smallest Appropriate Correction:**
  - Add `pub same_format: SameFormatPolicy` (`pub enum SameFormatPolicy { Skip /* Quick default */, Convert /* Advanced */ }`) to [`ConversionOptions`](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L388-L396).
- **Verification / Regression Test:**
  - Unit test in `radivine-convert` (`AC-C1.1`): call `engine.plan([a.png, c.webp], opts)` with `target = WebP`: verify `c.webp` is in `plan.skipped` (`AlreadyTargetFormat`) when `same_format == Skip`, and in `plan.items` when `same_format == Convert`.

---

### A3 — `ConvertAction::enablement` Returning `Pending` While Any of 64 Items Is Pending Can Cause Fast Multi-File Drops to Fail at Commit Time
- **Exact Source Document & Section:** [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L653-L662) (§7.3 Enablement pseudocode, line 654).
- **Exact Existing Code Symbols:**
  - [`Enricher::start`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L120-L243) in `crates/radivine-core/src/enrich.rs`
  - [`SessionController::commit_path`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/session.rs#L1554-L1580) in `apps/radivine/src/session.rs`
- **Factual Observation:**
  - In [`Enricher::start`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-core/src/enrich.rs#L126-L139), the first `MAX_ENRICHED_ITEMS` (`64`) items start as `FileFacts::pending()` and are inspected sequentially one by one on the enricher thread, streaming a partial `Context` snapshot after each item (lines 205–241).
  - In [`SessionController::commit_path`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine/src/session.rs#L1570-L1579), if the user releases the mouse button while `action.enablement(ctx)` is `Enablement::Pending`, the daemon **aborts the commit** and emits a notification: `"Action unavailable: Still inspecting file…"`.
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L654) (§7.3 line 654) specifies:
    `if ctx.facts is None or any eligible inspected item is Pending → Pending ("Checking…")`.
- **Inferred Risk:**
  - If a user drags 50 images (e.g., on a slow disk, USB drive, or network mount) and quickly flicks to `Convert ▸ Image ▸ WebP`, item #0 may already be enriched and confirmed as a valid PNG (`confirmed >= 1`), while item #40 is still `Pending`. Because line 654 waits until **all** eligible items in `0..64` are non-pending before returning `Enabled`, the segment stays disabled (`"Checking…"`) and a fast release aborts the action!
- **Status:** **Confirmed Interaction Issue Between C3 Spec and Existing `session.rs` (Medium Severity).**
- **Smallest Appropriate Correction:**
  - In [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L653-L662) (§7.3), evaluate `confirmed` first:
    - If `catalog.target_status == RequiresOptional` → `Disabled("Needs {requirement}")`.
    - Else if `confirmed > 0` → `Enabled` (at least one item is already verified convertible; any still-pending items will be validated authoritatively by the worker).
    - Else if `ctx.facts is None` or any eligible item is `Pending` → `Pending ("Checking…")`.
    - Else if `unknown > 0` (only items beyond the 64-item cap were eligible) → `Enabled`.
    - Else → `Disabled(...)` (`"Already {T}"` / `"No permission to read"` / `"No convertible images"`).
- **Verification / Regression Test:**
  - Unit test in `radivine-actions::convert` (`AC-C3.2`): construct a `Context` with 2 PNG items where item 0 has `FileFacts { kind: Regular, readable: true, sniffed_mime: Some("image/png"), .. }` and item 1 has `FileFacts::pending()`. Assert `ConvertAction(WebP)::enablement(&ctx) == Enablement::Enabled`.

---

### A4 — `ResourceGovernor` Over-Budget Exclusive Items vs. `image::Limits.max_alloc` and Starvation
- **Exact Source Document & Section:** [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L320) (§7.3 Memory, line 320) and [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L575) (§5.7 line 575).
- **Existing Code Symbol:** None (subtle bug in future C1/C2 specification).
- **Factual Observation:**
  - [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L320) (§7.3) states:
    *"Each item reserves `estimate.memory_bytes` (from header dimensions × bpp × factor) before decode. If an item needs more than the free budget, it waits. If it needs more than the whole budget but is within `limits.max_pixels`, it runs **exclusively** (waits for all others)... `image::Limits.max_alloc` is set per item to the reservation."*
  - Consider a 100 MP RGBA image (`10,000 × 10,000`, which is within default `max_pixels = 100_000_000`). Its decoded buffer is `400 MiB`, and its governor `estimate` (`w·h·bpp × 2.2 + 16 MiB`, [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L575)) is `~896 MiB`.
  - On a machine with 2 GiB RAM (or with `memory_budget_mb = 512`), `estimate` (`896 MiB`) exceeds the whole budget (`512 MiB`), so the item runs **exclusively**.
- **Inferred Risks:**
  1. If `Reservation::bytes()` for an exclusive over-budget item is clamped to `memory_budget_mb` (e.g., `256 MiB` in `AC-C2.12`), and `image::Limits.max_alloc` is set to that clamped reservation (`256 MiB`) instead of `estimate.memory_bytes` (`896 MiB`), `image` will reject the `400 MiB` decode allocation even though the item was granted exclusive execution!
  2. When an exclusive item is waiting for in-flight normal items to finish, newly arriving small items on other worker threads could continually acquire the freed budget if `ResourceGovernor::reserve()` does not use a FIFO queue or gate new normal reservations while an exclusive waiter is pending.
- **Status:** **Confirmed Specification Nuance (Medium Severity — Affects C1/C2).**
- **Smallest Appropriate Correction:**
  - Specify in [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L320) §7.3 that:
    1. An exclusive reservation sets `image::Limits.max_alloc = Some(estimate.memory_bytes)` (not the smaller pool `memory_budget_mb`).
    2. `ResourceGovernor::reserve()` blocks new normal reservations while an exclusive waiter is at the head of the wait queue (preventing starvation).
- **Verification / Regression Test:**
  - Unit test in `radivine-worker` (`AC-C2.12`): with `memory_budget_mb = 256` and a 100 MP image (estimate ~896 MiB, decoded 400 MiB), verify the item acquires an exclusive reservation with `max_alloc >= 400 MiB` and is not starved by queued small items.

---

### A5 — `radivine-control convert` Navigation to `"Open Jobs"` / `"Show progress"` Needs Explicit Process/IPC Handoff
- **Exact Source Document & Section:** [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L65-L83) (§2 Flow 2 line 65, Flow 4 line 83; §5 line 180).
- **Exact Existing Code Symbols:**
  - [`main`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/main.rs#L38-L167) in `apps/radivine-control/src/main.rs`
  - [`ControlInstanceLock::acquire_or_notify_existing`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/single_instance.rs#L44-L82) in `apps/radivine-control/src/single_instance.rs`
- **Factual Observation:**
  - `radivine-control convert` runs as a separate process that does **not** acquire the single-instance lock on `control.sock` ([CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L180)).
  - In Flows 2 and 4 ([CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L65-L83)), clicking `"Open Jobs"` or `"Show progress"` opens `Control Center → Jobs`.
- **Inferred Risk:**
  - Because `radivine-control convert` is already inside `eframe::run_native` without holding `ControlInstanceLock`, it cannot switch its own window into the single-instance `ControlCenterApp` without conflicting with an already-running Control Center instance.
- **Status:** **Confirmed Implementation Detail to Specify for C4 (Low-Medium Severity).**
- **Smallest Appropriate Correction:**
  - Specify in [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L180) §5 that clicking `"Open Jobs"` / `"Show progress"` in `radivine-control convert` spawns `std::env::current_exe()` with `["--page", "jobs"]` (which automatically notifies and focuses an existing Control Center via [`ControlInstanceLock::acquire_or_notify_existing`](file:///home/marwanmohamed/Documents/work/rust/radivine/apps/radivine-control/src/single_instance.rs#L44-L82) or launches a new single-instance Control Center on `Jobs`) and then closes the Advanced Convert window.
- **Verification / Regression Test:**
  - Unit/integration test in `radivine-control`: verify `ControlPage::from_slug("jobs")` and `ControlPage::from_slug("conversion")` round-trip over `ControlInstanceLock::acquire_or_notify_existing`.

---

### A6 — Minor Config Ownership Contradiction Between `CONVERSION_ARCHITECTURE.md` §9 and `CONVERSION_PRODUCT_SPEC.md` §13 (`[notifications] enabled`)
- **Exact Source Document & Section:**
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L755) (§9 table row 2, line 755)
  - [CONVERSION_PRODUCT_SPEC.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_PRODUCT_SPEC.md#L222) (§13 line 222)
  - [ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L165) (`ADR-C15`, line 165)
  - [IMPLEMENTATION_PLAN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/IMPLEMENTATION_PLAN.md#L100) (§C2 scope item 2, line 100)
- **Factual Observation:**
  - `ADR-C15`, `CONVERSION_PRODUCT_SPEC.md` §13, and `IMPLEMENTATION_PLAN.md` §C2 all state that `radivine-worker` reads both `[jobs] notify` and `[notifications] enabled` from `config.toml`.
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L755) (§9 table row 2) states: `worker (reads only [jobs]; ignores the rest)`.
- **Status:** **Confirmed Documentation Contradiction (Low Severity).**
- **Smallest Appropriate Correction:**
  - Update [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L755) §9 to read: `worker (reads [jobs] and [notifications].enabled; ignores the rest)`.

---

### A7 — Mixed-Selection Wheel Caption Format When Selection Contains Same-Format Images
- **Exact Source Document & Section:**
  - [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L664-L668) (§7.3 lines 664–668; §7.6 line 727)
  - [CONVERSION_UX.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_UX.md#L30-L33) (§1.1 lines 30–33)
- **Factual Observation:**
  - For a mixed selection of `a.png, b.jpg, c.webp` when hovering `WebP`, `c.webp` is an eligible image whose extension format already equals the target `WebP`, so only 2 of the 3 files will be converted (`c.webp` will be skipped as `Already WebP`, [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L727)).
  - However, the caption templates in [CONVERSION_ARCHITECTURE.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/CONVERSION_ARCHITECTURE.md#L665-L667) (§7.3) only list `"Convert 12 images to WebP"` and `"Convert 3 of 5 files to WebP · 2 not supported"`.
- **Status:** **Confirmed UX Specification Gap (Low Severity).**
- **Smallest Appropriate Correction:**
  - Specify that `ConvertAction::description_for(&BasicContext)` counts only items with `ext_format != target` as convertible in the caption (e.g., `"Convert 2 of 3 images to WebP · 1 already WebP"`, or `"Convert 2 of 4 files to WebP · 1 already WebP, 1 not supported"`).

---

### A8 — SQLite Schema: Missing Index on `jobs(retry_of)` and `WITHOUT ROWID` on Wide `items` Table
- **Exact Source Document & Section:** [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L164-L204) (§4.3 Schema v1, lines 164–204).
- **Factual Observation:**
  1. `jobs` declares `retry_of INTEGER REFERENCES jobs(id) ON DELETE SET NULL` (line 172) with `PRAGMA foreign_keys=ON` (line 157), but has no index on `jobs(retry_of)`. In SQLite, deleting parent rows from `jobs` during history retention pruning (§4.4) or `clear_history` performs a full table scan of `jobs` per deleted row unless `retry_of` is indexed.
  2. `items` is declared `WITHOUT ROWID` (line 202) while containing 8 variable-length `TEXT` columns (`source`, `output`, `warnings`, `error_message`, `error_detail`, `temp_dir`, `temp_name`, `final_name`) and a secondary index `items_state(job_id, state)`. SQLite documentation recommends against `WITHOUT ROWID` for wide rows (> ~100 bytes) because it disables overflow-page separation in the primary B-tree.
- **Status:** **Confirmed Schema Optimization (Low Severity).**
- **Smallest Appropriate Correction:**
  - Add `CREATE INDEX jobs_retry_of ON jobs(retry_of) WHERE retry_of IS NOT NULL;` and omit `WITHOUT ROWID` on `items` in [JOB_SYSTEM_DESIGN.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/JOB_SYSTEM_DESIGN.md#L164-L204) §4.3.

---

## 8. Corrected Technical Recommendations

Before starting implementation, apply these focused updates to the planning specifications:

1. **Output Commit Protocol (`SECURITY_AND_FILE_SAFETY.md` §3.1–3.5, `CONVERSION_ARCHITECTURE.md` §5.4, `ADR-C10`):**
   - **Remove Rung C** (non-atomic `O_EXCL` copy into `final_name`) and `Writing{final}` (`B1`).
   - **Open `tmp` with `O_RDWR`** (`O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC`, mode `0600`) so Step 8 header validation reads directly from `tmp` (`B5`).
   - **Create `tmp` and `fstat(tmp)` before journaling `TempCreated`:**
     ```rust
     pub enum CommitJournal {
         TempCreated { dir: PathBuf, temp_name: OsString, dev: u64, ino: u64 },
         Committing  { dir: PathBuf, temp_name: OsString, final_name: OsString, dev: u64, ino: u64 },
     }
     ```
     Verify `S_ISREG && st_dev == dev && st_ino == ino` before unlinking `temp_name` in recovery (`B2`).
   - **Sync destination directory before persisting `ItemState::Succeeded`:** Run `fsync(dir)` (per item or per ≤250 ms / 32-item commit batch) *before* committing the SQLite transaction that marks those items `Succeeded`, and ensure `Committing` journal writes execute under `PRAGMA synchronous = FULL` (`B6`).
2. **Watchdog Timeout, Drain, and Process-Exit Recovery (`JOB_SYSTEM_DESIGN.md` §5 & §7.3, `ADR-C06`):**
   - Fence `ItemControlImpl` with an atomic `TimedOut` state so a timed-out thread cannot journal `Committing` or publish an output file if it wakes up late (`B3`).
   - Never unlink a timed-out item's `.rvtmp-*` file or release its `ResourceGovernor` memory reservation while the worker process is still alive (`B3`).
   - On watchdog expiry, wake all threads blocked in `ResourceGovernor::reserve()` with `Cancelled`/`Interrupted`, bound drain mode to a hard `5 s` timeout without calling `join()` on the hung thread, close SQLite cleanly, and `exit(75)` so kernel process termination releases `worker.lock` and closes all file descriptors *before* the next worker starts and runs recovery (`B3`).
   - For `ResourceGovernor` exclusive over-budget items, set `image::Limits.max_alloc = Some(estimate.memory_bytes)` and prevent starvation by normal items (`A4`).
   - Lower the disk free-space precheck floor from `200 MiB` to `16 MiB` (`max(16 MiB, 2 × source_size)`) (`R3`).
3. **Worker & UI Cgroup Isolation (`JOB_SYSTEM_DESIGN.md` §2.3, `CONVERSION_ARCHITECTURE.md` §7.5, `ADR-C06`, `ADR-C14`):**
   - In `WorkerLauncher`, if `systemd --user` is reachable but `radivine-worker.service` is not installed, attempt `systemd-run --user --collect` before falling back to `process_group(0)` (`B4`).
   - Add `After=dbus.socket` to `packaging/radivine-worker.service` (`JOB_SYSTEM_DESIGN.md` §2.3).
4. **Conversion Options, Wheel Integration, and Invoke Path Preservation (`CONVERSION_ARCHITECTURE.md` §5.3 & §7.3, `radivine-core`, `apps/radivine`):**
   - Add `pub same_format: SameFormatPolicy` (`Skip | Convert`) to `ConversionOptions` (`A2`).
   - Update `ConvertAction::enablement` so `confirmed > 0` returns `Enablement::Enabled` even while later items in the 64-item enrichment batch are still `Pending` (`A3`).
   - Fix `radivine_core::local_path_to_file_uri` and `Commands::Invoke` in C3 so non-UTF-8 paths are percent-encoded from raw `OsStr` bytes rather than `to_string_lossy()` (`A1`).
5. **Documentation & Schema Hygiene:**
   - Reword `SECURITY_AND_FILE_SAFETY.md` §4 line 124 to *"Untrusted pixel decoding happens only in `radivine-worker`"* (`R1`).
   - Update `CONVERSION_ARCHITECTURE.md` §9 line 755 to note the worker reads `[notifications].enabled` (`A6`).
   - Fix the `D10` reference in `JOB_SYSTEM_DESIGN.md` §5 line 225 to `D9` (`R6`).
   - Drop `WITHOUT ROWID` from `items` and add `CREATE INDEX jobs_retry_of ON jobs(retry_of)` in `JOB_SYSTEM_DESIGN.md` §4.3 (`A8`).
   - Author the missing `docs/planning/v0.2.0/IMPLEMENTATION_HANDOFF.md` (`R6`).

---

## 9. Milestone C1 Authorization Readiness Checklist

Milestone **C1 (`radivine-convert` + `radivine-convert-image`)** is ready to be authorized once the following prerequisites are satisfied:

- [ ] **1. Owner Decisions Confirmed:** Owner confirms `D1` (5-milestone sequence), `D2` (vendored static `libwebp` linkage for lossy WebP, gated on C1 spikes `P1`/`P9`), `D5` (strip EXIF default), and `D6` (`"photo (1).webp"` collision format).
- [ ] **2. Specification Updates Applied to `docs/planning/v0.2.0/`:**
  - [ ] `SECURITY_AND_FILE_SAFETY.md` §3.1–3.5 updated for **B1** (drop Rung C), **B2** (`openat` + `fstat` before `TempCreated(dir, temp_name, dev, ino)`), **B5** (`O_RDWR` on `tmp`), and **B6** (`fsync(dir)` before SQLite `Succeeded`).
  - [ ] `CONVERSION_ARCHITECTURE.md` §5.3–5.4 updated for `CommitJournal::TempCreated { dir, temp_name, dev, ino }` (**B2**) and `ConversionOptions::same_format: SameFormatPolicy` (**A2**).
  - [ ] `JOB_SYSTEM_DESIGN.md` §2.3, §4.3, §5, §7.3 updated for **B2**, **B3**, **B4**, **B6**, **A4**, and **A8**.
  - [ ] `IMPLEMENTATION_HANDOFF.md` created in `docs/planning/v0.2.0/`.
- [ ] **3. C1 Spike Gate Enforced (`CONVERSION_BACKENDS.md` §10 `P1`–`P9`):**
  - C1 begins with time-boxed verification spikes `P1`–`P9` in `spikes/convert-*` and stops to report if any spike disproves a backend assumption.
- [ ] **4. Explicit Owner Authorization (`AGENTS.md` §3.1):**
  - Owner issues explicit authorization to begin Milestone C1.

---

## 10. Unresolved Decisions Genuinely Requiring Owner Approval

1. **Owner Decisions `D1`–`D9` ([ARCHITECTURE_DECISIONS.md](file:///home/marwanmohamed/Documents/work/rust/radivine/docs/planning/v0.2.0/ARCHITECTURE_DECISIONS.md#L204-L216)):**
   - Confirm whether all nine recommended defaults (`D1`–`D9` in Section 4 above) are approved.
2. **Removal of Rung C Exclusive-Copy Fallback (`B1` / `ADR-C10`):**
   - Confirm that v0.2.0 drops the non-atomic Rung C fallback (`O_CREAT | O_EXCL` copy into the final filename) and returns `ConversionError::FilesystemUnsupported` on exotic filesystems that reject both `renameat2(RENAME_NOREPLACE)` and `linkat`. (Standard Linux ext4, btrfs, xfs, tmpfs, and in-kernel vfat/exFAT USB mounts all support Rung A or Rung B.)
3. **Leftover Temp-File Cleanup Policy (`B2`):**
   - Confirm that automatic recovery and manual temp cleanup only unlink files whose `(temp_dir, temp_name, temp_dev, temp_ino)` match a journaled record in `jobs.sqlite3` (never blind wildcard scanning of destination directories).
4. **Authorization to Update Planning Specs Only vs. Authorize C1:**
   - State whether you want the planning documents in `docs/planning/v0.2.0/` updated with the Section 8 corrections first (plus creating `IMPLEMENTATION_HANDOFF.md`), or whether you also authorize starting Milestone C1 after those spec updates.
