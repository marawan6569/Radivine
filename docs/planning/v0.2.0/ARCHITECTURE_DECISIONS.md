# Radivine v0.2.0 — Proposed Architecture Decisions

Status: **All ADRs below are PROPOSED (pending owner review). None is Accepted.** · Prepared: 2026-10-24
These ADRs are intentionally kept **outside** the canonical `docs/DECISIONS.md` until the owner approves them. After approval, they are renumbered into `DECISIONS.md` (next free number ADR-027) without editing existing Accepted ADRs. Superseded items (e.g. ADR-018 "no settings GUI", the ACTION_SYSTEM §1 non-goal "no long-running job UI") are marked as such at that time.

## Index

| ADR | Title | Status |
|-----|-------|--------|
| C01 | Conversion is an Action capability, not a second action system | Proposed |
| C02 | Crate boundaries: `radivine-dbus`, `radivine-convert`, `radivine-convert-image`, `radivine-jobs`, `radivine-worker` | Proposed |
| C03 | Capability-based backends with data-only descriptors; deterministic routing | Proposed |
| C04 | Built-in image backend: pure-Rust decode, `image` encoders, libwebp for lossy WebP only | Proposed |
| C05 | libwebp linkage: vendored static (owner decision D2) | Proposed |
| C06 | Out-of-process, on-demand worker launched in its own systemd cgroup | Proposed |
| C07 | SQLite (WAL) job store, single writer | Proposed |
| C08 | Job/item state machines; retry creates a new attempt; no auto-resume across sessions | Proposed |
| C09 | Worker IPC: JSON lines, command + subscription connections, lossless paths | Proposed |
| C10 | No-overwrite output commit protocol | Proposed |
| C11 | Menu: `Convert ▸ Image ▸ target`; visibility by extension, enablement by sniffed MIME; `description_for` hook | Proposed |
| C12 | Backend-independent options and presets (intents + validated per-format overrides) | Proposed |
| C13 | Configuration: `convert.toml` + `config.toml [jobs]`; daemon untouched | Proposed |
| C14 | Advanced Convert hosted by `radivine-control convert` | Proposed |
| C15 | Worker sends notifications via the existing pure-Rust D-Bus client | Proposed |
| C16 | Optional backend framework ships; no optional backend in v0.2.0 (owner decision D3) | Proposed |
| C17 | Animated sources rejected; same-format hidden in Quick; skips always reported | Proposed |
| C18 | Metadata defaults: auto-orient, keep ICC, strip EXIF, drop XMP | Proposed |

---

## ADR-C01 — Conversion is an Action capability
**Context:** The brief requires conversion to integrate with the existing Action System (4.1) and forbids a competing interaction system.
**Decision:** Quick targets and Advanced are ordinary `impl Action` types in `radivine-actions::convert`, placed in `default_menu()` via `MenuSpec`. `run()` submits a durable job (or spawns the Advanced window) and returns.
**Alternatives:** (a) a dedicated "conversion mode" wheel: competing system, rejected. (b) Converting inside `Action::run` on the Dispatcher: blocks Dispatcher slots for minutes, dies with the daemon, inflates daemon memory. Rejected.
**Consequences:** All existing machinery is reused (overrides, ordering, "More…", enablement re-check, error notifications). New capability: actions that delegate to durable jobs.

## ADR-C02 — Crate boundaries
**Decision:**
- `radivine-dbus` (pure-Rust D-Bus notification and `FileManager1` client, extracted from `crates/radivine-actions/src/dbus.rs:1-492`, zero Radivine dependencies, re-exported by `radivine-actions::dbus`);
- `radivine-convert` (core, codec-free, no Radivine deps);
- `radivine-convert-image` (built-in backend);
- `radivine-jobs` (job contract, protocol, client, launcher);
- `apps/radivine-worker` (store, scheduler, handlers, server; depends on `radivine-jobs`, `radivine-convert`, `radivine-convert-image`, and `radivine-dbus` — zero dependency on `radivine-actions`, `radivine-core`, or `radivine-wheel`).

