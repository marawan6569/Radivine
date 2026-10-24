# Radivine v0.2.0 — Independent Architecture Review

**Status:** Review findings and recommendations; **not an implementation authorization**  
**Date:** 2026-10-24  
**Baseline:** v0.1.1, per the nine owner-provided planning documents  
**Scope:** Conversion architecture, backend strategy, job worker, filesystem safety, UX, ADRs, and milestone plan. Selected current repository source was cross-checked read-only through the connected GitHub repository.

## Executive verdict

The proposed architecture is directionally sound and substantially ready for an implementation *plan*, but should **not be approved as-is**. Preserve its main decisions: conversion as normal Radivine actions, a standalone codec-free Rust conversion core, a separate image backend, a general-purpose durable jobs contract/worker, SQLite persistence, five milestones (C1–C5), and unchanged wheel appearance. Address the blockers below before implementing source code.

**Review boundary:** This is a document/code-reading review, not a proof that the proposed APIs compile, filesystems behave as expected, or security claims pass fault-injection tests. All proposed ADRs remain Proposed until approved by the owner.

## Blocking findings

### B1 — Non-atomic final-file fallback contradicts crash-safe output UX
**References:** `SECURITY_AND_FILE_SAFETY.md` §§2 T10, 3.3 (rung C); `ARCHITECTURE_DECISIONS.md` C10; `IMPLEMENTATION_PLAN(1).md` R-V3.

The third commit fallback creates the *final pathname* with `O_EXCL` and copies into it incrementally. It prevents overwrites, but exposes a partial result if the process crashes or power is lost. This directly weakens the promised "no corrupt final output"/atomic visibility guarantee, and creates an additional recovery state (`Writing`) absent from the proposed `CommitJournal` API and worker schema. **Recommendation:** v0.2.0 should use `RENAME_NOREPLACE` or atomic hard-link publication, and return `FilesystemUnsupported` on filesystems supporting neither. Document USB/exFAT limitations instead of enabling partial-visible outputs. Later support needs an explicit owner-approved trade-off.

### B2 — Temp-file recovery may delete a file not owned by Radivine
**References:** `SECURITY_AND_FILE_SAFETY.md` §3.1 steps 4–5 and T9; `JOB_SYSTEM_DESIGN.md` §5 item 1.

The proposed sequence journals `TempCreated{dir,name}` **before** creating the temp file. Recovery removes an existing file if its name matches the expected pattern and it is regular; no recorded inode is required. On a shared directory, an unrelated/replaced file can acquire that name. **Recommendation:** create with `O_EXCL`, capture `fstat` identity, then journal `(dir,name,dev,ino)`; never unlink on recovery without identity validation. Accept harmless orphan temp files if a crash falls between creation and durable journaling; provide a conservative manual cleanup tool. Explicitly define directory-handle and symlink race tests.

### B3 — Hanging encoder threads are not actually bounded by the watchdog
**References:** `JOB_SYSTEM_DESIGN.md` §7.3 Time; `SECURITY_AND_FILE_SAFETY.md` T2; `CONVERSION_ARCHITECTURE.md` §5.6.

Rust cannot forcibly kill a stuck thread. "Mark timed out, enter drain mode, wait for other threads, then self-restart" may wait forever if the timed-out thread itself is stuck. **Recommendation:** distinguish cooperative cancellation latency from a true hard timeout. Choose an enforceable process-level safety policy (e.g., kill/restart the worker with explicit interrupted-job recovery, or isolate conversion attempts in supervised child processes). Define how other running items are affected and test a deliberately non-terminating handler.

### B4 — Detached fallback is not independent if launched within the daemon's systemd cgroup
**References:** `JOB_SYSTEM_DESIGN.md` §§2.2–2.3; `ARCHITECTURE_DECISIONS.md` C06.

The documents correctly explain that `setsid()`/`process_group(0)` do **not** escape a systemd service cgroup. Yet the fallback launcher spawns a detached child from the daemon when its dedicated service unit cannot be used. If the daemon itself is a systemd service, that child can still die when the daemon restarts. **Recommendation:** when a user systemd manager is present, require an independently supervised unit/scope (or fail clearly); use detached fallback only when the process is demonstrably outside the daemon service cgroup. Include negative tests where the dedicated worker unit is absent, not just normal systemd success tests. Check the analogous Advanced Convert window lifecycle.

