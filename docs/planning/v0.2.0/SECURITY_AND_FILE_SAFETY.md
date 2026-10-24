# Radivine v0.2.0 — Security and Filesystem Safety

Status: **Proposed — pending owner review** · Prepared: 2026-10-24
Related: [CONVERSION_ARCHITECTURE](CONVERSION_ARCHITECTURE.md) · [JOB_SYSTEM_DESIGN](JOB_SYSTEM_DESIGN.md) · [CONVERSION_BACKENDS](CONVERSION_BACKENDS.md)

Security is a first-class requirement. The baseline posture continues from v0.1 (`ARCHITECTURE.md` §10): no network, no telemetry, no privileged access, argv-only process execution, and never modify user files in built-in actions. v0.2.0 introduces the first built-in action that **creates** files. This document defines how it does so safely.

---

## 1. Assets, actors, trust boundaries

| Asset | Why it matters |
|-------|---------------|
| Source files | Must never be modified, moved, truncated, or deleted (approved 4.7) |
| Existing files in destination directories | Must never be overwritten |
| User privacy (EXIF GPS, serials) | Must not be leaked unintentionally |
| Desktop availability | Conversions must not freeze or exhaust the session |
| Job history database | Integrity (crash-safe). Contains file paths (private). |

| Actor | Capability |
|-------|-----------|
| The user | Selects files, triggers actions. Trusted. |
| **Untrusted file content** | Images downloaded from anywhere: crafted headers, bombs, decoder exploits |
| Another local user (shared dirs such as `/tmp`, removable media, network shares) | Can pre-create files, symlinks, or race on names in world-writable directories |
| Other processes of the same user | Can modify sources during conversion, can connect to user-owned sockets (same uid: the OS considers them trusted; Radivine does not defend against same-uid malware) |
| External converter binaries (future) | Code outside Radivine's control |

Trust boundaries: (1) file bytes → decoder, (2) filesystem namespace → output commit, (3) IPC socket → worker, (4) config files → worker/daemon (same user, trusted but validated), (5) worker → external programs (future).

## 2. Threat model