Graph in CONVERSION_ARCHITECTURE §4.2.
**Alternatives:**

| Alternative | Pros | Cons |
|-------------|------|------|
| Monolithic: conversion code inside `radivine-actions` | Fewer crates | Codecs in the daemon's dependency tree, not reusable, no clean backend seam |
| Single `radivine-convert` with codecs behind a feature | One crate | Cargo feature unification in workspace builds leaks codecs into every dependent, and future backends bloat the core |
| Worker as a subcommand of the `radivine` binary | No new binary | Daemon binary links SQLite/libwebp; the "decoder-free daemon" story is muddled; same cgroup trap applies |
| `radivine-worker` depending on `radivine-actions` for D-Bus | One fewer crate | Pulls `radivine-actions → radivine-core → radivine-wheel` into `radivine-worker`, violating the worker's independence from the radial wheel and action system |
| **Proposed split** | Reusable core, explicit ownership, small daemon delta, clean worker dependency graph | Five new crates/apps to maintain |

**Consequences:** The daemon depends on `radivine-convert-image` only for `descriptor()`. [VERIFY] binary delta ≤ +300 KiB (fallback: move the descriptor constant into `radivine-convert::builtin`). `radivine-worker` has zero dependency on `radivine-actions`, `radivine-core`, or `radivine-wheel` (verified via `cargo tree -p radivine-worker`). Add the CI dependency-graph check (discrepancy X7).

## ADR-C03 — Capability-based backends
**Decision:** Backends publish a data-only `BackendDescriptor` (capabilities per `(from,to)`, feature flags, `max_pixels`, kind `BuiltIn | ExternalProgram(ProbeSpec)`). Discovery produces `Availability`. `CapabilityCatalog::route` selects by availability → feature match → user preference → priority → id. Implementations (`ConversionBackend`) are separate and only instantiated by the worker.
**Alternatives:** One universal backend (simple, but there is no seam for optional/external backends or per-size routing). Cost-based selection (speculative with a single backend; deferred). Explicit-config-only selection (users should not have to configure anything).
**Consequences:** Three-way availability (available / requires optional / unsupported) is first-class. Routing is deterministic and unit-testable.

## ADR-C04 — Built-in image backend
**Decision:**
- Decode PNG/JPEG/WebP with the pure-Rust `image` crate, with explicit `Limits`.
- Encode PNG/JPEG with `image`.
- WebP lossless with `image-webp`.
- **WebP lossy with libwebp** behind a minimal safe wrapper.
- Metadata via `image` APIs, plus RIFF muxing for lossy WebP.
- Resize with `image::imageops` (SIMD crate deferred).

**Alternatives:**

| Alternative | Why not |
|-------------|---------|
| `image` only | **Cannot produce lossy WebP** (verified: `WebPEncoder::new_lossless` is the only constructor) |
| libvips (bindings) as the essential backend | Heavy closure (~30 libs), large C parsing surface for untrusted files, binding/API-version mismatch on noble, LGPL obligations if static |
| ImageMagick | Security history, policy dependence |
| Pure-Rust lossy WebP (`zenwebp`) | AGPL, licence-incompatible |

**Consequences:** All untrusted parsing stays in memory-safe Rust. C code (libwebp) only processes Radivine-generated pixels. A C compiler is needed at build time.

## ADR-C05 — libwebp linkage
**Decision (pending D2):** vendored static build via `libwebp-sys`, pinned libwebp ≥ 1.3.2 (prefer latest), tracked with `cargo audit`/`cargo deny` and a release checklist item.
**Alternative:** system `libwebp7` (`Depends: libwebp7 (>= 1.3.2)`). Gains distro security updates, but makes builds divergent, needs pkg-config, and system-link support in libwebp-sys is unverified.
**Consequences:** Self-contained `.deb` with no new `Depends`. The BSD-3 notice must be added to `THIRD_PARTY_NOTICES.md` and `debian/copyright`.

