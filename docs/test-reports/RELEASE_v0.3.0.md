# Final Stable Release Verification Report — Radivine v0.3.0 (`radivine_0.3.0_amd64.deb`)

- **Release Identity:**
  - **Cargo Workspace Version:** `0.3.0`
  - **Debian Package Version:** `0.3.0` (`radivine_0.3.0_amd64.deb`)
  - **Git Tag:** `v0.3.0`
  - **GitHub Release Title:** `Radivine v0.3.0 — Concentric Rings`
  - **GitHub Release State:** `draft=false`, `prerelease=false`, `make_latest=true` (`Latest` stable release)
  - **Date:** `2026-10-26`
  - **Owner Desktop Release Sign-Off:** Approved on `2026-10-26` following live installation and testing of `v0.3.0-rc.1` (`radivine_0.3.0~rc1_amd64.deb`) by the project owner on a separate laptop running **Linux Mint 22.3 Cinnamon (X11, `amd64`)**.

---

## 1. Freshly Built Stable Release Artifacts, Sizes & SHA-256 Checksums

Freshly built from the final `0.3.0` stable source via `./scripts/build-deb.sh` (`--release` profile, stripped with `strip --strip-unneeded`) and verified with `lintian --tag-display-limit 0 target/debian/radivine_0.3.0_amd64.deb` (**`0` errors, `0` warnings**).

| Artifact | Path in Package / Build Tree | Size (`bytes`) | Size (`MiB`) | `--version` Output | SHA-256 |
|---|---|---:|---:|---|---|
| **`radivine`** | `/usr/bin/radivine` | `9,940,768 B` | **`9.48 MiB`** | `radivine 0.3.0` | `7582a1eb42f361ad0334aafe3e5f17d40fa742b6400bf2859ab7a25aa6b0bfa0` |
| **`radivine-control`** | `/usr/bin/radivine-control` | `18,606,768 B` | **`17.74 MiB`** | `radivine-control 0.3.0` | `41ec36c1890c85885fa3b1790bf837b8c8078ff6c1133e78de984aab00faf57e` |
| **`radivine-viewer`** | `/usr/bin/radivine-viewer` | `13,349,088 B` | **`12.73 MiB`** | `radivine-viewer 0.3.0` | `1c11d37cf72a639aae89e723a221b4d05f73ac8011b1ddf9d1fa8a9c08c196cc` |
| **`radivine-worker`** | `/usr/bin/radivine-worker` | `7,982,536 B` | **`7.61 MiB`** | `radivine-worker 0.3.0` | `33007d726e55a9eeb34a88709077eb8a1b084a97d10b4e483704c7e669545f9f` |
| **`radivine_0.3.0_amd64.deb`** | `target/debian/radivine_0.3.0_amd64.deb` | **`16,149,118 B`** | **`15.40 MiB`** | `Version: 0.3.0` | **`4c0b8cc719963dec2f2457588a500a3530dd4aa88b01d0b3954eeb57edb37156`** |

---

## 2. Debian Package Metadata (`dpkg-deb -I`) & Runtime Font Dependencies

```text
Package: radivine
Version: 0.3.0
Section: utils
Priority: optional
Architecture: amd64
Depends: libc6 (>= 2.34), libgcc-s1 (>= 3.0), fonts-dejavu-core, libfontconfig1
Recommends: nemo, libx11-6, libegl1, libgl1, systemd, fonts-noto-core
Maintainer: marawan6569 <marawan6569@gmail.com>
Homepage: https://github.com/marawan6569/Radivine
Installed-Size: 49056
```

- **Embedded Manpages (`man1`):** `radivine.1.gz`, `radivine-viewer.1.gz`, `radivine-control.1.gz`, and `radivine-worker.1.gz` all carry `.TH ... "Radivine 0.3.0"`.
- **Arabic Font & Runtime Libraries:** `Depends: ..., fonts-dejavu-core, libfontconfig1` guarantees `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`, `/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf`, and `libfontconfig.so.1` are present on every installation.

---

## 3. Pre-Publication Verification Gates

