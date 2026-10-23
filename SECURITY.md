# Security Policy

## Supported Versions

| Version | Supported |
| :--- | :--- |
| `0.1.x` | :white_check_mark: Yes |

---

## Reporting a Vulnerability

If you discover a security vulnerability in **Radivine** or `radivine-viewer`, please report it privately rather than opening a public GitHub issue:

1. **GitHub Private Vulnerability Reporting:** Use the [Report a vulnerability](https://github.com/marawan6569/Radivine/security/advisories/new) button on the repository's **Security** tab, or
2. **Email:** Contact the maintainer directly at `marawan6569@gmail.com` with `[SECURITY] Radivine` in the subject line.

Please include:
- Affected component (`radivine`, `radivine-viewer`, `radivine-core`, `radivine-actions`, `radivine-platform`, etc.)
- Steps or input payload (`text/uri-list`, `actions.toml`, or file sample) to reproduce
- Impact assessment

We aim to acknowledge reports within **72 hours** and provide a fix and advisory once verified.

---

## Security Architecture & Invariants

Radivine is designed with defense-in-depth across desktop input, IPC, file inspection, and command execution:

1. **Shell-Free Custom Action Execution (`crates/radivine-actions`):**
   - Custom actions in `actions.toml` are executed directly via `std::process::Command::new(program).args(...)` (`execve`).
   - Shell interpreters (`/bin/sh -c`, `bash -c`, etc.), relative program paths, and partial argument interpolations (such as `--file={path}`) are rejected at configuration load time.
   - Filenames containing spaces, quotes, newlines, or shell metacharacters (`;`, `&`, `$(...)`, backticks) are passed as literal OS argument vectors and are never interpreted by a shell.
2. **Non-Destructive XDND v5 Protocol (`crates/radivine-platform`):**
   - Radivine only replies with `XdndActionPrivate` (or `None` when rejecting a drop) and **never** replies with `XdndActionMove` or requests the `DELETE` selection target, guaranteeing that drag sources never delete or move user files.
   - `input.passthrough` defaults to `false` (`ADR-0001`) so releasing a drag outside the radial wheel cancels cleanly rather than dropping files into underlying windows.
   - Selection transfers enforce a `2 s` timeout and a `16 MiB` payload size cap.
   - `file://` URIs containing interior NUL bytes (`%00` or `\0`) are rejected before path construction.
3. **TOCTOU-Free Non-Blocking File Inspection (`radivine-core` & `radivine-viewer`):**
   - Files are opened with `O_RDONLY | O_NONBLOCK | O_CLOEXEC` and verified via `fstat` on the open file descriptor before reading, preventing blocks on named pipes (`FIFO`), device nodes, or sockets.
   - `radivine-viewer` uses chunked `pread` (`FileExt::read_at`) rather than `mmap`, preventing `SIGBUS` crashes when another process truncates an open file (`AC-5.4`).
4. **Memory Safety (`#![forbid(unsafe_code)]`):**
   - `radivine-wheel`, `radivine-render`, `radivine-actions`, and `radivine-viewer` enforce `#![forbid(unsafe_code)]`.
   - The workspace contains only 4 audited `unsafe` blocks: 3 POSIX shared-memory FFI calls (`shmget`, `shmat`, `shmdt`/`shmctl`) in `crates/radivine-platform/src/x11/shm.rs` and 1 `BorrowedFd::borrow_raw` wrapper for the `calloop` X11 connection poll source in `apps/radivine/src/main.rs`.
5. **Private IPC Socket Permissions (`apps/radivine`):**
   - The daemon Unix domain socket (`$XDG_RUNTIME_DIR/radivine.sock`) is created inside the user's private runtime directory (`0700`) with single-instance lock protection.