## ADR-C06 — On-demand worker in its own cgroup
**Decision:** Single-instance `radivine-worker` (`flock(worker.lock, LOCK_EX)` + socket). Launched via `systemctl --user start --no-block radivine-worker.service` (a static unit with `After=dbus.socket`, no `[Install]`) when the user manager is reachable, falling back to a transient user unit (`systemd-run --user --collect --unit=radivine-worker-transient -- radivine-worker serve`) if `systemd --user` is active but the unit file is not installed (e.g., dev `cargo run`), and falling back to `setsid` / `process_group(0)` only when no `systemd --user` manager is running. Before idle exit (60 s), the worker unlinks `worker.sock`, performs a final non-blocking `accept()` drain while holding `worker.lock`, and aborts idle exit if a client connected during the race window. Graceful SIGTERM handling (`PRAGMA wal_checkpoint(TRUNCATE)` before exit). Per-item watchdog timeouts are coordinated via O(1) lock-free-I/O `Mutex<ItemPhase>` (`PreCommit` / `JournalingCommit` / `AbortedBeforeCommit` / `InCommit { timeout_observed }` / `Committed`) and `CommitPermit` (`Mutex<ItemPhase>` is never held during SQLite or filesystem I/O, and `InCommit.timeout_observed = true` prevents any new commit attempt after `EEXIST`), with a bounded ≤ 5 s drain before `exit(75)` while holding `worker.lock` (`O_CLOEXEC`) so replacement workers never overlap (`JOB_SYSTEM_DESIGN` §7.5).
**Alternatives:**
- In-daemon: violates lifetime independence.
- Always-on service: idle process plus a second autostart path.
- Detached child of the daemon without `systemd-run --user` when `systemd --user` is active: **killed by `systemctl --user restart radivine.service`** because `KillMode=control-group` kills the whole cgroup.
- Socket-activated unit: needs enabling a `.socket` per user; more packaging complexity for little gain.
- Process-per-job: unnecessary while decoders are memory-safe.

**Consequences:** A second unit file ships in the `.deb`. Spike P10 must confirm cgroup independence for both the installed static unit and the `systemd-run --user` transient fallback. Resource controls (`Nice`, IO class, optional `MemoryMax`) are available through the unit.

## ADR-C07 — SQLite job store
**Decision:** `rusqlite` with `bundled`, WAL, default `synchronous=NORMAL` for high-throughput status updates and `PRAGMA synchronous=FULL` specifically around the synchronous `Committing` journal transaction (before `renameat2`/`linkat`), `PRAGMA wal_checkpoint(TRUNCATE)` on startup after recovery and before clean exit, single writer (worker serialized by `worker.lock`), schema versioning, retention pruning, at `$XDG_STATE_HOME/radivine/jobs.sqlite3`. The `items` table uses standard integer primary key row storage (not `WITHOUT ROWID`) and indexes `jobs(retry_of)`.
**Alternatives:** Atomic JSON files (hand-rolled indexing and compaction, O(n) rewrites, fsync storms). redb (no queries/tooling). System libsqlite3 (smaller, but version drift across platforms).
**Consequences:** About 1 MB in the worker binary only. Concurrent readers are possible but not used (clients use IPC).

## ADR-C08 — Job lifecycle semantics
**Decision:**
- States: Queued, Preparing, Running, Cancelling, Completed, Failed, Cancelled, Interrupted.
- Item states: Pending, Running, Succeeded, Failed, Skipped, Cancelled, Interrupted.
- **Retry creates a new job** linked by `retry_of`, reusing the settings snapshot.
- Restart, never resume, mid-file.
- Queued jobs survive only a worker *crash* within the same boot. After graceful shutdown (logout) or reboot, they become Interrupted.
- `request_id` gives idempotent submit/retry.