### B5 — Output validation as specified cannot read the temp fd
**Reference:** `SECURITY_AND_FILE_SAFETY.md` §3.1 steps 5 and 8.

The temporary output file is opened `O_WRONLY`, then the design says "re-read output header via the tmp fd". A write-only fd cannot be read. **Recommendation:** specify a safe `O_RDWR` strategy, or reopen relative to the pinned destination directory fd with inode verification and reject symlink changes. Flush buffered encoders, rewind, validate, and only then commit. Add a realistic end-to-end test for this path.

### B6 — Persisted success, fsync order, and recovery need one exact protocol
**References:** `SECURITY_AND_FILE_SAFETY.md` §§3.1, 3.5; `JOB_SYSTEM_DESIGN.md` §§4.2, 5, 8.

The proposal batches `fsync(dir)` per job, records individual `Succeeded` items earlier, and only reviews non-terminal journaled items during recovery. A power failure before the directory-entry sync may leave a persisted "Succeeded" item with a missing output, depending on transaction ordering. The `Committing` journal is described as FULL-durable while general SQLite WAL transactions are NORMAL, but the atomic ordering across those two durability domains is not fully specified. **Recommendation:** write a precise crash-state table for `(temp fsync, journal durable commit, atomic publish, dir fsync, result commit)` and test every kill point. Never report or persist an unrecoverable success before its output directory entry is durable; alternatively verify all uncertain `Succeeded` entries at restart and downgrade missing outputs. Prefer a clearly correct initial policy even if that means per-item directory fsync until benchmarks justify batching. SQLite's own docs distinguish WAL NORMAL consistency from FULL power-loss durability.

## Important design clarifications (nonblocking once resolved in specs)

### R1 — UI-process header parsing contradicts the isolation statement
`SECURITY_AND_FILE_SAFETY.md` §4 says untrusted parsing happens **only in the worker**, but `CONVERSION_UX.md` §5 specifies running `ConversionEngine::inspect` (header parsing) inside `radivine-control` on a background thread. A UI background thread still belongs to the UI process. Choose one policy: either inspect headers via worker IPC (recommended, keeping one trust boundary), or explicitly weaken the isolation claim, justify pure-Rust parsers, and test malformed headers in the Control Center process. Don't call it full worker isolation while doing the latter.

### R2 — Job-state/schema mismatch: a committing state is not persisted explicitly
`JOB_SYSTEM_DESIGN.md` §3.3 treats `Committing` as an item stage; the schema has `items.state`, but no `items.stage`, and the recovery algorithm depends on journal fields. This can work *only if* the record structure and atomic update steps are defined precisely. If B1's exclusive-copy fallback is rejected, remove all `Writing` references as well. Add a state/journal matrix and schema migration contract.

### R3 — Some performance acceptance criteria are over-specific or not guaranteed
- WebP lossy output size is not mathematically guaranteed to be strictly monotonic for every image as quality rises. Benchmark representative samples, but avoid treating a single reversal as an automatic correctness failure.
- A rigid `max(200 MiB, 2 × input_size)` free-disk **minimum** may reject a perfectly valid tiny image conversion on a disk with 150 MiB free. Use a sensible estimate/margin and always handle actual `ENOSPC` safely.
- Thread cancellation within 1 s for a 12 MP image is only a target if codec stages can meet it; the initial design admits no mid-stage interruption.
- The 10,000-item, 0.5 ms menu computation, 750 ms worker cold start, and 1.5 s 12 MP WebP encode are **targets** until measured on a named machine.

### R4 — Indirect wheel dependency in `radivine-worker`
The proposed `radivine-worker → radivine-actions` edge for one D-Bus notification helper can pull the action/core/wheel crate graph into a generic worker. Not a functional blocker, but consider extracting the small notifier into a lightweight shared module/crate or implementing a platform-specific worker notifier behind a narrow interface. Do not create extra crates without a measurable benefit.

### R5 — Portable path contract and source/destination races need explicit acceptance tests
The protocol intentionally preserves non-UTF-8 Unix paths; ensure actual end-to-end tests cover literal `%`, control bytes, and invalid UTF-8 filenames, and that diagnostics never normalize them into a different file path. Verify the destination directory fd is pinned and that no recovery cleanup uses paths alone without checking identities.

