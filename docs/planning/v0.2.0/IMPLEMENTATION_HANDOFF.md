# Radivine v0.2.0 — Implementation Handoff Guide

Status: **Architecture Consolidated — Pending Owner Approval & Milestone Authorization** · Prepared: 2026-10-24 · Baseline: `v0.1.1` (`872d5d4`)
Related: [README](README.md) · [CONVERSION_ARCHITECTURE](CONVERSION_ARCHITECTURE.md) · [JOB_SYSTEM_DESIGN](JOB_SYSTEM_DESIGN.md) · [SECURITY_AND_FILE_SAFETY](SECURITY_AND_FILE_SAFETY.md) · [CONVERSION_BACKENDS](CONVERSION_BACKENDS.md) · [CONVERSION_PRODUCT_SPEC](CONVERSION_PRODUCT_SPEC.md) · [CONVERSION_UX](CONVERSION_UX.md) · [ARCHITECTURE_DECISIONS](ARCHITECTURE_DECISIONS.md) · [IMPLEMENTATION_PLAN](IMPLEMENTATION_PLAN.md) · [INDEPENDENT_ARCHITECTURE_REVIEW](INDEPENDENT_ARCHITECTURE_REVIEW.md)

---

## 13. Explicit Non-Authorization Notice (Read First)

> [!CAUTION]
> **MILESTONE C1 IS NOT YET AUTHORIZED.**
>
> Per `AGENTS.md` §3 (*Mandatory Milestone Authorization Policy*):
> - **Do NOT begin Milestone C1 (or any spike/crate implementation)** until the project owner explicitly approves the v0.2.0 architecture decisions (`ADR-C01`–`ADR-C18` and `D1`–`D9`) and explicitly authorizes Milestone C1.
> - All ADRs in [`ARCHITECTURE_DECISIONS.md`](ARCHITECTURE_DECISIONS.md) (`ADR-C01` through `ADR-C18`) are currently in **Proposed** status, and owner decisions `D1`–`D9` are recommendations awaiting formal approval.
> - Each authorized milestone (`C1`, `C2`, `C3`, `C4`, `C5`) is a **hard scope boundary**. Completing one milestone never authorizes the next.

---

## 1. Executive Summary of the v0.2.0 Architecture

Radivine v0.2.0 introduces a **reusable, desktop-agnostic Conversion Engine** and a **general-purpose durable Background Job System** while preserving the shipped v0.1.1 radial wheel (`radivine-wheel`, `radivine-render`), X11 platform layer (`radivine-platform`), session state machine, and text-file wheel layout:

1. **One Action System (`ADR-C01`, `ADR-C11`):** Image conversion is integrated into `ActionRegistry` via `default_menu()` at root index 1 (`Convert ▸ Image ▸ PNG | JPEG | WebP | Advanced…`). Visibility is computed in zero-I/O from file extensions in `BasicContext` plus an in-memory `CapabilityCatalog` snapshot. Progressive enablement is computed from `FileFacts.sniffed_mime` (enabling as soon as the first enriched item confirms ≥ 1 convertible image; `enrich.rs:205-242` emits an `EnrichEvent::Updated` snapshot after each inspected item up to `MAX_ENRICHED_ITEMS = 64`).
2. **Actions Decide; Worker Executes (`ADR-C02`, `ADR-C06`):** `ConvertAction::run` never decodes or encodes images in the wheel daemon. It builds a `JobSpec` (`kind = "radivine.convert.image"`) with lossless `WirePath` items and submits it over a `0600` Unix domain socket (`$XDG_RUNTIME_DIR/radivine/worker.sock`) to an out-of-process, on-demand worker (`radivine-worker`).
3. **Cgroup Isolation (`ADR-C06`, `B4`):** `WorkerLauncher` starts `radivine-worker` via `systemctl --user start --no-block radivine-worker.service` (static unit with `After=dbus.socket`), falling back to `systemd-run --user --collect --unit=radivine-worker-transient -- radivine-worker serve` when `systemd --user` is active but the unit file is not installed (dev builds), and falling back to `setsid` / `process_group(0)` only when no `systemd --user` manager is running. Restarting `radivine.service` (`KillMode=control-group`) or closing the Control Center never terminates running conversions.
4. **Authoritative Worker Configuration Resolution (`ADR-C13`):** The wheel daemon (`radivine`) reads **zero** conversion configuration (`convert.toml` is never loaded by the daemon). `radivine-worker` is the sole authoritative runtime resolver of `convert.toml` during a job's `Preparing` phase and snapshots the resolved `ConversionOptions` into `jobs.resolved`.
5. **Hybrid Memory-Safe Image Pipeline (`ADR-C04`, `ADR-C05`):** All untrusted input parsing (format sniffing, header probing, pixel decoding for PNG/JPEG/WebP) and PNG/JPEG/lossless-WebP encoding run in pure memory-safe Rust (`image` 0.25 + `image-webp` + `zune-jpeg`). Native C `libwebp` (vendored static ≥ 1.3.2 via a minimal audited wrapper) is invoked **only** for lossy WebP encoding on already-validated, Radivine-allocated RGBA8/RGB8 pixel buffers.
6. **Durable SQLite WAL Store & Atomic Output Commit (`ADR-C07`, `ADR-C10`):** `radivine-worker` persists job and item state in `$XDG_STATE_HOME/radivine/jobs.sqlite3`. Outputs are staged as `item-<idx>-<rand16>.part` (`0600`) inside a private per-job directory `.rvstage-<job>-<rand16>` (`0700`) on the destination filesystem, opened `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC`, validated via `O_RDWR` header re-inspection after `fsync(temp_fd)`, fenced by `ItemControl::begin_commit(CommitRequest::new(...))` (`PRAGMA synchronous=FULL`), and published exclusively via atomic no-replace primitives (Rung A `RENAME_NOREPLACE`/`RENAME_EXCL` → Rung B `linkat` + `unlinkat` → `ConversionError::FilesystemUnsupported`).

---

## 2. Crate and Binary Responsibilities