**Alternatives:** Retry as a transition of the same job (mutable history, complex recovery). Auto-resume queued jobs after login (surprising background CPU and disk activity after reboot; it would need the daemon to nudge the worker). Partial-file resume (no backend supports it).
**Consequences:** Immutable, auditable history. Simple recovery rules. Users retry explicitly after a session ends.

## ADR-C09 — Worker IPC
**Decision:** JSON lines over a 0600 Unix socket. One-shot command connections plus a long-lived subscription connection with coalesced (≤10 Hz) progress, heartbeats, and `seq`/`worker_instance` re-sync. `SO_PEERCRED` uid check. Protocol version field. `WirePath` lossless path encoding. Unified frame and batch bounds (`JOB_SYSTEM_DESIGN` §6): `16 MiB` max `submit` request frame, `64 KiB` max non-`submit` request frame, `4 MiB` max command response frame (`get` paged at ≤ 200 items), `64 KiB` max subscription event frame, `1..=10,000` paths per job (each `≤ 4,096` raw OS bytes), and `{"status":"error","code":"frame_too_large"}` rejection before full allocation.
**Alternatives:** Synchronous polling only (latency and wakeups; acceptable for the CLI, which uses it). D-Bus (heavier, not portable). Binary protocols (unnecessary).
**Consequences:** Consistent with ADR-017. The Control Center gets push updates without blocking.

## ADR-C10 — No-overwrite output commit
**Decision:**
- Source files are opened `O_RDONLY | O_NONBLOCK | O_CLOEXEC` (following user-selected symlinks to regular files, matching `radivine_core::enrich::inspect_regular_file` at `enrich.rs:483-538`), verified via `fstat(src_fd)` as `S_ISREG` (rejecting FIFOs/sockets/devices/directories), and re-checked (`dev, ino, size, mtime_ns`) after decode.
- All destination filesystem operations are relative to opened directory fds (`dest_dir_fd` and private per-job staging directory `.rvstage-<job>-<rand16>`, mode `0700`, `O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`).
- Two-phase staging-directory journaling: `StageIntent` (`dest_dir`, `stage_dir_name`) is durably recorded in SQLite **before** `mkdirat(dest_dir_fd, stage_dir_name, 0o700)` (`C0b`), and `StageCreated` (`stage_dev`, `stage_ino`) is durably recorded **after** `mkdirat` + `fsync(dest_dir_fd)` + `fstat(stage_dir_fd)` and **before** creating any temp file (`C0c`).
- Canonical temp file naming: `item-<idx>-<rand16>.part` (`^item-[0-9]+-[0-9a-f]{16}\.part$`, mode `0600`) inside `.rvstage-<job>-<rand16>` (`^\.rvstage-[0-9]+-[0-9a-f]{16}$`, mode `0700`), opened `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC`, followed by `fstat(temp_fd)` and durable `TempCreated` journaling (`temp_name`, `temp_dev`, `temp_ino`) before writing bytes.
- `fsync(temp_fd)`, `O_RDWR` rewind + header validation, then lock-free-I/O commit-fenced `ItemControl::begin_commit(CommitRequest::new(attempt, candidate, temp_dev, temp_ino))` (`PreCommit`/`InCommit(timeout_observed: false)` → `JournalingCommit` → unlocked `PRAGMA synchronous=FULL` `Committing` update → `InCommit` + `req.authorize() -> CommitPermit`).
- Two-rung atomic commit ladder only: Rung A (`renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)` → `fsync(stage_dir_fd)` → `fsync(dest_dir_fd)`) → Rung B (`linkat(stage_dir_fd, temp_name, dest_dir_fd, candidate, 0)` → `fsync(dest_dir_fd)` → `unlinkat(stage_dir_fd, temp_name, 0)` → `fsync(stage_dir_fd)`). No non-atomic copy fallback and no check-then-replace fallback; filesystems supporting neither Rung A nor Rung B fail cleanly with `ConversionError::FilesystemUnsupported`.
- Numbered collision loop (`stem (1).ext` .. `stem (9999).ext`), re-journaling `Committing` per candidate before attempting `commit_no_replace` and aborting immediately if a watchdog timeout was observed (`InCommit.timeout_observed == true`) during the previous attempt.
- Both `fsync(stage_dir_fd)` and `fsync(dest_dir_fd)` are completed **before** recording `Succeeded` or clearing `temp_name` in SQLite, and `fsync(dest_dir_fd)` is completed after `unlinkat(dest_dir_fd, stage_dir_name, AT_REMOVEDIR)` **before** clearing `stage_dir_name` in SQLite.
- Safe temp cleanup (`cleanup_temp_safely`) requires `worker.lock`, regex-validated single-component names, verified `(stage_dev, stage_ino)` + `0700`/`uid` namespace exclusivity, and verified `(temp_dev, temp_ino)` before `unlinkat`, followed by `fsync(stage_dir_fd)`; unjournaled (`C0b`/`C1`) or unverified orphans are preserved untouched (`OrphanedTempPreserved`) and never deleted by filename pattern.
- `std::fs::rename` banned in conversion crates.