| Gate | Command / Environment | Result |
|---|---|---|
| **1. Formatting (`cargo fmt`)** | `cargo fmt --all --check` | **PASS** |
| **2. Architecture Dependency Boundaries** | `./scripts/check-dep-graph.sh` | **PASS** (`radivine-wheel` depends solely on `smallvec v1.16.3`; all 6 binary/crate boundary rules verified) |
| **3. Clippy Lints (`-D warnings`)** | `DISPLAY="" cargo clippy --workspace --all-targets -- -D warnings` | **PASS** (`0` warnings) |
| **4. Full Workspace Unit & Integration Tests** | `DISPLAY="" cargo test --workspace` | **PASS** (`585` tests passed, `0` failed) |
| **5. Isolated `Xvfb` X11 / XDND / `XShape` Suites** | `xvfb-run -a -s "-screen 0 1920x1080x24 +extension COMPOSITE +extension RANDR"` (`nested_acceptance` + `x11_tests`) | **PASS** (`8` `nested_acceptance` + `15` `x11_tests` = `23` real X11 tests passed, `0` failed) |
| **6. Supply-Chain & License Audit** | GitHub Actions `Supply-Chain & License Audit (cargo-deny)` + `LICENSE-MIT` / `LICENSE-APACHE` / `THIRD_PARTY_NOTICES.md` | **PASS** |
| **7. Debian Package Lintian Audit** | `lintian --tag-display-limit 0 target/debian/radivine_0.3.0_amd64.deb` | **PASS** (`0` errors, `0` warnings) |
| **8. Long-Duration N5 Endurance & Fuzz Suites** | Reused from [`docs/test-reports/N5.md`](N5.md) (`0` runtime code changes between N5/N6 and `v0.3.0`): `3,605.40 s` M6 mixed-layout soak (`84,455` sessions), `1,200.01 s` M6 fuzz (`503.7M` iterations), `3,606.25 s` C5 worker soak (`80,108` items) | **PASS** |

---

## 4. Isolated `ubuntu:24.04` Container Clean-Install, Upgrade, Rollback & Removal Matrix

Executed in clean `ubuntu:24.04` Docker containers with only `target/debian:/debs:ro` mounted:

| Test Case | Procedure | Result |
|---|---|---|
| **1. Debian Version Ordering** | `dpkg --compare-versions "0.2.0" lt "0.3.0~rc1"` and `"0.3.0~rc1" lt "0.3.0"` and `"0.2.0" lt "0.3.0"` | **PASS** (`0.2.0 < 0.3.0~rc1 < 0.3.0`) |
| **2. Fresh Install of `radivine_0.3.0_amd64.deb` & Font/Library Check** | `apt-get install -y --no-install-recommends /debs/radivine_0.3.0_amd64.deb` | **PASS** — Automatically installed `fonts-dejavu-core` and `libfontconfig1`; verified `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`, `DejaVuSans-Bold.ttf`, and `libfontconfig.so.1`; all 4 binaries report `0.3.0` (`radivine 0.3.0`, etc.); `ldd` reports `0` missing libraries; unprivileged user exported 10 Dark/Light screenshots. |
| **3. Authentic Upgrade A (`v0.2.0 -> v0.3.0`)** | Install `radivine_0.2.0_amd64.deb`, populate `config.toml`, `actions.toml`, `convert.toml`, and `Job #1` (`src1.png -> src1.webp`) in `jobs.sqlite3`, then run `dpkg -i /debs/radivine_0.3.0_amd64.deb` and submit `Job #2` (`src2.png -> src2.jpg`) | **PASS** — `sha256sum -c` confirmed `config.toml`, `actions.toml`, and `convert.toml` remained **100% byte-for-byte identical**; `Job #1` (`v0.2.0`) and `Job #2` (`v0.3.0`) both persisted with `status='succeeded'` in `jobs.sqlite3`. |
| **4. Authentic Upgrade B (`v0.3.0~rc1 -> v0.3.0`)** | Install `radivine_0.3.0~rc1_amd64.deb`, then upgrade via `dpkg -i /debs/radivine_0.3.0_amd64.deb` and submit `Job #3` (`src3.png -> src3.webp`) | **PASS** — Upgraded cleanly to `Version: 0.3.0` (`radivine 0.3.0`); `sha256sum -c` confirmed all user TOML files byte-identical; `jobs.sqlite3` retained all `3` succeeded jobs (`v0.2.0`, `0.3.0~rc1`, `0.3.0`). |
| **5. Documented Rollback (`0.3.0 -> 0.2.0`)** | `dpkg -i --force-downgrade /debs/radivine_0.2.0_amd64.deb` | **PASS** — Cleanly downgraded to `radivine 0.2.0` with all user config and SQLite state intact. |
| **6. Clean Package Removal (`dpkg -r radivine`)** | `dpkg -r radivine` | **PASS** — Removed `/usr/bin/radivine*` cleanly while preserving `~/.config/radivine/{config,actions,convert}.toml` and `~/.local/state/radivine/jobs.sqlite3` (`3` jobs intact). |