| Crate / Binary | Path | `#![forbid(unsafe_code)]` | Responsibility |
|----------------|------|---------------------------|----------------|
| `radivine-dbus` *(new)* | `crates/radivine-dbus` | **Yes** | Pure-Rust, zero-Radivine-dependency `org.freedesktop.Notifications` and `org.freedesktop.FileManager1` D-Bus wire-protocol client (`call_desktop_notify`, `call_show_item_properties`, `call_show_items`), extracted from `crates/radivine-actions/src/dbus.rs:1-492` (`E6`) and re-exported by `radivine-actions::dbus`. Depends only on `thiserror`. |
| `radivine-convert` *(new)* | `crates/radivine-convert` | **Yes** | Pure-Rust, codec-free, desktop-agnostic conversion core: `FormatId`, magic-byte sniffing, `FeatureSet`, `BackendDescriptor`, `Availability`, `CapabilityCatalog` & deterministic routing, `ConversionOptions` (`SameFormatPolicy`), `QualityIntent`, `Preset`, `ConvertSettings` (`convert.toml`), `ConversionEngine` (`plan`, `inspect`, `convert_item`), `ItemControl`, `CommitRequest`, & `CommitPermit`, private staging & two-rung atomic output commit (`output`), and `ConversionError`. |
| `radivine-convert-image` *(new)* | `crates/radivine-convert-image` | **Yes** (C FFI isolated in `libwebp-sys` / audited minimal wrapper) | Built-in image backend (`builtin.image`): `descriptor()`, header-only `inspect()`, pure-Rust PNG/JPEG/WebP decode with `image::Limits`, EXIF auto-orientation, RGBA8→RGB8 alpha flattening, resize (`image::imageops`), PNG/JPEG/lossless-WebP encode (`image`), lossy-WebP encode (`libwebp`), and ICC/EXIF RIFF/container preservation. |
| `radivine-jobs` *(new)* | `crates/radivine-jobs` | **Yes** | General-purpose durable job contract & client library: `JobId`, `JobState`, `ItemState`, transition validators, `JobSpec`, `RequestId`, lossless `WirePath`, JSON-lines IPC protocol v1 types (`WorkerRequest`, `WorkerResponse`, `JobUpdate`), `JobsClient` (sync commands + `Subscription`), `WorkerLauncher` (3-tier cgroup-safe launch), and `JobsSettings` (`config.toml [jobs]`). |
| `radivine-worker` *(new binary)* | `apps/radivine-worker` | **Yes** | On-demand background job daemon & CLI (`serve`, `status`, `list`, `show`, `cancel`, `retry`, `submit-convert`): single-instance `worker.lock` (`flock` + `O_CLOEXEC`), Unix socket server (`SO_PEERCRED` check, `16 MiB` max `submit` frame / `64 KiB` non-`submit` frame, 10 Hz subscription coalescing), `JobStore` (`rusqlite` bundled WAL), startup recovery (`C0`–`C6`), round-robin scheduler, `ResourceGovernor` + watchdog commit fence (`Mutex<ItemPhase>`), `ConvertImageHandler`, and D-Bus completion notifications (`radivine-dbus`). |
| `radivine-core` *(existing)* | `crates/radivine-core` | **Yes** | Action trait (`E1` `description_for`), `FileFacts` (`E2` `is_uninspected`), and lossless RFC 8089 URI conversion (`E8` `local_path_to_file_uri`, `normalize_cli_arg_to_uri`). |
| `radivine-actions` *(existing)* | `crates/radivine-actions` | **Yes** | Built-in actions, `default_menu()` (`E4`), `register_builtins_with_options` + `registry_from_options` (`E3`), `pub use radivine_dbus as dbus;` (`E6`), and `convert` module (`ConvertMenuContext`, `ConvertAction`, `AdvancedConvertAction`). |
| `radivine` *(existing binary)* | `apps/radivine` | **Yes** | Wheel daemon & CLI (`E8` lossless `OsString` in `Commands::Invoke { paths: Vec<OsString>, at: Option<String> }` and `normalize_cli_paths`); constructs `ConvertMenuContext` with static built-in `CapabilityCatalog`. Reads **zero** `convert.toml` settings. |
| `radivine-control` *(existing binary)* | `apps/radivine-control` | **Yes** | Control Center GUI (`ControlPage::{Conversion, Jobs}`, `--page jobs\|conversion`, `radivine-control-jobs` subscriber thread) and standalone `radivine-control convert [--stdin] [PATHS...]` window (bypasses `ControlInstanceLock::acquire_or_notify_existing()`). |

---

## 3. Dependency Graph and Forbidden Dependency Edges

### Allowed v0.2.0 Workspace Dependency Graph

```text
radivine-wheel          (pure engine; ONLY smallvec at runtime)
radivine-render         (tiny-skia, cosmic-text, resvg; +3 wheel SVGs in E5)
radivine-platform       (X11/Nemo integration; unchanged)
radivine-core           (Action, Context, MenuSpec, Enricher; E1, E2, E8)
radivine-dbus           (pure-Rust D-Bus client: thiserror ONLY; NO Radivine crates)
radivine-convert        (pure Rust: serde, toml, toml_edit, thiserror, rustix[fs]; NO codecs, NO Radivine crates)
  ▲
  └── radivine-convert-image (image, image-webp, zune-jpeg, libwebp wrapper, img-parts/exif)
radivine-jobs           (serde, serde_json, thiserror, rustix[fs,process,net]; NO codecs, NO rusqlite)
  ▲
  ├── radivine-actions  (depends on radivine-core, radivine-dbus, radivine-convert, radivine-convert-image, radivine-jobs)
  │     ▲
  │     └── apps/radivine         (depends on radivine-wheel, radivine-render, radivine-core, radivine-actions, radivine-platform)
  ├── apps/radivine-worker        (depends on radivine-jobs, radivine-convert, radivine-convert-image, radivine-dbus, rusqlite[bundled])
  └── apps/radivine-control       (depends on radivine, radivine-core, radivine-actions, radivine-platform, radivine-convert, radivine-convert-image, radivine-jobs)
```

### Explicitly Forbidden Dependency Edges (Enforced in CI — Discrepancy `X7`)

1. `radivine-wheel` MUST NOT depend on any workspace crate or external runtime crate other than `smallvec`.
2. `radivine-dbus` and `radivine-convert` MUST NOT depend on `radivine-core`, `radivine-actions`, `radivine-jobs`, `radivine-platform`, `radivine-wheel`, `radivine-render`, `image`, `libwebp-sys`, `rusqlite`, `eframe`, or X11 crates.
3. `radivine-jobs` MUST NOT depend on `radivine-convert`, `radivine-convert-image`, `radivine-core`, `radivine-actions`, `rusqlite`, or `eframe`.
4. `apps/radivine` (wheel daemon) and `apps/radivine-control` (Control Center) MUST NOT depend on `rusqlite` and MUST NOT invoke image pixel decoders (`radivine-control` only calls header-level `ConversionEngine::inspect` on a background thread, never full pixel decode).
5. `apps/radivine-worker` MUST NOT depend on `radivine-actions`, `radivine-core`, `radivine-wheel`, `radivine-render`, `radivine-platform`, `eframe`, `egui`, or `winit` (verified via `cargo tree -p radivine-worker`).

---

## 4. Non-Negotiable File Safety Invariants

Every implementation commit in `radivine-convert` and `radivine-worker` must preserve these invariants ([`SECURITY_AND_FILE_SAFETY.md`](SECURITY_AND_FILE_SAFETY.md) §3):

1. **`INV-SRC` (Source Immutability & Symlink Policy):**
   - Source files are opened `O_RDONLY | O_NONBLOCK | O_CLOEXEC` (**without** `O_NOFOLLOW`, so user-selected symlinks to regular files work seamlessly, matching `radivine_core::enrich::inspect_regular_file` at `enrich.rs:483-538`).
   - Immediately after `open`, `fstat(src_fd)` requires `S_ISREG` (rejecting FIFOs, sockets, device nodes, and directories with `ConversionError::SourceNotRegular`) and records `(dev, ino, size, mtime_ns)`, which is re-verified after decode (`ConversionError::SourceChanged`).
   - Sources are never opened for writing, truncated, chmodded, renamed, or unlinked.
   - All **destination**, **staging directory** (`.rvstage-*`), and **temporary file** (`item-*.part`) opens/stats/unlinks strictly enforce `O_NOFOLLOW` / `AT_SYMLINK_NOFOLLOW`.