**Alternatives:** Plain `rename` (replaces). Check-then-rename (TOCTOU). Rung C `O_EXCL` copy fallback (exposes 0-byte/partial final files before crash/validation). Writing directly to the final name (partial files visible). `O_TMPFILE` as the baseline (filesystem support gaps; kept as an optional enhancement).
**Consequences:** Overwrites and partial final files are impossible by construction. `INV-SRC` and `INV-NOOVERWRITE` are testable invariants.

## ADR-C11 — Menu integration
**Decision:**
- `Convert ▸ Image ▸ {PNG, JPEG, WebP, Advanced…}` at root index 1.
- Visibility from extensions plus a static capability-catalog snapshot.
- Progressive enablement from `FileFacts.sniffed_mime`: enables immediately as soon as `confirmed > 0` (first enriched item confirming ≥ 1 convertible image; `enrich.rs:205-242` emits an `EnrichEvent::Updated` snapshot after each inspected item up to `MAX_ENRICHED_ITEMS = 64`), before checking whether remaining items are still `Pending`.
- Items beyond the 64-item enrichment cap are trusted to the worker.
- New default trait method `Action::description_for(&BasicContext)` for count-aware captions (including `"Convert 2 of 3 images to WebP · 1 already WebP"` and `"Convert 3 of 5 files to WebP · 2 not supported"`).
- Lossless `OsStr` path handling (`E8`) in `radivine_core::local_path_to_file_uri`, `normalize_cli_arg_to_uri`, and `Commands::Invoke { paths: Vec<OsString>, .. }`.

**Alternatives:** Flat `Convert ▸ target` (one fewer hover level, but paths change when other categories arrive). Owner decision D4. Content-sniff visibility (would require I/O before the first frame, violating ADR-012).
**Consequences:** The text-file wheel layout is unchanged from v0.1.1. Image wheels gain one root item.

## ADR-C12 — Options and presets
**Decision:** Presets are expressed as `QualityIntent` + `ResizeSpec` + optional per-format overrides, and `ConversionOptions` includes explicit `SameFormatPolicy` (`Skip` for Quick Convert, `Convert` for Advanced Convert). `ConversionOptions::validate()` uses only `FormatTraits`. Backend support is a separate routing step. The UI never shows a cross-format quality number.
**Alternatives:** Backend-specific option schemas exposed directly (couples presets to backends, so presets break when backends change). Fixed presets only (too rigid for Advanced).
**Consequences:** Presets survive backend changes. The intent→number table is calibrated in C1.

