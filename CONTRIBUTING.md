# Contributing to Radivine

Thank you for your interest in contributing to **Radivine** ("Branch into action.")!

---

## 1. Development Environment Setup

- **OS:** Linux (Linux Mint 22.x / Ubuntu 24.04+ on X11 recommended for desktop integration work).
- **Rust Toolchain:** Rust `1.85+` (`edition = "2024"`), `rustfmt`, and `clippy`.

Clone and verify the workspace:

```bash
git clone https://github.com/marawan6569/Radivine.git
cd Radivine

DISPLAY="" cargo fmt --all --check
DISPLAY="" cargo clippy --workspace --all-targets -- -D warnings
DISPLAY="" cargo test --workspace
```

---

## 2. Host Desktop Safety & Isolated X11 Testing

> [!IMPORTANT]
> **Never run automated X11 GUI tests against your active desktop session (`DISPLAY=:0` or `DISPLAY=:1`).**

All live X11 integration tests are guarded by `radivine_platform::is_isolated_test_display()`, which refuses to inject synthetic pointer/keyboard events unless `DISPLAY` points to an isolated `Xvfb` server (`/tmp/.X<N>-lock` owned by `Xvfb`).

- **Standard unit, property, fuzz, and headless soak tests:** Run with `DISPLAY=""`:
  ```bash
  DISPLAY="" cargo test --workspace
  ```
- **Isolated X11 integration tests (when `Xvfb` is installed):**
  ```bash
  xvfb-run -n 99 -s "-screen 0 1920x1080x24 +extension Composite +extension RENDER +extension MIT-SHM" \
      cargo test --workspace -- --test-threads=1
  ```

---

## 3. Architectural & Safety Invariants

1. **Crate Safety Boundaries:**
   - `crates/radivine-wheel`, `crates/radivine-render`, `crates/radivine-actions`, and `apps/radivine-viewer` must retain `#![forbid(unsafe_code)]`.
   - `crates/radivine-wheel` must remain pure, deterministic, platform-independent, and depend only on `smallvec` at runtime, with **zero heap allocations** in steady-state `handle`/`tick`/`scene` calls (`cargo test -p radivine-wheel --test alloc_test`).
2. **Non-Destructive Drag & Drop:**
   - Radivine's XDND v5 target must only reply with `XdndActionPrivate` (or `None` when rejecting) and must **never** reply with `XdndActionMove` or request the `DELETE` selection target.
   - `input.passthrough` defaults to `false` (`ADR-0001`) so releasing a drag outside the wheel cancels cleanly without dropping files into underlying windows.
3. **Command Execution Safety:**
   - Custom actions (`actions.toml`) must execute directly via `Command::new(program).args(...)` without invoking a shell.
4. **Protected Branding Assets:**
   - Do not modify, overwrite, or regenerate existing branding or icon files under `assets/branding/`, `assets/icons/`, or `assets/Radivine_ Branch Into Action.png`.

---

## 4. Commit & Pull Request Guidelines

- Follow the [Conventional Commits](https://www.conventionalcommits.org/) specification (`feat(wheel): ...`, `fix(platform): ...`, `docs: ...`, `test(core): ...`).
- Keep commits focused, incremental, and logically self-contained.
- Ensure `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass before opening a pull request.

---

## 5. Contribution Licensing (`MIT OR Apache-2.0`)

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Radivine by you, as defined in the Apache-2.0 license, shall be dual-licensed under **`MIT OR Apache-2.0`** ([`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE)), without any additional terms or conditions.