2. **`INV-NOOVERWRITE` (Two-Rung Atomic Output Publication Only):**
   - **Rung A:** `rustix::fs::renameat_with(stage_dir_fd, temp_name, dest_dir_fd, candidate, RenameFlags::NOREPLACE)` (Linux `RENAME_NOREPLACE`, macOS `RENAME_EXCL`), followed immediately by **`fsync(stage_dir_fd)`** then **`fsync(dest_dir_fd)`**.
   - **Rung B:** `rustix::fs::linkat(stage_dir_fd, temp_name, dest_dir_fd, candidate, AtFlags::empty())` → **`fsync(dest_dir_fd)`** → `unlinkat(stage_dir_fd, temp_name, AtFlags::empty())` → **`fsync(stage_dir_fd)`**.
   - **No Rung C copy fallback and no check-then-replace fallback:** If both Rung A and Rung B return `ENOSYS`/`EINVAL`/`EOPNOTSUPP`/`EPERM`/`EXDEV`, `commit_no_replace` **must fail cleanly** with `ConversionError::FilesystemUnsupported` (leaving the destination untouched).
   - `std::fs::rename` is **banned** in `radivine-convert` and `radivine-worker`.
3. **`INV-TEMP-RDWR` (Two-Phase Staging Journaling, Canonical Temp Naming, `O_RDWR` Creation & Journal Ordering):**
   - **Phase 3a (`StageIntent` before `mkdirat`):** Before creating a staging directory `.rvstage-<job_id>-<rand16>` (`^\.rvstage-[0-9]+-[0-9a-f]{16}$`), the engine calls `ctl.journal_stage_intent(StageIntentJournal { dest_dir, stage_dir_name })` so SQLite holds a durable reference to `(dest_dir, stage_dir_name)` before `mkdirat` is invoked (`C0b`).
   - **Phase 3b (`StageCreated` before temp creation):** After `mkdirat(dest_dir_fd, stage_dir_name, 0o700)` + `fsync(dest_dir_fd)`, the engine opens `stage_dir_fd` with `O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`, verifies `fstat(stage_dir_fd)` (`S_ISDIR`, `st_uid == geteuid()`, and `(st_mode & 0o022) == 0` or private user mount), and calls `ctl.journal_stage_created(StageCreatedJournal { dest_dir, stage_dir_name, stage_dev, stage_ino })` **before** creating any temporary file (`C0c`).
   - **Phase 4 (`TempCreated` before writing bytes):** Temp files use the single canonical name `item-<idx>-<rand16>.part` (`^item-[0-9]+-[0-9a-f]{16}\.part$`) and are opened inside `stage_dir_fd` with `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC` (mode `0600`). Immediately after `openat`, the engine calls `fstat(temp_fd)` and synchronously records `TempCreated { temp_name, temp_dev, temp_ino }` in SQLite **before** writing any bytes.
   - At Step 9 (after `fsync(temp_fd)`), the engine rewinds the same `O_RDWR` `PendingOutput` (`SeekFrom::Start(0)`) and runs `backend.inspect()` to verify dimensions and format before calling `ctl.begin_commit(...)`.