## ADR-C13 — Configuration ownership
**Decision:**
- New `convert.toml` (schema owned by `radivine-convert`, resolved authoritatively at runtime **only** by `radivine-worker` during `Preparing` and snapshotted into `jobs.resolved`, edited by the CC and Advanced Convert window).
- New `config.toml [jobs]` (schema owned by `radivine-jobs`; worker reads `[jobs]` and `[notifications].enabled`).
- **The wheel daemon (`radivine`) reads zero conversion configuration (`convert.toml` is never read by the daemon).**
- Invalid config never blocks startup.
- `toml_edit` atomic writes with conflict detection (existing pattern).

**Alternatives:** Everything in `config.toml` (couples daemon schema validation to conversion; harder downgrade story). Daemon reading `convert.toml` for caption preset labels (couples daemon reload to conversion config edits). Separate `worker.toml` (file sprawl).
**Consequences:** v0.1.1 configs unchanged and compatible both ways; Control Center saves to `convert.toml` take effect on the very next Quick Convert without a daemon reload, while already-submitted jobs use their immutable `jobs.resolved` snapshot.

## ADR-C14 — Advanced Convert host
**Decision:** `radivine-control convert` mode (separate process from the main CC window that explicitly **bypasses `ControlInstanceLock::acquire_or_notify_existing()`** in `apps/radivine-control/src/single_instance.rs:39-42` so multiple Advanced Convert windows can coexist with the Settings/Jobs window; same binary, theme, and widgets). Spawned in a transient user cgroup scope (`systemd-run --user --scope --collect`) when `systemd --user` is active so daemon restarts do not kill open dialogs. The item list is passed over stdin (`WirePath` JSON lines, bounded to `≤ 10,000` lines, `≤ 16 KiB` per line, and `≤ 16 MiB` total stdin bytes). Clicking "Open Jobs" / "Show progress" spawns `radivine-control --page jobs`.
**Alternatives:** A new binary (another ~10–20 MB eframe app [VERIFY size]). A page inside the main CC window (heavy for a transient dialog; single-instance focus semantics conflict with multiple concurrent dialogs).
**Consequences:** Small refactor of `ui.rs` (extract palette/widgets into a module). Text fields depend on issue #1's resolution.