### R6 — Documentation and decision tracking need consistency
The uploaded `README.md` inventories `IMPLEMENTATION_HANDOFF.md`, but that file was not among the nine uploads; check whether it exists in the local Workspace before declaring it absent. Some text refers to the queued-after-reboot decision as D10; `ARCHITECTURE_DECISIONS.md` lists D9. Correct this and explicitly label Proposed versus Accepted throughout. `IMPLEMENTATION_PLAN(1).md` is the **new** conversion plan, not the preexisting repository `IMPLEMENTATION_PLAN.md`.

### R7 — Static vendoring and metadata are C1 verification gates, not assumed guarantees
Confirm safe lossy-WebP wrapper, current libwebp source version/security maintenance, RIFF metadata muxing, ICC round-trip behavior, animated image rejection, and actual decoder memory bounds before committing to a shipped backend. The choice to bundle vendored libwebp should stay conditional until these spikes pass.

## Recommended owner decisions (provisional)

| ID | Recommendation | Status |
|----|----------------|--------|
| D1 | Adopt five milestones: C1 conversion libs, C2 worker, C3 wheel, C4 Control Center UI, C5 release hardening | Recommend approval |
| D2 | Vendored static libwebp for lossy encode, contingent on C1 verification and proper notices | Conditional approval |
| D3 | Framework for optional backends, no optional backend shipped in 0.2.0 | Recommend approval |
| D4 | Keep `Convert → Image → target` to preserve future extensibility | Recommend approval |
| D5 | Strip EXIF by default, preserve ICC only where correctly supported, with explicit warnings | Recommend approval |
| D6 | Number collisions as `photo (1).webp`, with kernel-level no-replace | Recommend approval |
| D7 | One terminal-job notification, not per-file spam; clarify `always` versus success-only | Recommend approval |
| D8 | Keep GitHub issue #1 separate; fix before C4 if possible | Recommend approval |
| D9 | Do not auto-restart queued work after logout/reboot; show Interrupted with manual Retry | Recommend approval |

## Recommended corrected implementation gates

**Architecture signoff:** B1–B6 resolved *on paper* with canonical diagrams, journal schema, and failure matrix; R1 isolation policy selected; corrected ADR-C06/C07/C10 and cross-file references; complete `IMPLEMENTATION_HANDOFF.md` located/created. No production code, tests, or Git history changes are authorized by this review.

**C1 (owner authorization required):** Run source-backed backend spikes first. Implement reusable conversion core + image backend + atomic safe-output protocol. Test write-only/readback, collision races, non-UTF-8 paths, symlinks, formats/ICC/EXIF/animation, hard limits, and crash points. Stop if WebP backend viability or output safety is disproven.

**C2 (separate authorization):** Implement durable worker, journal/store and test crash recovery, reliable process independence, hard timeout policy, cancellation, and exact power-loss assumptions. Lifecycle tests must use isolated environment; no active-session pointer automation.

**C3–C5:** Existing proposed sequence is reasonable, pending the corrected C1–C2 contracts. No automatic release/tag/push. Existing wheel layout, branding assets, and native file Copy must remain unchanged.

## Independent-review checklist for Gemini

1. Inspect current source and nine planning documents without editing code.
2. Independently validate/refute findings B1–B6 and R1–R7 with concrete evidence; do not merely echo them.
3. Search for new blockers not listed here (especially path preservation, IPC, application lifecycle, SQLite journaling, format metadata, and failure tests).
4. Propose minimal edits to the planning documents and ADRs only; keep them Proposed.
5. Check that code sketches are internally consistent and align with the real `Action`/`MenuSpec` and Control Center APIs.
6. Produce a corrected implementation handoff and an architecture-review summary. Do not begin C1 or publish/push anything.

## Source inventory

Owner-uploaded: `ARCHITECTURE_DECISIONS.md`, `CONVERSION_ARCHITECTURE.md`, `CONVERSION_BACKENDS.md`, `CONVERSION_PRODUCT_SPEC.md`, `CONVERSION_UX.md`, `IMPLEMENTATION_PLAN(1).md`, `JOB_SYSTEM_DESIGN.md`, `README.md`, `SECURITY_AND_FILE_SAFETY.md`. Selected source in `https://github.com/marawan6569/Radivine` was inspected read-only through the linked GitHub account (`AGENTS.md`, Action API/registry/actions, Control Center code). No compilation or fault-injection tests were run in this review.