4. **`INV-DURABILITY` (Lock-Free-I/O `CommitRequest`/`CommitPermit` & Both-Directory Durability Ordering):**
   - `radivine-convert` constructs `let req = CommitRequest::new(attempt, candidate.as_os_str(), temp_dev, temp_ino);` (`pub(crate) fn new`) and passes it to `ctl.begin_commit(req)`.
   - `ItemControl::begin_commit` **never holds `Mutex<ItemPhase>` during SQLite or filesystem I/O**:
     1. **Step A (O(1) lock #1):** Checks that `*phase` is `PreCommit` or `InCommit { timeout_observed: false, .. }` (if `InCommit { timeout_observed: true, .. }`, transitions to `AbortedBeforeCommit(TimedOut)` and rejects the collision retry immediately), transitions to `ItemPhase::JournalingCommit { attempt, candidate }`, and **releases `Mutex<ItemPhase>`**.
     2. **Step B (unlocked I/O):** Executes the SQLite `Committing` update (`final_name = ?candidate`) under `PRAGMA synchronous = FULL` **without** holding `Mutex<ItemPhase>`.
     3. **Step C (O(1) lock #2):** Re-acquires `Mutex<ItemPhase>`, verifies the phase is still `JournalingCommit { attempt }` (not aborted by watchdog/cancel during Step B), transitions to `ItemPhase::InCommit { attempt, candidate, temp_dev, temp_ino, timeout_observed: false }`, and calls `Ok(req.authorize())` to return `CommitPermit<'a>` (`!Send + !Sync`, private fields, invariant over `'a`) **before** `commit_no_replace` is called.
   - **Both `fsync(stage_dir_fd)` and `fsync(dest_dir_fd)`** are executed **before** `convert_item` returns `Ok(ItemResult)` and **before** `state = 'succeeded'` or `temp_name = NULL` is persisted in SQLite. Similarly, `fsync(dest_dir_fd)` is executed after `unlinkat(dest_dir_fd, stage_dir_name, AT_REMOVEDIR)` **before** `stage_dir_name = NULL` is persisted in SQLite.
5. **`INV-SAFE-CLEANUP` (`cleanup_temp_safely` Preconditions, Directory `fsync`, & No Pattern Deletion):**
   - Because POSIX has no `funlinkat(fd)` syscall, `unlinkat(stage_dir_fd, temp_name, 0)` is only executed when:
      1. The caller holds the exclusive `worker.lock` (`flock(LOCK_EX)`).
      2. `stage_dir_name` and `temp_name` match strict single-component regexes (`^\.rvstage-[0-9]+-[0-9a-f]{16}$` and `^item-[0-9]+-[0-9a-f]{16}\.part$`).
      3. `(stage_dev, stage_ino)` is non-`NULL` in SQLite and `fstat(stage_dir_fd)` matches `(stage_dev, stage_ino)`, `S_ISDIR`, `st_uid == geteuid()`, and namespace exclusivity (`(st_mode & 0o022) == 0` or private user mount).
      4. `(temp_dev, temp_ino)` is non-`NULL` in SQLite and `fstatat(stage_dir_fd, temp_name, AT_SYMLINK_NOFOLLOW)` matches `(temp_dev, temp_ino)` and `S_ISREG`.
   - After `unlinkat(stage_dir_fd, temp_name, 0)`, the worker calls **`fsync(stage_dir_fd)`** before clearing `temp_name = NULL, temp_dev = NULL, temp_ino = NULL` in SQLite. After `unlinkat(dest_dir_fd, stage_dir_name, AT_REMOVEDIR)`, the worker calls **`fsync(dest_dir_fd)`** before clearing `stage_dir_name = NULL, stage_dev = NULL, stage_ino = NULL` in SQLite.
   - If any check fails—or on a `C0b` crash before `StageCreated` where `stage_dir_name` is non-empty, or a `C1` crash between `openat(O_EXCL)` and durable `TempCreated` where `(temp_dev, temp_ino)` was never journaled—**never invent an ownership guarantee and never unlink by filename pattern**: preserve the file/directory untouched and record `OrphanedTempPreserved`.

---

## 5. Job Lifecycle and Crash Recovery Invariants

### Startup Recovery Table (`JOB_SYSTEM_DESIGN.md` §5)

On startup—**after** acquiring `flock(worker.lock, LOCK_EX)` and **before** accepting IPC connections—`radivine-worker` queries `WHERE state IN ('pending', 'running') OR stage_dir_name IS NOT NULL OR temp_name IS NOT NULL` so active rows, terminal `Failed(timed_out)` rows (`C5`), and terminal `Succeeded` rows with un-cleaned Rung B links or staging dirs (`C6`) are reconciled:

| Crash Point | Durable DB State on Disk | Recovery Action |
|-------------|--------------------------|-----------------|
| **`C0a`** (Before `StageIntent` durable) | `state = 'running'`, `stage_dir_name IS NULL`, `temp_name IS NULL` | Mark item `Interrupted` (or keep `Queued` if job was `Preparing` on same boot). |
| **`C0b`** (After `StageIntent` durable, before `StageCreated` durable) | `state = 'running'`, `stage_dir_name IS NOT NULL`, `stage_dev IS NULL`, `temp_name IS NULL` | Check `fstatat(dest_dir_fd, stage_dir_name, AT_SYMLINK_NOFOLLOW)`: if `ENOENT` (crash before `mkdirat`), `fsync(dest_dir_fd)` and clear `stage_dir_name = NULL`. If it exists: because `(stage_dev, stage_ino)` was never journaled, **never unlink any file inside it**. If `S_ISDIR`, `st_uid == geteuid()`, `(st_mode & 0o022) == 0`, and `unlinkat(dest_dir_fd, stage_dir_name, AT_REMOVEDIR)` succeeds (empty dir only), call **`fsync(dest_dir_fd)`** and clear `stage_dir_name = NULL`; otherwise preserve untouched, record `OrphanedTempPreserved`, clear `stage_dir_name = NULL` after persisting warning, and mark item `Interrupted`. |
| **`C0c`** (After `StageCreated` durable, before `openat(O_EXCL)`) | `state = 'running'`, `stage_dir_name IS NOT NULL`, `stage_dev IS NOT NULL`, `temp_name IS NULL` (empty dir) | Verify `stage_dir` against `(stage_dev, stage_ino)` + `0700` + `st_uid == geteuid()`, call `unlinkat(dest_dir_fd, stage_dir_name, AT_REMOVEDIR)` + **`fsync(dest_dir_fd)`**, clear `stage_dir_name = NULL, stage_dev = NULL, stage_ino = NULL`, and mark item `Interrupted`. |
| **`C1`** (After `openat(O_EXCL)`, before `TempCreated` durable) | `state = 'running'`, `stage_dir_name IS NOT NULL`, `stage_dev IS NOT NULL`, `temp_name IS NULL` (unjournaled `item-<idx>-<rand16>.part` inside `.rvstage-*`) | **Never delete unjournaled files by filename pattern.** When `unlinkat(..., AT_REMOVEDIR)` returns `ENOTEMPTY`, preserve `.rvstage-*` and the unjournaled file untouched, record `OrphanedTempPreserved`, clear `stage_dir_name = NULL, stage_dev = NULL, stage_ino = NULL` after persisting warning, and mark item `Interrupted`. |
| **`C2`** (After `TempCreated`, before `Committing`) | `state = 'running'`, `temp_name IS NOT NULL`, `final_name IS NULL` | Call `cleanup_temp_safely` + **`fsync(stage_dir_fd)`**; clear `temp_name = NULL, temp_dev = NULL, temp_ino = NULL`; remove empty `.rvstage-*` dir + **`fsync(dest_dir_fd)`** and clear `stage_dir_* = NULL`; mark item `Interrupted`. |
| **`C3`** (After `Committing`, before kernel `renameat2`/`linkat`) | `state = 'running'`, `final_name IS NOT NULL`; `dest_dir/final_name` absent or `(st_dev, st_ino) != (temp_dev, temp_ino)` | Leave any foreign `dest_dir/final_name` untouched; call `cleanup_temp_safely` on `.rvstage-*/temp_name` + **`fsync(stage_dir_fd)`**; clear `temp_name = NULL, temp_dev = NULL, temp_ino = NULL`; remove empty `.rvstage-*` + **`fsync(dest_dir_fd)`** and clear `stage_dir_* = NULL`; mark item `Interrupted` (or `Failed(timed_out)` if `timed_out_in_commit = 1`). |
| **`C4`** (After kernel `renameat2`/`linkat`, before directory `fsync`s + `Succeeded` durable) | `state = 'running'`, `final_name IS NOT NULL`; `dest_dir/final_name` exists, is `S_ISREG`, and `(st_dev, st_ino) == (temp_dev, temp_ino)` | Call `fsync(file_fd)` and **`fsync(dest_dir_fd)`**. If `.rvstage-*/temp_name` still exists with `(temp_dev, temp_ino)` (Rung B crash between `linkat` and `unlinkat`), unlink it via `cleanup_temp_safely` + **`fsync(stage_dir_fd)`**; otherwise call **`fsync(stage_dir_fd)`** directly. Clear `temp_name = NULL, temp_dev = NULL, temp_ino = NULL`; remove empty `.rvstage-*` + **`fsync(dest_dir_fd)`** and clear `stage_dir_* = NULL`; mark item **`Succeeded`**. |
| **`C5`** (Pre-commit / `JournalingCommit` watchdog timeout before process exit) | `state = 'failed'`, `error_code = 'timed_out'`, `temp_name IS NOT NULL` | Call `cleanup_temp_safely` + **`fsync(stage_dir_fd)`**; clear `temp_name = NULL, temp_dev = NULL, temp_ino = NULL`; remove empty `.rvstage-*` + **`fsync(dest_dir_fd)`** and clear `stage_dir_* = NULL` when no items remain. |
| **`C6`** (After `Succeeded` durable, before Rung B `unlinkat(temp_name)` or `AT_REMOVEDIR` synced) | `state = 'succeeded'`, `temp_name IS NULL` (or non-`NULL` if Rung B cleanup was interrupted) | If `temp_name IS NOT NULL` or `stage_dir_name IS NOT NULL`, complete `cleanup_temp_safely` + **`fsync(stage_dir_fd)`** and `AT_REMOVEDIR` + **`fsync(dest_dir_fd)`**, then clear `temp_name = NULL, stage_dir_* = NULL`. |

### Watchdog & Commit Fencing Invariants (`JOB_SYSTEM_DESIGN.md` §7.5)

1. **Lock-Free-I/O Linearization Point:** `Mutex<ItemPhase>` is never held during SQLite or filesystem I/O. `ItemControl::begin_commit` transitions `PreCommit`/`InCommit(timeout_observed: false)` → `JournalingCommit { attempt, candidate }` under short O(1) lock #1, flushes the `Committing` row (`PRAGMA synchronous=FULL`) **unlocked**, and re-acquires short O(1) lock #2 to transition `JournalingCommit { attempt }` → `InCommit { attempt, candidate, temp_dev, temp_ino, timeout_observed: false }` and call `req.authorize()`.
2. **Pre-Commit / `JournalingCommit` Timeout:** If the watchdog fires while `*phase` is `PreCommit` or `JournalingCommit { .. }`, it sets `*phase = AbortedBeforeCommit(TimedOut)` in O(1) without blocking (even if Step B's SQLite write is stalled) and records `state = 'failed', error_code = 'timed_out'` (using a bounded `≤ 500 ms` DB busy timeout) while keeping `(stage_*, temp_*)` columns non-`NULL` for `C5`/`C2` startup cleanup.
3. **In-Commit Timeout & Collision-Retry Fencing (`timeout_observed = true`):** If the watchdog fires while `*phase` is `InCommit { .. }`, it sets `timeout_observed = true` in O(1) and `timed_out_in_commit = 1` in SQLite (bounded `≤ 500 ms` timeout):
   - If the in-flight `attempt` returns `EEXIST` (collision), the next `begin_commit(attempt + 1)` call sees `InCommit { timeout_observed: true, .. }`, transitions to `AbortedBeforeCommit(TimedOut)`, and returns `Err(ConversionError::TimedOut)` **before** journaling or attempting any new candidate (`C3` recovery then cleans up `temp_name` and preserves the foreign file).
   - If the in-flight `attempt` was executing in the kernel, replacement-worker recovery (`C3` vs `C4`) determines after old process exit whether `(temp_dev, temp_ino)` was published to `dest_dir/final_name`.
4. **Bounded Drain, Singleton Handoff, & Honest OS Boundary:** On watchdog expiry, the worker calls `governor.begin_drain()` (`draining = true` + `notify_all()` to wake blocked `reserve()` callers), waits **at most `5 s`** for responsive threads (never `join()`ing the timed-out thread and never blocking beyond `5 s` on a wedged DB lock), and calls `std::process::exit(75)` while holding `worker.lock` (`O_CLOEXEC`) until the kernel destroys the process.
   - **Explicit C2 Concerns to Honour During C2 Implementation:**
     1. **SQLite WAL Power-Loss Durability for `StageIntent`, `StageCreated`, and `TempCreated`:** Under `PRAGMA journal_mode=WAL; synchronous=NORMAL`, committed transactions survive all process crashes (`SIGKILL`, `abort()`, `exit(75)`), but sudden power loss or an OS kernel panic before a WAL `fsync` can roll back `StageIntent`, `StageCreated`, or `TempCreated` even if `mkdirat` or `openat(O_EXCL)` already reached disk. In C2, `JobStore` must either (a) evaluate `PRAGMA synchronous=FULL` for `StageIntent`/`StageCreated`/`TempCreated` (or connection-wide `synchronous=FULL` since terminal writes are batched) or (b) explicitly document that power loss before WAL sync falls into the unjournaled `C0a`/`C0b`/`C1` orphan path (leaving at most an empty `0700` `.rvstage-*` dir or `0600` `item-*.part` file on disk, never overwriting or deleting user files because `Committing` always uses `PRAGMA synchronous=FULL`).
     2. **Real-World Kernel `D`-State Limits on Watchdog Termination:** When a worker thread is stuck in uninterruptible kernel sleep (`TASK_UNINTERRUPTIBLE` / `D` state on a stalled hard-mounted NFS or FUSE filesystem), neither `std::process::exit(75)` nor `SIGKILL` can destroy the kernel task or release its kernel-held `flock(worker.lock)` / file descriptors until the kernel filesystem driver times out or the mount recovers. Never claim user-space preemption stronger than the OS kernel provides.

---

## 6. Milestone Execution Order (C1–C5)

Per owner decision `D1`, implementation proceeds in five strictly sequential milestones, each requiring **separate, explicit owner authorization**:

| Milestone | Scope Summary | Touches Production Apps/Daemon? |
|-----------|---------------|---------------------------------|
| **C1** | Verification spikes `P1`–`P9` (gate) + `crates/radivine-convert` + `crates/radivine-convert-image` + `tools/convert-cli` + `tools/convert-corpus` + `fuzz/` targets | **No** (libraries & dev tools only) |
| **C2** | Verification spike `P10` + `crates/radivine-dbus` (`E6`) + `crates/radivine-jobs` + `apps/radivine-worker` + `packaging/radivine-worker.service` + crash/fault harness | **No** (worker, jobs, & D-Bus crates only; daemon and CC behavior unchanged) |
| **C3** | `radivine-core` (`E1`, `E2`, `E8`) + `radivine-actions` (`E3`, `E4`, `convert` actions) + `radivine-render` (`E5` icons) + `apps/radivine` wiring & Invoke parity | **Yes** (`radivine` daemon & CLI; CC untouched) |
| **C4** | `apps/radivine-control`: `ui/theme.rs` extraction, `Jobs` page, `Conversion` settings page, `radivine-control convert` window (`E7`), register `AdvancedConvertAction` | **Yes** (`radivine-control` & `AdvancedConvertAction` registration) |
| **C5** | Performance benchmarks vs. budgets, ≥ 1 h fuzzing, 1 h soak test, `.deb` packaging (`build-deb.sh`, `debian/*`), CI dependency-graph script (`X7`), ADR promotion to `docs/DECISIONS.md`, `v0.2.0` release prep | **Yes** (packaging, docs, release prep) |

---

## 7. Mandatory C1 Verification Spikes (`P1`–`P10`)

At the start of Milestone C1 (once authorized), run spikes `P1`–`P9` in `spikes/convert-*` (excluded from workspace) and record results in `docs/planning/v0.2.0/spikes/C1-spikes.md`. **Stop and report to the owner if any gate fails:**

| ID | Verification Target | Gate / Pass Criterion |
|----|---------------------|-----------------------|
| **P1** | `libwebp-sys` vendored static build on Ubuntu 24.04 / Mint 22 & CI | Bundled `libwebp` ≥ 1.3.2; builds cleanly with `cc` only; binary size delta recorded |
| **P2** | Lossy WebP RIFF muxing (`ICCP` + `EXIF` chunks) | Byte-exact ICC & normalized EXIF round-trip verified by `webpinfo`/`exiftool` and browser/viewer decode |
| **P3** | `image::Limits` pre-allocation rejection on 65535×65535 PNG/JPEG/WebP bombs | Rejected before pixel allocation; process RSS delta < 50 MB |
| **P4** | `zune-jpeg` behaviour on CMYK/YCCK/Adobe-inverted, 12-bit, and arithmetic JPEGs | Correct RGB conversion or clean `ConversionError::UnsupportedFeature`; zero panics |
| **P5** | ICC profile byte-exact round-trip across PNG↔JPEG↔WebP (lossless & lossy) | Extracted profile bytes match source SHA-256 |
| **P6** | `image` `JpegEncoder` vs. `jpeg-encoder` (chroma subsampling, speed, ICC/EXIF) | Encoder choice and quality-intent table (`Small`/`Balanced`/`High`) recorded |
| **P7** | Peak RSS and wall time for 12/24/50/100 MP conversions | Calibrates `ResourceGovernor` memory factor (`2.2 × decoded + 16 MiB`) and watchdog formula |
| **P8** | Rung A (`RENAME_NOREPLACE`) and Rung B (`linkat` + `unlinkat`) across ext4, tmpfs, vfat/exFAT, FUSE/gvfs | Exact `errno` table recorded; confirms Rung A → Rung B → `FilesystemUnsupported` behaviour |
| **P9** | `cargo deny check` and `cargo audit` on proposed dependency graph | Zero advisories; all licenses compatible with `deny.toml` (BSD-3 notice prepared for `libwebp`) |
| **P10** | *(C1/C2 gate)* `radivine-worker.service` and `systemd-run --user` cgroup survival across `systemctl --user restart radivine.service` | Worker PID and running job survive daemon restart unchanged |

---

## 8. Existing v0.1.1 Code Integration Points (`E1`–`E8`)

All existing v0.1.1 signatures and behavior must remain backwards-compatible:

| Ext | File & Verified v0.1.1 Lines | Existing Symbol / Behavior | v0.2.0 Additive Change |
|-----|------------------------------|----------------------------|------------------------|
| **E1** | `crates/radivine-core/src/action.rs:53-64, 182-201`<br/>`crates/radivine-core/src/registry.rs:317-333` | `ActionMeta { pub description: Option<String>, .. }`; `pub trait Action` (`meta()`, `is_visible()`, `enablement()`, `run()`); `build_action_leaf` reads `action.meta().description.clone()` | Add default method `fn description_for(&self, _ctx: &BasicContext) -> Option<Cow<'_, str>> { self.meta().description.as_deref().map(Cow::Borrowed) }` on `Action`; call `action.description_for(basic_ctx).map(Cow::into_owned)` in `build_action_leaf`. |
| **E2** | `crates/radivine-core/src/context.rs:167-219`<br/>`crates/radivine-core/src/enrich.rs:22, 130-137, 301-308` | `pub struct FileFacts { pub kind: FileKind, pub size: Option<u64>, pub readable: bool, pub text: TextVerdict, pub sniffed_mime: Option<&'static str>, pub error: Option<String> }`; `pub const MAX_ENRICHED_ITEMS: usize = 64;`; `FileFacts::timed_out()` sets `error: Some("Couldn't inspect the file".to_string())` | Export `pub const CAP_EXCEEDED_REASON: &str = "Too many items to inspect";` and `pub const ENRICH_TIMEOUT_REASON: &str = "Couldn't inspect the file";` plus `FileFacts::is_uninspected(&self) -> bool` (`matches!(self.kind, FileKind::Unknown) && matches!(self.error.as_deref(), Some(CAP_EXCEEDED_REASON \| ENRICH_TIMEOUT_REASON))`). |
| **E3** | `crates/radivine-actions/src/lib.rs:72-91, 181-204` | `register_builtins`, `register_builtins_with_config`, `register_builtins_with_preview`, `builtin_registry`, `registry_from_actions_config`, `registry_from_actions_and_preview_config` | Add `register_builtins_with_options(registry, &ActionsConfig, &PreviewSettings, &ConvertMenuContext)` and `registry_from_options(&ActionsConfig, &PreviewSettings, &ConvertMenuContext)`; existing 6 functions keep their exact signatures and delegate with a default `ConvertMenuContext`. |
| **E4** | `crates/radivine-core/src/registry.rs:13-33`<br/>`crates/radivine-actions/src/lib.rs:31-69` | `pub fn default_menu() -> Vec<MenuSpec>` (`[0] preview`, `[1] copy`, `[2] properties`, `[3] custom`) where `MenuSpec::Submenu` has fields `{ id, label, icon, description, default, children }` | Insert `MenuSpec::Submenu { id: Arc::from("radivine.convert"), label: "Convert".into(), icon: Some(IconKey::from("lucide:arrow-right-left")), description: Some("Convert files".into()), default: None, children: vec![...] }` at index `1`. Pruned automatically for non-image selections (`registry.rs:290-294`). |
| **E5** | `assets/wheel-icons/`<br/>`crates/radivine-render/src/cache.rs:47-95` | `const EMBEDDED_ICONS: &[(&str, &str)]` in `cache.rs` | Add `arrow-right-left.svg`, `sliders-horizontal.svg`, `file-image.svg` to `assets/wheel-icons/` and register `"lucide:arrow-right-left"`, `"lucide:sliders-horizontal"`, `"lucide:file-image"` in `EMBEDDED_ICONS`. **Never touch `assets/branding/` or `assets/icons/`.** |
| **E6** | `crates/radivine-actions/src/dbus.rs:1-492`<br/>`crates/radivine-actions/src/lib.rs:8, 25-27` | `call_desktop_notify`, `call_show_item_properties` in `radivine_actions::dbus` | Extract `dbus.rs` into `crates/radivine-dbus`, add `call_show_items(uris: &[String])` (`org.freedesktop.FileManager1.ShowItems`), and `pub use radivine_dbus as dbus;` in `radivine-actions`. |
| **E7** | `apps/radivine-control/src/lib.rs:17-31`<br/>`apps/radivine-control/src/main.rs:58-73, 147-159`<br/>`apps/radivine-control/src/single_instance.rs:39-42` | `ControlPage` (`Overview`, `Activation`, `Appearance`, `Actions`, `Diagnostics`, `About`), `--page <slug>`, `ControlInstanceLock::acquire_or_notify_existing(&control_sock, explicit_page)` | Add `ControlPage::{Conversion, Jobs}`, `--page jobs\|conversion`, and `radivine-control convert [--stdin] [PATHS…]` subcommand that **bypasses `ControlInstanceLock::acquire_or_notify_existing()`** before launching its eframe window. |
| **E8** | `crates/radivine-core/src/context.rs:334-380`<br/>`apps/radivine/src/main.rs:58-66, 493-502` | `local_path_to_file_uri(path: &Path)` uses `path.to_string_lossy()`; `Commands::Invoke { paths: Vec<String>, at: Option<String> }`; `normalize_cli_paths(raw_args: &[OsString])` | Percent-encode `path.as_os_str().as_bytes()` directly in `local_path_to_file_uri`; change `Commands::Invoke.paths` to `Vec<OsString>` so `radivine invoke` passes raw `&[OsString]` to `normalize_cli_paths`. |

---

## 9. Security, Resource, and IPC Guardrails

### 9.1 Untrusted Decode Isolation & Disk Precheck
- **Untrusted Decode Isolation (`R1`):** Only `radivine-worker` performs full image pixel decoding. `radivine` daemon performs zero image header or pixel parsing beyond existing 512-byte magic-MIME sniffing (`enrich.rs:648`). `radivine-control convert` performs bounded header-only inspection (`ConversionEngine::inspect`, capped at 1,000 items, zero pixel allocation).
- **Disk Precheck (`R3`):** `fstatvfs(dest_dir_fd)` free bytes ≥ `max(16 MiB, 2 × source_size)` before each item; fast-fail remaining items targeting the same directory after 3 consecutive `NoSpace` or `DestinationNotWritable` errors.

### 9.2 Authoritative Resource & Safety Limits Table (`CONVERSION_PRODUCT_SPEC.md` §9.4)

| Limit / Setting | Config Key | Default | User-Configurable Range | Backend / Format Limit | Hard Safety Ceiling |
|-----------------|------------|---------|-------------------------|------------------------|---------------------|
| Max source pixels | `convert.toml [limits] max_pixels` | `100_000_000` (`100 MP`) | `1_000_000 ..= 500_000_000` (`1..=500 MP`) | `builtin.image` default `100 MP` (up to `500 MP` when user-configured) | `500_000_000` (`500 MP`) |
| Max source/output dimension | `convert.toml [limits] max_dimension` | `32,768` px | `64 ..= 65,500` px | WebP output: `16,383` px; JPEG output: `65,500` px | `65,500` px |
| Max source file size | `convert.toml [limits] max_file_mb` | `512 MiB` | `1 ..= 2,048 MiB` | — | `2,048 MiB` |
| Worker concurrent memory budget | `config.toml [jobs] memory_budget_mb` | `0` (Auto: `clamp(25% MemTotal, 256 MiB, 1024 MiB)`) | `0` (Auto) or `256 ..= 8,192 MiB` | — | `8,192 MiB` |
| Per-item exclusive allocation limit | Derived (`ResourceGovernor` + `image::Limits.max_alloc`) | `estimate(100 MP)` (`≈ 900 MiB`) | Scales with `max_pixels` (`estimate = w·h·bpp_decoded × 2.2 + 16 MiB`) | Items with `estimate > memory_budget_mb` and `pixels ≤ max_pixels` run **exclusively** (`in_use == 0`, FIFO queue) with `image::Limits.max_alloc = Some(estimate.memory_bytes)` | `estimate(500 MP)` (`≈ 4.4 GiB`) |

### 9.3 Authoritative Worker IPC & Stdin Bounds Contract (`JOB_SYSTEM_DESIGN.md` §6)

- `worker.sock` created with mode `0600` inside `$XDG_RUNTIME_DIR/radivine/` (`0700`); every client connection verified via `SO_PEERCRED` (`peer.uid == geteuid()`).
- Before idle exit, `radivine-worker` unlinks `worker.sock` and drains `accept()` non-blockingly while holding `worker.lock`.

| Channel / Field | Hard Limit | Enforcement & Safe Rejection Behavior |
|-----------------|------------|---------------------------------------|
| `submit` request frame (client → worker) | **`16 MiB`** (`16,777,216` bytes) | Bounded `Read::take` reader; if newline not reached within `16 MiB`, respond `{"v":1,"status":"error","code":"frame_too_large","message":"request exceeds maximum frame size"}` and close connection without allocating further. |
| Non-`submit` request frame (`cancel`, `retry`, `list`, `get`, `clear_history`, `subscribe`, `reload_settings`, `status`) | **`64 KiB`** (`65,536` bytes) | Bounded line reader; reject > `64 KiB` with `"code":"frame_too_large"` and close connection. |
| Command response frame (worker → client, including paged `get` with `limit <= 200`) | **`4 MiB`** (`4,194,304` bytes) | Bounded client reader; `get` pages at most `200` items per call and truncates per-item `detail` to `≤ 1,024` UTF-8 bytes. |
| Subscription event frame (worker → client `JobUpdate`) | **`64 KiB`** (`65,536` bytes) | Bounded client reader; each `JobUpdate` carries summary counters plus at most 1 item status transition. |
| Paths per job (`JobSpec.items`) | **`1 ..= 10,000`** items (`≤ 4,096` raw OS bytes per path) | Validated before DB transaction; `0` items → `"code":"empty_job"`, `> 10,000` items → `"code":"batch_too_large"`, path `> 4,096` bytes → `"code":"invalid_path"`. |
| Queue admission caps | **`256`** active/queued jobs · **`50,000`** total pending items | Checked before `INSERT`; rejected with `"code":"queue_full"`. |
| `radivine-control convert --stdin` pipe | **`≤ 10,000`** lines · **`≤ 16 KiB`** per line · **`≤ 16 MiB`** total stdin bytes | Bounded reader; excess bytes or > `10,000` paths abort stdin ingestion with an inline validation banner without OOM. |

---

## 10. Required Tests Per Milestone

| Milestone | Mandatory Verification Suite |
|-----------|------------------------------|
| **C1** | • All 6 cross-format pairs (PNG↔JPEG↔WebP) + lossless/lossy WebP quality monotonicity + EXIF orientations 1–8 + ICC byte-exact round-trip + alpha flattening.<br/>• `T-COMMIT-NO-PARTIAL` & `T-STAGE-DIR-FSYNC`: fault-injected Rung A (`RENAME_NOREPLACE` → `fsync(stage_dir)` → `fsync(dest_dir)`), Rung B (`linkat` → `fsync(dest_dir)` → `unlinkat` → `fsync(stage_dir)`), and `ConversionError::FilesystemUnsupported` when both fail (zero 0-byte/partial target files).<br/>• `T-VALIDATE-RDWR`: corrupting encoded bytes in `PendingOutput` before Step 9 fails with `ConversionError::Internal` and unlinks `item-<idx>-<rand16>.part` + `fsync(stage_dir)` before `begin_commit`.<br/>• `T-TEMP-INODE-RACE` & `T-STAGE-SHARED-DIR`: replacing `item-<idx>-<rand16>.part` or `.rvstage-<job>-<rand16>` before `cleanup_temp_safely` preserves the replacement file untouched and reports `OrphanedTempPreserved`; `0700` `.rvstage-*` inside a `1777` dir blocks other local UIDs; `StageIntent` → `StageCreated` → `TempCreated` hook sequence verified.<br/>• Source symlink test: symlink to regular image converts normally; symlink to FIFO/directory fails with `SourceNotRegular`; symlink at destination or inside `.rvstage-*` is rejected (`O_NOFOLLOW`).<br/>• `proptest` invariants (`INV-SRC`, `INV-NOOVERWRITE`, `INV-NOTEMP`, ≥ 500 cases) + bomb rejection (< 50 MB RSS) + `cargo tree` dependency-edge check. |
| **C2** | • `T-DURABILITY-ORDER`, `T-STAGE-CRASH-WINDOW`, & `T-STAGE-DIR-FSYNC`: `RADIVINE_WORKER_TEST_CRASH_AT` abort harness at `C0a`, `C0b` (before `mkdirat` and after `mkdirat` before `StageCreated`), `C0c`, `C1`, `C2`, `C3`, `C4`, `C5`, `C6` verifying `(stage_dev, stage_ino)` and `(temp_dev, temp_ino)` reconciliation, `C0b`/`C1` unverified/unjournaled orphan preservation (`OrphanedTempPreserved` without filename-pattern deletion), `C5`/`C6` cleanup via `WHERE state IN ('pending', 'running') OR stage_dir_name IS NOT NULL OR temp_name IS NOT NULL`, `PRAGMA synchronous = FULL` on `Committing`, and **both** `fsync(stage_dir_fd)` and `fsync(dest_dir_fd)` before `Succeeded` or journal clearing.<br/>• `T-WATCHDOG-FENCE`, `T-WATCHDOG-LOCK-NONBLOCKING`, & `T-WATCHDOG-COLLISION-FENCE`: injected decode/commit/SQLite hangs in `PreCommit`, `JournalingCommit` (stalling SQLite inside `record_committing_sync_full` and verifying the watchdog acquires `Mutex<ItemPhase>` in `< 10 ms` and exits within `≤ 5 s`), and `InCommit` (injecting timeout during `attempt = 0` followed by `EEXIST` and verifying `InCommit.timeout_observed == true` rejects `attempt = 1` with `Err(TimedOut)` without journaling or creating `"photo (1).webp"`, while in-flight `attempt = 0` is reconciled via `C3`/`C4`).<br/>• `T-LIFE-1` (client kill survival), `T-LIFE-2` & `T-CGROUP-DETACH` (survival across `systemctl --user restart radivine.service` for both static unit and `systemd-run --user` transient unit).<br/>• Idle-exit socket unlink/accept drain race test (`R5`), `WirePath` non-UTF-8 round-trip, IPC frame bounds (`16 MiB` `submit` / `64 KiB` non-`submit` / `"code":"frame_too_large"`), FIFO exclusive memory governor test (`A4`), and `cargo tree -p radivine-worker` independence check (`AC-C2.17`). |
| **C3** | • Text-selection `MenuTree` snapshot identical to v0.1.1; existing `m4_actions_acceptance` tests pass unchanged.<br/>• Visibility, progressive enablement (64-item batch enables as soon as the first enriched item confirms ≥ 1 image while items 1..64 are `Pending`), and `description_for` caption table tests (including `"Convert 2 of 3 images to WebP · 1 already WebP"`).<br/>• Zero-I/O enforcement test in `is_visible` / `description_for`.<br/>• `radivine invoke` non-UTF-8 filename end-to-end conversion test (`E8`). |
| **C4** | • View-model reducer unit tests (`JobUpdate` stream → Jobs page state), 10,000-item virtualized list frame-time test (< 50 ms).<br/>• `convert.toml` and `config.toml [jobs]` `toml_edit` round-trip, comment preservation, and external conflict detection.<br/>• Standalone `radivine-control convert` coexistence with open Settings/Jobs window (`ControlInstanceLock::acquire_or_notify_existing()` bypass) + `--stdin` bounds check + zero pixel decode assertion + `"Open Jobs"` spawning `radivine-control --page jobs`.<br/>• Existing `v0_1_1_acceptance` tests pass unchanged. |
| **C5** | • Full benchmark suite (`docs/BENCHMARKS.md`), ≥ 1 h fuzzing per target (8 targets), 1-hour soak test (0 source mutations, 0 overwrites, 0 temp leaks, RSS stability), `.deb` clean install & v0.1.1 upgrade test, `lintian`, and full workspace gate. |

---

## 11. Git Workflow and Commit Requirements (`AGENTS.md`)

1. **Preserve Existing Work (`AGENTS.md` §1.1):** Inspect `git status` and `git log --oneline` before starting any authorized work. Never use destructive Git commands (`git reset --hard`, `git checkout -- .`, `git push --force`).
2. **Commit Incrementally (`AGENTS.md` §1.2–§1.3):** Create atomic Conventional Commits (`feat(convert): ...`, `feat(jobs): ...`, `feat(worker): ...`, `test(convert): ...`, `docs: ...`) continuously as each logical unit is implemented and verified—never accumulate an entire milestone into a single commit.
3. **Pre-Commit Verification Gate (`AGENTS.md` §1.4):** Before every commit, review `git diff`, stage only relevant files, and pass:
   - `cargo fmt --all --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test` (focused tests during incremental commits; full `cargo test --workspace -- --test-threads=1` at milestone completion)
4. **Asset & Invariant Protection (`AGENTS.md` §2):**
   - Never modify, delete, rename, or regenerate any file under `assets/branding/` or `assets/icons/`.
   - Keep `crates/radivine-wheel` and `crates/radivine-render` `#![forbid(unsafe_code)]` and platform-independent.
   - Keep `input.passthrough` defaulting to `false`.
5. **Milestone Completion (`AGENTS.md` §1.6):** Write `docs/test-reports/C<n>.md`, verify clean `git status`, summarize `git log --oneline`, and **stop completely** to await explicit owner authorization for the next milestone.

---

## 12. Owner Decisions (`D1`–`D9`) and ADR Status (`ADR-C01`–`ADR-C18`)

### Proposed ADRs (`ADR-C01`–`ADR-C18`)

All 18 v0.2.0 Architecture Decision Records in [`ARCHITECTURE_DECISIONS.md`](ARCHITECTURE_DECISIONS.md) are in **Proposed** status pending formal owner approval:
- `ADR-C01` — Conversion is an Action capability, not a second action system (**Proposed**)
- `ADR-C02` — Crate boundaries: `radivine-dbus`, `radivine-convert`, `radivine-convert-image`, `radivine-jobs`, `radivine-worker` (**Proposed**)
- `ADR-C03` — Capability-based backends with data-only descriptors; deterministic routing (**Proposed**)
- `ADR-C04` — Built-in image backend: pure-Rust decode, `image` encoders, `libwebp` for lossy WebP only (**Proposed**)
- `ADR-C05` — `libwebp` linkage: vendored static (`D2`) (**Proposed**)
- `ADR-C06` — Out-of-process, on-demand worker launched in its own systemd cgroup (**Proposed**)
- `ADR-C07` — SQLite (WAL) job store, single writer (**Proposed**)
- `ADR-C08` — Job/item state machines; retry creates a new attempt; no auto-resume across sessions (**Proposed**)
- `ADR-C09` — Worker IPC: JSON lines, command + subscription connections, lossless paths, unified frame bounds (**Proposed**)
- `ADR-C10` — No-overwrite output commit protocol (two-rung atomic commit + private `.rvstage-*` staging + `item-<idx>-<rand16>.part` + `O_RDWR` validation) (**Proposed**)
- `ADR-C11` — Menu: `Convert ▸ Image ▸ target`; visibility by extension, progressive enablement by sniffed MIME; `description_for` hook (**Proposed**)
- `ADR-C12` — Backend-independent options and presets (`QualityIntent`, `SameFormatPolicy`, validated overrides) (**Proposed**)
- `ADR-C13` — Configuration: `convert.toml` (worker-authoritative) + `config.toml [jobs]`; daemon reads zero conversion config (**Proposed**)
- `ADR-C14` — Advanced Convert hosted by `radivine-control convert` (bypasses `ControlInstanceLock::acquire_or_notify_existing()`, transient cgroup scope) (**Proposed**)
- `ADR-C15` — Worker sends notifications via `radivine-dbus` (extracted from `radivine-actions/src/dbus.rs`, zero wheel/core deps) (**Proposed**)
- `ADR-C16` — Optional backend framework ships; no optional backend in v0.2.0 (`D3`) (**Proposed**)
- `ADR-C17` — Animated sources rejected; same-format governed by `SameFormatPolicy`; skips always reported (**Proposed**)
- `ADR-C18` — Metadata defaults: auto-orient, keep ICC, strip EXIF, drop XMP (**Proposed**)

### Recommended Owner Decisions Pending Approval (`D1`–`D9`)

| ID | Decision Topic | Recommended Choice | Status |
|----|----------------|--------------------|--------|
| **D1** | Milestone sequence | 5 milestones: **C1** (core + image backend) → **C2** (durable worker) → **C3** (wheel Quick Convert) → **C4** (Control Center UI + Advanced Convert) → **C5** (hardening & release) | Pending owner approval |
| **D2** | `libwebp` linkage | Vendored static build via `libwebp-sys` (≥ 1.3.2) | Pending owner approval |
| **D3** | Optional external backend in v0.2.0 | Ship discovery/routing framework only; defer `external.vips` to v0.2.x | Pending owner approval |
| **D4** | Radial menu depth | 3-level `Convert ▸ Image ▸ {PNG, JPEG, WebP, Advanced…}` | Pending owner approval |
| **D5** | Default EXIF policy | `ExifPolicy::Strip` (privacy-first; orientation still applied to pixels) | Pending owner approval |
| **D6** | Collision naming format | Space-parenthesized suffix: `photo (1).webp`, `photo (2).webp` | Pending owner approval |
| **D7** | Completion notification default | One notification per job (`notify = "always"`) | Pending owner approval |
| **D8** | GitHub Issue #1 (`eframe` clipboard) | Fix separately before Milestone C4 | Pending owner approval |
| **D9** | Queued jobs across logout / reboot | Transition to `Interrupted`; user triggers explicit Retry | Pending owner approval |