## ADR-C15 — Notifications from the worker
**Decision:** Extract `crates/radivine-actions/src/dbus.rs:1-492` into a standalone workspace crate `crates/radivine-dbus` (`#![forbid(unsafe_code)]`, depending only on `thiserror`) and re-export `pub use radivine_dbus as dbus;` in `radivine-actions`. The worker sends one notification per terminal job using `radivine_dbus::call_desktop_notify` on a notifier thread, honouring `[notifications] enabled` and `[jobs] notify`.
**Alternatives:** (a) The daemon sends notifications (it is not necessarily running and does not know about completions). (b) The Control Center sends them (often closed). (c) Having `radivine-worker` depend on `radivine-actions` (rejected: pulls `radivine-actions → radivine-core → radivine-wheel` into the worker's dependency tree).
**Consequences:** `radivine-worker → radivine-dbus` has zero dependency on `radivine-actions`, `radivine-core`, or `radivine-wheel`, while `radivine-actions::dbus` remains 100% backwards-compatible for existing callers.

## ADR-C16 — Optional backends
**Decision (pending D3):** Ship the discovery framework and UI states in v0.2.0, but no optional backend. First candidate: `external.vips` (CLI, process-isolated) in v0.2.x.
**Alternative:** Implement `external.vips` in C5 as a stretch goal.
**Consequences:** "Requires optional dependency" states are exercised by tests (fake descriptor) rather than by real users in v0.2.0.

## ADR-C17 — Scope guards
**Decision:**
- Animated APNG/WebP sources are rejected with a clear reason.
- Same-format handling is governed by `SameFormatPolicy`: hidden in Quick (`SameFormatPolicy::Skip`, skipping items already in the target format with `AlreadyTargetFormat`) and allowed in Advanced (`SameFormatPolicy::Convert`, with a warning when no transform is set).
- Items already in the target format during Quick Convert are skipped and reported.
- Unsupported items are skipped and reported. Never silent.

**Consequences:** No surprising frame loss or duplicate files.

## ADR-C18 — Metadata defaults
**Decision:** Auto-orient (pixels rotated, tag normalized). ICC preserved byte-exact when the backend reports `ICC_EMBED`, otherwise an explicit warning. EXIF stripped by default (preserve optional, orientation normalized). XMP/IPTC dropped. New mtime. Mode = source & 0o666.
**Alternatives:** Preserve EXIF by default (leaks GPS when sharing web images).
**Consequences:** Privacy-first. Owner decision D5 confirms.

---

## Trade-off summary for the brief's §16 comparisons

| # | Comparison | Choice | Main reason |
|---|-----------|--------|-------------|
| 1 | Monolithic vs reusable library | Reusable `radivine-convert` (+ backend crates) | Desktop-independence, testability, future backends |
| 2 | One universal backend vs capability-based | Capability-based (C03) | Optional/external backends, honest availability, size routing |
| 3 | Pure Rust vs native bindings | Pure-Rust decode + native libwebp **encode only** (C04) | Memory-safe parsing of untrusted input. Lossy WebP is impossible in pure Rust under a compatible licence. |
| 4 | In-daemon vs out-of-process | Out-of-process worker (C06) | Lifetime independence, daemon RSS, crash isolation |
| 5 | Long-lived service vs on-demand | On-demand, own cgroup (C06) | Zero idle cost, survives daemon restart |
| 6 | SQLite vs files | SQLite WAL (C07) | Transactions, queries, crash safety, pruning |
| 7 | Synchronous IPC vs event subscriptions | Both: commands sync, progress via subscription (C09) | Responsive UI without polling |
| 8 | Fixed presets vs backend-specific options | Intent presets + validated per-format overrides (C12) | Backend independence with Advanced flexibility |
| 9 | Full batch prevalidation vs per-item | Hybrid: cheap prevalidation in Preparing + authoritative per-item validation (JOB_SYSTEM_DESIGN §7.1) | Early feedback without doubling I/O. TOCTOU-safe. |

## Decisions that genuinely require the owner

| ID | Decision | Recommendation | Impact if changed |
|----|----------|----------------|-------------------|
| **D1** | Milestone re-sequencing: C1 core → **C2 worker** → C3 wheel integration → C4 Control Center UI → C5 hardening/release (5 milestones instead of 4) | Approve | The original C2 (wheel integration + batch) would otherwise need a throwaway in-daemon execution path |
| **D2** | libwebp linkage | Vendored static | System lib: change `build-deb.sh` Depends generation, add `libwebp-dev` to CI |
| **D3** | Optional backend in v0.2.0 | Framework only | `external.vips` adds roughly half a milestone of work + security review |
| **D4** | Menu depth | `Convert ▸ Image ▸ target` | Flat menu: one hover level fewer; paths change later when other categories arrive |
| **D5** | EXIF default | Strip | Preserve: leaks GPS by default |
| **D6** | Collision naming | `photo (1).webp` | `photo-1.webp` is a trivial change |
| **D7** | Notification default | One per job, always | "Failures only" is quieter |
| **D8** | Issue #1 timing | Fix separately before C4 | If deferred, the new text fields ship with the same clipboard limitation |
| **D9** | Queued jobs after logout/reboot | Interrupted, manual Retry | Auto-resume needs a daemon-start nudge and may surprise users |

## Open questions requiring experimental verification

P1–P10 in CONVERSION_BACKENDS §10, plus:
- Q-A: daemon binary size delta from depending on `radivine-convert-image` for descriptors only (C3).
- Q-B: systemd user-manager hardening options that start reliably on Mint 22 (`MemoryMax`, `SystemCallFilter`, `RestrictAddressFamilies`) (C5).
- Q-C: `radivine-control` cold start time for the Advanced window on the reference machine (C4).
- Q-D: libwebp progress hook availability through the chosen wrapper (mid-encode cancellation) (C1).