| # | Threat | Mitigation | Residual risk |
|---|--------|-----------|---------------|
| T1 | Maliciously crafted image triggers memory corruption | **All decoding uses memory-safe pure-Rust decoders** (png, zune-jpeg, image-webp). libwebp (C) is used **only to encode** pixel buffers that Radivine produced itself. Decoding runs in a separate process (`radivine-worker`), not in the wheel daemon. Fuzzing (§6). | Logic bugs/panics in Rust decoders → caught per item (`catch_unwind`) and the item fails |
| T2 | Decoder panic, infinite loop, or hung I/O | `catch_unwind` per item. Per-item watchdog fences `CommitPermit` via O(1) lock-free-I/O `Mutex<ItemPhase>` (`PreCommit` / `JournalingCommit` → `AbortedBeforeCommit(TimedOut)`; `InCommit` → `timeout_observed = true` blocking any collision retry after `EEXIST`), wakes blocked governor waiters, drains responsive items (≤ 5 s without blocking on wedged SQLite locks), and exits with code 75 while holding `worker.lock` (`O_CLOEXEC`) until the kernel terminates the process (JOB_SYSTEM_DESIGN §7.3, §7.5). | A CPU-burning thread until drain completes (bounded to ≤ 5 s after timeout); **OS kernel boundary (C2 note):** if a thread is stuck in uninterruptible kernel sleep (`TASK_UNINTERRUPTIBLE` / `D` state on a stalled hard-mounted NFS or FUSE driver), neither `exit(75)` nor `SIGKILL` can destroy the kernel task or release its kernel-held `flock(worker.lock)` / fds until the kernel filesystem driver times out or the mount recovers |
| T3 | Decompression bomb / excessive dimensions | Header dimensions checked against `max_pixels` (default `100 MP`, user-configurable `1..=500 MP`, hard ceiling `500 MP`) and `max_dimension` (default `32,768` px, user-configurable `64..=65,500` px, WebP output ceiling `16,383` px, hard ceiling `65,500` px) **before decode** (see authoritative table in [`CONVERSION_PRODUCT_SPEC.md`](CONVERSION_PRODUCT_SPEC.md) §9.4). `image::Limits{max_image_width, max_image_height, max_alloc: Some(estimate.memory_bytes)}` passed to the decoder (allocation-time guard, verified in source). The governor reserves memory before decode. `max_file_mb` source cap (default `512 MiB`, user-configurable `1..=2,048 MiB`, hard ceiling `2,048 MiB`). Optional `MemoryMax=` in the unit [VERIFY]. | Highly compressed but in-limit images still cost CPU (bounded by the limits) |
| T4 | Symlink tricks on the **source** (e.g. the selection contains a symlink to `/dev/zero` or a FIFO) | **Unified source symlink policy:** Selected symlinks to regular files are **followed intentionally** (matching `radivine_core::enrich::inspect_regular_file`, `enrich.rs:483-538`). The engine opens the source with `O_RDONLY|O_NONBLOCK|O_CLOEXEC` (no `O_NOFOLLOW` on source open; `O_NONBLOCK` prevents blocking on a FIFO), then immediately calls **`fstat(src_fd)`** and requires `S_ISREG` (`ConversionError::SourceNotRegular` otherwise). When `OutputLocation::BesideSource` is active, `dest_dir` is the parent directory of the selected path (the symlink's parent directory, never resolved via `canonicalize`), and all destination/staging/temporary opens strictly enforce `O_NOFOLLOW`. The `max_file_mb` cap is applied to `fstat(src_fd).st_size`. | — |
| T5 | Source modified or replaced during conversion | Identity `(dev, ino, size, mtime_ns)` taken from `fstat` at open and compared again after decode. A mismatch fails the item with `SourceChanged`. All reads go through the single opened fd (a path swap after open has no effect). | A modification racing within one read is detected only by size/mtime (best effort) |
| T6 | Output path traversal | The output name is built from `file_name()` of the source (never contains `/`). The suffix is validated (`[A-Za-z0-9 ._-]{0,32}`, no `/`, no NUL, no leading `.`). All filesystem operations are **relative to opened directory fds** (`dest_dir` and private `stage_dir` via `openat`/`renameat2`/`linkat`/`unlinkat`). Names containing `/` or NUL, or equal to `.`/`..`, are rejected. | — |
| T7 | Destination symlink attack (attacker pre-creates `photo.webp` → `~/.bashrc` in a shared dir) | Temp files are created inside a private per-job staging subdirectory `.rvstage-<job>-<rand16>` (`0700`) with `O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW` (`O_EXCL` never follows a symlink at the final component). The commit uses **no-replace** primitives (`RENAME_NOREPLACE` / `RENAME_EXCL` / `linkat`), which fail with `EEXIST` if *any* entry exists at `final_name` in `dest_dir`, including a symlink, and never write through it. | — |
| T8 | Destination directory swapped (a parent symlink replaced mid-job) | The destination directory (`dest_dir`) and per-job staging directory (`stage_dir`) are opened as `O_RDONLY|O_DIRECTORY|O_CLOEXEC` descriptors and verified with `fstat`. All operations are relative to those fds. A path swap after open does not redirect writes. | The directory resolved at open time may itself be reached through a symlink (intended: the user's path) |
| T9 | Temp file exposure, leftovers, or recovery unlink race | Two-phase staging-directory journaling (`StageIntent` recorded in SQLite **before** `mkdirat`, and `StageCreated` `(stage_dev, stage_ino)` recorded **after** `mkdirat` + `fstat` and **before** temp creation), followed by exclusive temp creation (`item-<idx>-<rand16>.part`, mode `0600`, `O_RDWR\|O_CREAT\|O_EXCL\|O_NOFOLLOW\|O_CLOEXEC`) and `TempCreated` `(temp_name, temp_dev, temp_ino)` journaling before writing bytes. Unlinked on failure/cancel/recovery only when namespace exclusivity and inode identity are verified (§3.6), followed by `fsync(stage_dir_fd)` and `fsync(dest_dir_fd)` before clearing journal columns; otherwise preserved and reported as `OrphanedTempPreserved`. | Orphans if a crash occurs in the narrow window between `openat(O_EXCL)` and the `TempCreated` journal commit (`C1`), or after `mkdirat` before `StageCreated` (`C0b` if non-empty), or on permissive non-POSIX mounts where safe unlink ownership cannot be proven (§3.6) |
| T10 | Partial output visible under the final name | Content is written only to the temp file inside `stage_dir`, `fsync`ed, header-validated via the `O_RDWR` temp fd, and then atomically published into `dest_dir` via Rung A (`RENAME_NOREPLACE` / `RENAME_EXCL`) or Rung B (`linkat` + `unlinkat`). **There is no non-atomic copy fallback (`Rung C` removed).** Filesystems supporting neither Rung A nor Rung B fail cleanly with `FilesystemUnsupported`. | None — partial content is never visible under `final_name` |
| T11 | Concurrent jobs targeting the same file name | Kernel-enforced no-replace commit plus a retry loop with the next number (`1..=9999`) | — |
| T12 | Unintended overwrite | There is **no code path that overwrites**. `std::fs::rename` (which replaces on all platforms) is **banned** in `radivine-convert::output` (clippy `disallowed-methods` for `std::fs::rename`, `std::fs::write`, `File::create`, `OpenOptions::truncate(true)` in that crate). | — |
| T13 | Shell injection | No shell anywhere. Argv-only `Command`. Future external backends receive fds/stdio, not interpolated strings. File names are never placed in a shell string. | — |
| T14 | Untrusted external converters (future optional backends) | Fixed program names resolved only in fixed directories (`/usr/bin`, `/usr/local/bin`), never `$PATH` by default. Version probe with timeout. Cleared environment except an allowlist. stdin/stdout fds instead of paths. Hard timeout + kill. `prlimit` (rustix, applied after spawn) for address space/CPU. Separate process. | Bugs inside the external program (process-isolated; it runs as the user) |
| T15 | IPC abuse | Socket dir `0700`, socket `0600`, `SO_PEERCRED` uid check, bounded line-length caps (`16 MiB` for `submit`, `64 KiB` for all other requests, `4 MiB` for command responses, `64 KiB` for subscription events; oversized lines rejected with `"code":"frame_too_large"`, see [`JOB_SYSTEM_DESIGN.md`](JOB_SYSTEM_DESIGN.md) §6), `radivine-control convert --stdin` capped at `10,000` lines / `16 KiB` per line / `16 MiB` total, strict serde parsing, queue limits (`≤ 10,000` items per job, `≤ 256` active jobs, `≤ 50,000` pending items), and decoded paths required to be absolute local paths `≤ 4,096` bytes. The worker only reads files the user can read and writes where the user can write: it adds **no privilege**. | Same-uid processes can submit jobs (equivalent to running the user's own commands) |
| T16 | Resource exhaustion (CPU/RAM/disk/inodes) | `max_parallel`, nice/batch scheduling, memory budget, disk free-space precheck (`max(16 MiB, 2 × source size)`) + fast-fail, queue and item limits, history pruning | — |
| T17 | Privacy leak via metadata | EXIF **stripped by default**. ICC kept (not personally identifying). XMP/IPTC dropped. Logs never contain EXIF or file contents. Diagnostic export redaction (existing) extends to the job DB summaries. | The user opts into EXIF preservation |
| T18 | Malicious config values | Strict validation and clamping (`validate_strict`, ranges). A preset folder must be absolute. `suffix` is validated. Invalid → defaults + warning. | — |
| T19 | Job DB tampering | Same-uid trusted file, 0600. Parsed with bounds. Never used to construct shell commands. Paths re-validated before use. | — |
| T20 | Notification injection | Notification bodies use file *names* only. The D-Bus client sends strings as typed arguments (no markup interpretation is assumed; `<`/`&` escaped because some servers render markup). | — |

## 3. Output commit protocol (normative) [PROPOSED — ADR-C10]

### 3.1 Per-item steps

```text
1  src = open(source, O_RDONLY|O_NONBLOCK|O_CLOEXEC); st0 = fstat(src); require S_ISREG; size ≤ max_file_mb
   (source symlinks to regular files are followed intentionally; O_NONBLOCK + fstat(S_ISREG) rejects FIFOs/devices)
2  probe = sniff + header parse via src (no pixel decode); limits check; governor.reserve(estimate)
3  dest_dir = open(dest_dir_path, O_RDONLY|O_DIRECTORY|O_CLOEXEC); st_dest = fstat(dest_dir); require S_ISDIR;
   fstatvfs(dest_dir) free-space precheck (free ≥ max(16 MiB, 2 × st0.st_size))
4a stage_dir_name = ".rvstage-{job}-{rand16}"
   ctl.journal_stage_intent(StageIntentJournal {
     dest_dir: dest_dir_path, stage_dir_name,
   })?                                                                               (DB, BEFORE mkdirat so C0b knows path)
4b stage_dir = open_or_create_job_staging_dir(dest_dir, stage_dir_name):
     if creating new staging dir for this (job, dest_dir):
       mkdirat(dest_dir, stage_dir_name, 0700)
       fsync(dest_dir)                                                               (make stage_dir entry durable in dest_dir)
     stage_dir = openat(dest_dir, stage_dir_name, O_RDONLY|O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC)
     st_stage = fstat(stage_dir); require S_ISDIR ∧ st_stage.st_dev == st_dest.st_dev
                ∧ st_stage.st_uid == geteuid() ∧ ((st_stage.st_mode & 0o022) == 0 ∨ private_user_mount)
   ctl.journal_stage_created(StageCreatedJournal {
     dest_dir: dest_dir_path, stage_dir_name, stage_dev: st_stage.st_dev, stage_ino: st_stage.st_ino,
   })?                                                                               (DB, BEFORE openat temp file)
5  temp_name = "item-{idx}-{rand16}.part"
   tmp = openat(stage_dir, temp_name, O_RDWR|O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC, 0600)
   st_tmp = fstat(tmp); require S_ISREG ∧ st_tmp.st_dev == st_stage.st_dev
6  ctl.journal_temp_created(TempCreatedJournal {
     temp_name, temp_dev: st_tmp.st_dev, temp_ino: st_tmp.st_ino,
   })?                                                                               (DB, after O_EXCL creation, before write)
7  backend.convert(src → OutputSink(&mut tmp))  [cancellation checkpoints between stages]
8  st1 = fstat(src); require identity(st0) == identity(st1)                          → else SourceChanged
9  tmp.flush(); validate: seek(0) / pread on the O_RDWR tmp fd and re-parse output header
   (require st_size > 0 and format, width, height match expected target)
10 fchmod(tmp, source_mode & 0o666); optional futimens (preserve_mtime); fsync(tmp)
11 candidate = "{stem}{suffix}.{ext}"; attempt = 0
12 loop:
     req = CommitRequest::new(attempt, candidate, st_tmp.st_dev, st_tmp.st_ino)      (pub(crate) to radivine-convert)
     permit = ctl.begin_commit(req)?                                                 (Step A: O(1) lock -> JournalingCommit,
                                                                                      rejecting if InCommit.timeout_observed;
                                                                                      Step B: unlocked FULL DB Committing journal;
                                                                                      Step C: O(1) lock -> InCommit + req.authorize())
     match commit_no_replace(&permit, stage_dir, temp_name, dest_dir, candidate):
        Ok(CommitRung::RenameNoReplace) => {
          fsync(stage_dir)?;                                                         (make temp_name removal durable in stage_dir)
          fsync(dest_dir)?;                                                          (make candidate insertion durable in dest_dir)
          break
        }
        Ok(CommitRung::HardLinkThenUnlink) => {
          // Inside Rung B (§3.3): linkat -> fsync(dest_dir) -> unlinkat(stage_dir, temp_name) -> fsync(stage_dir)
          break
        }
        Err(EEXIST) => {
          attempt += 1; if attempt > 9999 → CollisionExhausted;
          candidate = "{stem}{suffix} ({attempt}).{ext}"                             (next begin_commit checks timeout_observed)
        }
        Err(other)  => map error (rung fallback in §3.3; if both Rung A and B unsupported → FilesystemUnsupported)
13 ctl.complete_commit(permit) -> persist Succeeded{output} and clear temp_name/temp_dev/temp_ino
   ONLY AFTER both fsync(stage_dir) and fsync(dest_dir) have succeeded (§3.5)
14 At job/batch completion when no items in this job still reference stage_dir:
   close(stage_dir); unlinkat(dest_dir, stage_dir_name, AT_REMOVEDIR); fsync(dest_dir);
   then clear stage_dir_name/stage_dev/stage_ino in DB; release reservation
on any error/cancel after step 5:
   cleanup_temp_safely(stage_dir, st_stage, temp_name, st_tmp) + fsync(stage_dir) (§3.6) BEFORE clearing temp journal
```

### 3.2 Why not plain `rename` or check-then-replace

`rename(2)` (and `std::fs::rename` on all platforms, including Windows where it uses `MOVEFILE_REPLACE_EXISTING`) **atomically replaces** an existing destination. Any check-then-rename sequence (`if !exists { rename }`, or `openat(O_CREAT|O_EXCL)` reservation followed by `renameat` replacement) has a TOCTOU window where another actor in a shared directory can replace the target entry between the check/reservation and the replacing `renameat`, or leaves a 0-byte placeholder under `final_name` if the process crashes between the two calls. Only kernel-level **no-replace** publication primitives are used.

### 3.3 Commit primitive ladder (`commit_no_replace`)

| Rung | Primitive & Directory `fsync` Ordering | Platforms / filesystems | Semantics |
|------|----------------------------------------|-------------------------|-----------|
| **A** | `renameat2(stage_dir, temp_name, dest_dir, final_name, RENAME_NOREPLACE)`: `rustix::fs::renameat_with(…, RenameFlags::NOREPLACE)` (Linux). `renameatx_np(RENAME_EXCL)` (macOS, same rustix API, verified `rustix-1.1.5/src/fs/at.rs:302`), followed immediately by **`fsync(stage_dir)`** then **`fsync(dest_dir)`**. | ext4 (3.15+), btrfs/tmpfs/cifs (3.17+), xfs (4.0+), vfat/exfat/ext2/… (4.9+) [WEB man renameat2] | Atomic directory-entry move from `stage_dir` to `dest_dir` on the same `st_dev`; fails with `EEXIST` if `final_name` exists. `fsync(stage_dir)` makes removal of `temp_name` durable in `stage_dir`, and `fsync(dest_dir)` makes insertion of `final_name` durable in `dest_dir`, **before** `Succeeded` or journal cleanup is committed to SQLite. |
| **B** | 1. `linkat(stage_dir, temp_name, dest_dir, final_name, 0)`<br/>2. **`fsync(dest_dir)`**<br/>3. `unlinkat(stage_dir, temp_name, 0)`<br/>4. **`fsync(stage_dir)`** | POSIX filesystems with hard links, including NFS. Used when Rung A returns `EINVAL`/`ENOSYS`/`EOPNOTSUPP`. | Atomic creation of `final_name` with the exact `(temp_dev, temp_ino)` inode; fails with `EEXIST` if `final_name` exists. `fsync(dest_dir)` guarantees `dest_dir/final_name` is durable on disk **before** the staging link `stage_dir/temp_name` is unlinked, and `fsync(stage_dir)` guarantees the staging link removal is durable **before** `temp_name` is cleared in SQLite. If `unlinkat` or `fsync(stage_dir)` fails after `linkat` + `fsync(dest_dir)` succeeded, the item still transitions to `Succeeded` (output is durably published), but `temp_name`/`stage_dir_name` remain non-`NULL` in SQLite so startup recovery (`C6`) retries `cleanup_temp_safely` + `fsync(stage_dir)`. |
| — | Neither Rung A nor Rung B is supported (e.g. `linkat` returns `EPERM`/`EOPNOTSUPP`/`EXDEV`/`EMLINK` after Rung A failed) | Certain FUSE/GVFS mounts or non-POSIX drivers without `RENAME_NOREPLACE` or hard links [VERIFY P8] | **Fail safely with `ConversionError::FilesystemUnsupported`** ("This location doesn't support safe atomic file creation"). Clean up `temp_name` in `stage_dir` + `fsync(stage_dir)` (§3.6) and offer **Retry to another folder…**. **Rung C (non-atomic exclusive copy into `final_name`) is rejected and omitted** so partial or corrupt files are never exposed under `final_name`. |

The rung that works is cached per `(st_dev)` for the worker lifetime to avoid repeated failing syscalls. Windows (future): `MoveFileExW(tmp, final, 0)` (no `REPLACE_EXISTING`, fails with `ERROR_ALREADY_EXISTS`). It lives in a separate tiny crate because the FFI is `unsafe`.

**`O_TMPFILE` + `linkat("/proc/self/fd/N", AT_SYMLINK_FOLLOW)`** (Linux) would avoid named temp files on supporting filesystems and is kept as a future enhancement candidate after v0.2.0. It is not the baseline because filesystem support varies (`EOPNOTSUPP` on NFS/FUSE/vfat) and it requires `/proc`.

### 3.4 Validation before commit

- **File descriptor mode (`O_RDWR`):** Step 5 opens `tmp` with `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC` (mode `0600`). Backends only receive `OutputSink<'a>` (which exposes `std::io::Write` and `as_fd()`, never `Read` or paths).
- **Mandatory header verification (Step 9):** After `backend.convert` returns, the engine flushes the buffered writer, checks `fstat(tmp).st_size > 0`, seeks to offset `0` (or uses `pread`) on the **same `O_RDWR` `tmp` file descriptor** (never reopening by pathname), and runs `sniff_format` + header inspection to verify that the written output format, width, and height match expectations. A mismatch fails the item before `fchmod`/`fsync(tmp)` and before `begin_commit`.
- **Optional (`verify = "full"`, off by default):** fully decode the output from `tmp`. It costs time and is recommended only for archival use.

### 3.5 Durability and ordering policy

| Step | Policy | Rationale |
|------|--------|-----------|
| 1a. `StageIntent` journal (`dest_dir`, `stage_dir_name`) **before** `mkdirat` | Written to SQLite before `mkdirat(dest_dir, stage_dir_name, 0700)` | Guarantees across process crashes (`SIGKILL`/`abort`/`exit(75)`) that if a crash occurs before or right after `mkdirat` (`C0b`), recovery has a durable reference to `(dest_dir, stage_dir_name)` without scanning `dest_dir` by wildcard pattern. *(Power-loss durability note for C2: under `WAL synchronous=NORMAL`, sudden power loss before WAL `fsync` can roll back `StageIntent`/`StageCreated`/`TempCreated` while `mkdirat` or `openat(O_EXCL)` reached disk, leaving at most an unjournaled `0700` `.rvstage-*` or `0600` `.part` orphan on disk and never modifying/deleting user files; C2 must either use `PRAGMA synchronous=FULL` for pre-commit journals or explicitly document this power-loss orphan boundary.)* |
| 1b. `mkdirat` + `fsync(dest_dir)` + `fstat(stage_dir)` → `StageCreated` journal (`stage_dev`, `stage_ino`) **before** `openat(temp_name, O_EXCL)` | `fsync(dest_dir)` makes `.rvstage-<job>-<rand16>` durable in `dest_dir`, and `StageCreated` records `(stage_dev, stage_ino)` before any temp file is created | Guarantees across process crashes (and across power loss when `StageCreated` is WAL-synced) that recovery knows the verified `(stage_dev, stage_ino)` identity of `.rvstage-<job>-<rand16>` before `openat(O_EXCL)` can create any file inside it (`C0c`, `C1`). |
| 1c. `openat(O_EXCL)` + `fstat(tmp)` → `TempCreated` journal (`temp_name`, `temp_dev`, `temp_ino`) | `TempCreated` written **after** `tmp` is created and `fstat`ed, and **before** any bytes are written | Guarantees `temp_dev` and `temp_ino` are known before writing data. If the process crashes (or power fails before WAL sync under `synchronous=NORMAL`) between `openat(O_EXCL)` and durable `TempCreated` (`C1`), recovery never deletes the unjournaled file by filename pattern, instead preserving it and recording `OrphanedTempPreserved`. |
| 2. `fsync(tmp)` before `begin_commit` | **Always** | Without `fsync(tmp)`, delayed allocation can leave a zero-length file after power loss (`ext4` `auto_da_alloc` only covers rename-*replace*, not newly created names). |
| 3. `Committing` journal inside `ctl.begin_commit(...)` | **Durable (`PRAGMA synchronous=FULL`) before `commit_no_replace`**, executed in `ItemPhase::JournalingCommit` **without** holding `Mutex<ItemPhase>` | Preserves the durable-before-publication guarantee (`final_name` is on stable storage before `renameat2`/`linkat` executes) while keeping `Mutex<ItemPhase>` O(1) and non-blocking so the watchdog can never stall on a wedged SQLite write (`JOB_SYSTEM_DESIGN` §7.5). |
| 4. `fsync(stage_dir)` and `fsync(dest_dir)` before persisting `ItemState::Succeeded` or clearing temp journals | **Always before `Succeeded` is committed or `temp_name` is cleared in SQLite** (Rung A: `renameat2` → `fsync(stage_dir)` → `fsync(dest_dir)`; Rung B: `linkat` → `fsync(dest_dir)` → `unlinkat` → `fsync(stage_dir)`) | Output publication mutates directory entries in **both** `stage_dir` and `dest_dir`. Synchronizing both directories before recording `Succeeded` and clearing `temp_name = NULL` ensures power loss can never drop the published entry in `dest_dir` or resurrect a unlinked entry in `stage_dir` after the recovery journal is cleared. |
| 5. Staging-directory removal (`unlinkat(dest_dir, stage_dir_name, AT_REMOVEDIR)`) + `fsync(dest_dir)` | **Always `fsync(dest_dir)` before clearing `stage_dir_name = NULL, stage_dev = NULL, stage_ino = NULL` in SQLite** | Ensures `.rvstage-<job>-<rand16>` removal is durable in `dest_dir` before forgetting the staging directory's journal reference. |
| 6. DB item terminal results (`Succeeded` / `Failed` / `Skipped` / `Cancelled`) | WAL `synchronous=NORMAL` (or `FULL`), batched ≤ 250 ms or 32 items **after** `fsync(stage_dir)` and `fsync(dest_dir)` | Survives process crashes; if power loss rolls back the final `Succeeded` batch transaction, the durable `Committing` row remains and startup recovery (`C4`) re-verifies `(temp_dev, temp_ino)` on `final_name`, re-runs `fsync(dest_dir)` / `fsync(stage_dir)`, and restores `Succeeded`. |

### 3.6 Temporary-file ownership, namespace protection, and safe cleanup model

POSIX `unlinkat(dirfd, name, 0)` operates on a directory entry **by name**, not by file descriptor. Even if a process calls `fstatat(dirfd, name, AT_SYMLINK_NOFOLLOW)` to check `(st_dev, st_ino)` immediately before `unlinkat(dirfd, name, 0)`, that two-step sequence is only race-free if **no untrusted actor can mutate `dirfd` between `fstatat` and `unlinkat`**.

Radivine enforces this invariant through a **two-layer ownership model**:

1. **Private Per-Job Staging Subdirectory (`.rvstage-<job_id>-<rand16>`, mode `0700`):**
   - For each destination directory `dest_dir` used by a job, the worker first journals `StageIntent { dest_dir, stage_dir_name }` in SQLite, then creates `.rvstage-<job_id>-<rand16>` inside `dest_dir` using `mkdirat(dest_dir_fd, stage_dir_name, 0o700)` + `fsync(dest_dir_fd)`, opens it with `openat(dest_dir_fd, stage_dir_name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)`, verifies `fstat(stage_dir_fd)`, and journals `StageCreated { dest_dir, stage_dir_name, stage_dev, stage_ino }` **before** creating any `item-<idx>-<rand16>.part` file inside `stage_dir_fd`.
   - Because `stage_dir_fd` resides inside `dest_dir`, it is guaranteed to be on the **same mount and filesystem (`st_dev`)** as `dest_dir`, so `renameat2(stage_dir_fd, temp_name, dest_dir_fd, final_name, RENAME_NOREPLACE)` and `linkat(stage_dir_fd, temp_name, dest_dir_fd, final_name, 0)` are atomic same-filesystem operations.
   - On POSIX filesystems, mode `0700` + `st_uid == geteuid()` on `stage_dir_fd` ensures the kernel VFS denies write/search access inside `stage_dir_fd` to every other non-root local user—**even when `dest_dir` itself is a shared or world-writable directory such as `/tmp` (`1777`)**.
   - Concurrent Radivine worker instances are serialized by the exclusive `flock(LOCK_EX)` on `$XDG_RUNTIME_DIR/radivine/worker.lock` (`O_CLOEXEC`).

2. **Verified Cleanup Preconditions & Directory Durability (`cleanup_temp_safely`):**
   - Before unlinking `temp_name` from `stage_dir` (either during normal error/cancel cleanup or during startup crash recovery after the previous worker process has exited), the worker MUST verify **all** of the following conditions:
     1. **Single-writer lock held:** The current process holds `flock(worker.lock, LOCK_EX)`, and (for in-process cleanup) the item is not an active timed-out thread (timed-out items are cleaned up **only** by the next worker instance after process exit; see JOB_SYSTEM_DESIGN §7.5).
     2. **Valid single-component names:** `stage_dir_name` matches `^\.rvstage-[0-9]+-[0-9a-f]{16}$` and `temp_name` matches `^item-[0-9]+-[0-9a-f]{16}\.part$` (no `/`, no NUL, not `.` or `..`).
     3. **Staging directory identity & exclusive namespace protection:** `stage_dir_fd` (opened via `openat(dest_dir_fd, stage_dir_name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)`) has a non-`NULL` journaled `(stage_dev, stage_ino)` and satisfies `fstat(stage_dir_fd)`:
        - `S_ISDIR(st_mode)`
        - `(st_dev, st_ino) == (stage_dev, stage_ino)` recorded in `StageCreated`
        - `st_uid == geteuid()`
        - **Namespace exclusivity check:** `(st_mode & 0o022) == 0` (neither group-writable nor world-writable), **or** the filesystem mount itself is a private user mount under `/run/media/<uid>/` or `$XDG_RUNTIME_DIR` whose parent directory is `0700`/`0750` owned by `geteuid()`.
     4. **Temp file inode identity:** `(temp_dev, temp_ino)` is non-`NULL` in `TempCreated`, and `fstatat(stage_dir_fd, temp_name, AT_SYMLINK_NOFOLLOW)` satisfies:
        - `S_ISREG(st_mode)` (never a symlink, directory, FIFO, or device)
        - `(st_dev, st_ino) == (temp_dev, temp_ino)` recorded in `TempCreated`
        - `st_uid == geteuid()`
   - **Only when all four preconditions hold** does the worker:
     1. Call `unlinkat(stage_dir_fd, temp_name, 0)`.
     2. Call **`fsync(stage_dir_fd)`** to make the temp entry removal durable in `stage_dir` **before** clearing `temp_name = NULL, temp_dev = NULL, temp_ino = NULL` in SQLite.
     3. Once no remaining items in the job reference `stage_dir_name`, close `stage_dir_fd`, call `unlinkat(dest_dir_fd, stage_dir_name, AT_REMOVEDIR)` (`AT_REMOVEDIR` is kernel-enforced to fail with `ENOTEMPTY` if any entry remains), call **`fsync(dest_dir_fd)`** to make the staging-directory removal durable in `dest_dir`, and **only then** clear `stage_dir_name = NULL, stage_dev = NULL, stage_ino = NULL` in SQLite.
   - **Safe Orphan Preservation Rule (when ownership or namespace exclusivity cannot be verified):**
     - If **any** precondition fails (for example, `stage_dev`/`stage_ino` is `NULL` after a `C0b` crash before `StageCreated` and `stage_dir` is non-empty, or `temp_dev`/`temp_ino` is `NULL` after a `C1` crash before `TempCreated`, or `stage_dir_fd` is on a shared non-POSIX mount where `(st_mode & 0o022) != 0` and parent mount exclusivity cannot be proven, or `(st_dev, st_ino)` does not match the journaled identity), Radivine **never invents an ownership guarantee and never calls `unlinkat` on any file**.
     - Instead, it **preserves the file/directory untouched**, records `ItemWarning::OrphanedTempPreserved { path, reason }` in the database and worker log, and surfaces the orphan path in the Jobs page so the user can inspect or remove it manually. Radivine never performs blind wildcard directory scans to delete `item-*.part` or `.rvstage-*` entries.

### 3.7 Source preservation invariant (testable)

> **INV-SRC:** For every conversion, success or failure, cancellation, crash, or fault injection, every source file keeps identical content (SHA-256), size, mtime, inode, mode, and path. The engine never holds a writable fd to a source and never calls `rename`/`unlink`/`truncate` on a path it did not create with `O_EXCL` in a verified staging directory.

Enforcement:
1. API: `SourceReader` exposes only `Read + Seek` over an fd opened `O_RDONLY`.
2. Lint: `disallowed-methods` in `radivine-convert` / `radivine-convert-image` (see T12).
3. Tests: a property test over random option combinations and injected faults at every stage asserts INV-SRC plus **INV-NOOVERWRITE** (every pre-existing file in the destination directory is byte-identical after the run) plus **INV-NOTEMP** (no `.rvstage-*` or `*.part` left after a non-crash run in a verified private staging directory, and unverified entries preserved as reported orphans).
4. Crash tests: abort at each stage, run recovery, assert INV-SRC, INV-NOOVERWRITE, and exact recovery state reconciliation.
5. Same-name targets (`a.png` → PNG in the same directory in Advanced mode) must produce `a (1).png`.

## 4. Process isolation and privileges

- **Untrusted full pixel decoding happens only in `radivine-worker`.** The wheel daemon never opens or parses image files beyond the existing 8 KiB magic-number sniff in `radivine-core::enrich` (`sniff_magic_mime`). The Control Center's main window never parses image files. The `radivine-control convert` (Advanced Convert) window performs bounded, read-only **header inspection only** (`ConversionEngine::inspect`, capped at 1,000 items, zero pixel buffer allocation) on a background thread using the same memory-safe pure-Rust header parsers and dimension/file-size limits.
- The worker needs no privileges. `NoNewPrivileges=yes` in the unit. Further sandboxing (`SystemCallFilter`, `RestrictAddressFamilies=AF_UNIX`, `MemoryMax`) is evaluated in C5 and enabled only if verified to start reliably on Mint 22 user managers [VERIFY].
- Future external backends always run as child processes of the worker with the constraints of T14.

## 5. Permission boundaries

- The worker acts with the user's own permissions. It cannot read what the user cannot read and cannot write where the user cannot write.
- Non-local URIs (`sftp://`, `smb://` from gvfs without a FUSE path) are skipped as `NotLocal` (same rule as `Item::local_path`). gvfs FUSE paths (`/run/user/<uid>/gvfs/…`) are treated as local, with the P8 rung results.
- Files on removable or read-only media: a read-only source is fine; a read-only destination fails with a clear message and Retry-to-folder.

## 6. Verification activities

| Activity | Milestone |
|----------|-----------|
| `cargo-fuzz` targets in the existing `fuzz/` crate: `sniff_format`, `inspect` (header parse with limits), `convert_small` (end-to-end with tiny limits into a tmpfs dir), `exif_orientation_reset`, `webp_riff_mux`, `wirepath_roundtrip`, `worker_protocol_parse`, `convert_toml_parse` | C1–C2 (targets), C5 (≥ 1 h each, recorded) |
| Malformed corpus regression suite (truncated, bad CRC, huge dims, zero dims, palette overflow, bomb PNGs, polyglot files, wrong extensions) | C1 |
| INV-SRC / INV-NOOVERWRITE / INV-NOTEMP property tests + crash tests (`T-STAGE-CRASH-WINDOW` covering `C0a`/`C0b`/`C0c`/`C1`, `T-STAGE-DIR-FSYNC` verifying `fsync(stage_dir)` and `fsync(dest_dir)` ordering before `Succeeded`/journal clear, `T-WATCHDOG-LOCK-NONBLOCKING` stalling SQLite inside `JournalingCommit`, and `T-WATCHDOG-COLLISION-FENCE` verifying `InCommit.timeout_observed` blocks collision retry after `EEXIST`) | C1 (engine), C2 (worker) |
| Symlink/race tests: pre-planted symlink at the output name, symlinked destination dir swapped mid-job, FIFO as source, `/dev/zero` symlink | C1 |
| `cargo deny check`, `cargo audit` (CI job) | C1 onward |
| Security review checklist (this document) signed off in the C5 test report | C5 |
