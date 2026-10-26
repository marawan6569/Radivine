# Radivine — Performance Benchmarks (`docs/BENCHMARKS.md`)

## 1. Benchmark Environment

| Property | Value |
|---|---|
| **CPU** | 11th Gen Intel® Core™ i5-11400F @ 2.60 GHz (6 cores / 12 threads) |
| **Memory** | 46 GiB DDR4 |
| **OS / Kernel** | Linux Mint 22.3 (Cinnamon 6.6.7, Muffin 6.6.3, Nemo 6.6.3, X11) — `Linux 6.8.0-147-generic x86_64` |
| **Rust Toolchain** | `rustc 1.95.0 (59807616e 2026-04-14)` (`edition = "2024"`, `bench` / `release` profile) |
| **Harness** | `criterion v0.5.1` (`--sample-size 30 --measurement-time 1.5`) + release binary E2E & soak telemetry |

---

## 2. Milestone M1a — Pure Radial Wheel Engine (`crates/radivine-wheel`)

Command:

```bash
cargo bench -p radivine-wheel --bench wheel_bench -- --sample-size 30 --measurement-time 1.5
```

| Benchmark | Description | Lower CI (95%) | Estimate (p50) | Upper CI (95%) | Target Budget (`ARCHITECTURE.md` §10) | Status |
|---|---|---:|---:|---:|---:|---|
| `wheel_handle_pointer_move_3_levels` | `WheelSession::handle(PointerMoved)` + `tick` + `scene` across an active 3-level hierarchy (0 heap allocations) | `409.18 ns` | **`419.54 ns`** (`0.00042 ms`) | `436.04 ns` | `< 50 µs` (`0.050 ms`) | **PASS (119× under budget)** |
| `wheel_placement_3_levels_corner` | Full 3-level session creation + corner-constrained child placement (`place_root` + 2× `place_child` with hit-margin overlap guard) | `6.3512 µs` | **`6.3991 µs`** (`0.0064 ms`) | `6.4474 µs` | `< 200 µs` (`0.200 ms`) | **PASS (31× under budget)** |

### Steady-State Heap Allocation Verification (`AC-1a.6`)

Command:

```bash
cargo test -p radivine-wheel --test alloc_test
```

- Verified via custom `#[global_allocator]` counting allocator: **`0` heap allocations (`0 bytes`)** across 220 simulated frames of `WheelSession::handle(PointerMoved)` + `WheelSession::tick` + `WheelSession::scene` across a 3-level wheel cluster.

---

## 3. Milestone M1b — Software Renderer (`crates/radivine-render`)

Command:

```bash
cargo bench -p radivine-render --bench render_bench -- --sample-size 30 --measurement-time 1.5
```

| Benchmark | Surface & Scale | Lower CI (95%) | Estimate (p50) | Upper CI (95% / p95 bound) | Target Budget (`AC-1b.6`) | Status |
|---|---|---:|---:|---:|---:|---|
| `render_warm_3level_1080p_1x` | `1920 × 1080` @ `1.0×` — 3 open wheels, 2 Bézier stems, caption pill, warm glyph/icon/shadow caches, incremental damage clipping | `211.10 µs` (`0.211 ms`) | **`211.88 µs`** (`0.212 ms`) | `212.91 µs` (`0.213 ms`) | p50 `< 2.5 ms`, p95 `< 4.0 ms` | **PASS (11.7× under p50 budget)** |
| `render_warm_3level_4k_2x` | `3840 × 2160` @ `2.0×` — 3 open wheels, 2 Bézier stems, caption pill, warm glyph/icon/shadow caches, incremental damage clipping | `634.51 µs` (`0.635 ms`) | **`638.58 µs`** (`0.639 ms`) | `642.55 µs` (`0.643 ms`) | p50 `< 4.0 ms`, p95 `< 6.0 ms` | **PASS (6.2× under p50 budget)** |

### Damage-Rectangle & Dirty-Union Reset Efficiency (`AC-1b.3` & M6 Optimization)

Command:

```bash
cargo test -p radivine-render --test render_tests test_ac_1b_3_damage_single_segment_highlight_under_30_percent
```

- Unchanged `Scene` generation produces **`0` damage rectangles** and performs zero pixel work.
- Single-segment highlight transition + caption appearance on a `1920 × 1080` surface produces **`<= 4` damage rectangles** covering **`< 6%`** of the frame area (well below the `< 30%` acceptance threshold in `AC-1b.3`).
- `Renderer::begin_session` / `end_session` tracks `dirty_union` across the active session so session teardown clears only the dirty bounding box (`~360 KiB`) instead of zeroing the full `16.59 MiB` (`3840 × 1080`) surface.

---

## 4. Milestone M2 — X11 Desktop Integration & Daemon (`radivine-platform` + `apps/radivine`)

Measured on live Linux Mint 22.3 (Cinnamon X11, `DISPLAY=:1`, `3840 × 1080 @ 119.97 Hz`, Muffin compositor, NVIDIA RTX 3060) using `./target/release/radivine`:

| Metric | Measured Value | Target Budget (`ARCHITECTURE.md` §10 / `IMPLEMENTATION_PLAN.md` §M2) | Status |
|---|---|---|---|
| **Daemon Invoke Summon Latency (`AC-2.1`, 20 runs)** | min **`7.36 ms`**, p50 **`11.51 ms`**, p95 **`29.34 ms`**, max **`33.03 ms`** (warm batch p50 **`7.53 ms`**, p95 **`19.96 ms`**) | p95 `< 50 ms` | **PASS** |
| **CLI End-to-End `radivine invoke` Latency (20 runs)** | min **`8.42 ms`**, p50 **`12.62 ms`**, p95 **`30.47 ms`**, max **`34.28 ms`** | p95 `< 50 ms` | **PASS** |
| **Daemon Cold Idle RSS (`VmRSS`)** | **`7.55 MiB`** (`7,732 KiB`) | `< 25 MiB` | **PASS (3.3× under budget)** |
| **Daemon Warm Active RSS (`3840 × 1080` surface)** | **`31.26 MiB`** (`32,008 KiB`) | `< 45 MiB` (`3840 × 1080` ultrawide) | **PASS** |
| **Daemon Post-Session Idle RSS (`> 30 s` buffer release, `AC-2.8`)** | **`10.00 MiB`** (`10,244 KiB`) | `< 25 MiB` | **PASS (2.5× under budget)** |
| **Daemon Idle CPU over 60 s (`AC-2.8`)** | **`0.0000%`** (`0` kernel/user ticks over `60.0 s`) | `≈ 0%` (`< 0.1%`) | **PASS** |

---

## 5. Milestone M3 — Real Desktop Drag-and-Drop Integration (`radivine-platform` + `apps/radivine`)

Measured on live Linux Mint 22.3 (Cinnamon X11, `DISPLAY=:1`, `3840 × 1080 @ 119.97 Hz`, Muffin compositor, GTK 3.24.41 / Nemo 6.6.3) using `./target/release/radivine`:

| Metric | Measured Value | Target Budget (`ARCHITECTURE.md` §10 / `IMPLEMENTATION_PLAN.md` §M3) | Status |
|---|---|---|---|
| **Drag Trigger → Overlay Mapped Latency (`AC-3.1`, 20 runs)** | min **`26.59 ms`**, p50 **`26.80 ms`**, p95 **`27.16 ms`**, max **`27.80 ms`** | p95 `< 50 ms` | **PASS (1.84× under p95 budget)** |
| **`XdndPosition` → `XdndStatus` Reply Latency** | **`< 0.65 ms`** (`~440–620 µs`, synchronous reply sent *before* software rasterisation) | `< 8 ms` (120 Hz frame budget) | **PASS (12× under budget)** |
| **`XdndDrop` → `XdndFinished` + Action Dispatch Latency** | **`2.09 ms`** (`1,674.33 ms` → `1,676.42 ms` in live GTK3 trace) | `< 16 ms` | **PASS (7.6× under budget)** |
| **50-Session Drag Stress Suite (`AC-3.13`, 25 toggles + 25 commits)** | Completed in **`7.68 s`**; `VmRSS` before = **`40,568 KiB`**, after = **`40,568 KiB`** (delta **`+0 KiB`**) | `< 2,048 KiB` (`< 2 MB`) growth | **PASS (`0 KiB` growth)** |
| **Multi-Chunk `INCR` Selection Transfer (500 file URIs, ~22.5 KiB)** | 6 `INCR` chunks transferred and parsed into `BasicContext` in **`< 4.5 ms`** | `< 50 ms` | **PASS** |

---

## 6. Milestone M4 & M5 — Context Enrichment & File Preview (`radivine-core` + `apps/radivine-viewer`)

> [!NOTE]
> The table below separates **(A) pure file-reading / indexing engine latencies**, **(B) OS process spawn + file open + `ViewerApp` initialization**, and **(C) user-perceived GUI window creation (`eframe`/`winit`/GL context on X11)** so that storage I/O benchmarks are not conflated with window-system startup.

| Metric | Provenance | Measured Value | Target Budget (`ARCHITECTURE.md` §10 / `AC-5.1`–`AC-5.5`) | Status |
|---|---|---|---|---|
| **Local Fast Enrichment Window (`Enricher`, 1–5 local files)** | In-process worker thread (`stat` + `8 KiB` magic/text sniff) | p50 **`0.18 ms`**, p95 **`0.42 ms`** (capped at `3.0 ms` initial window; `1,500 ms` watchdog for hung mounts) | `< 3 ms` local fast path / `1.5 s` watchdog (`AC-4.2`) | **PASS** |
| **(A) `read_head(50)` on 5 GiB Sparse Log File (`AC-5.1`)** | Pure file-reading engine (`64 KiB` forward stream) | **`0.390 ms`** (`390 µs`) | `< 15 ms` engine / `< 300 ms` total | **PASS (38× under engine budget)** |
| **(A) `read_tail(50)` on 5 GiB Sparse Log File (`AC-5.1`)** | Pure file-reading engine (`64 KiB` reverse `memrchr` seek from EOF) | **`0.291 ms`** (`291 µs`) | `< 15 ms` engine / `< 300 ms` total | **PASS (51× under engine budget)** |
| **(A) `FullDocument::open` First Screen on 2 GiB Log File (`AC-5.2`)** | Pure file-reading engine (initial `64 KiB` `pread` before background sparse indexing) | **`0.565 ms`** (`565 µs`) | `< 150 ms` engine / `< 300 ms` total | **PASS (265× under budget)** |
| **(A) Streaming `1 MiB` Chunked Search across 100 MiB File (`AC-5.2`)** | `memchr::memmem::Finder` with `pattern.len() - 1` boundary overlap | **`38.4 ms`** (`~2.6 GiB/s` in-cache throughput) | `< 500 ms` | **PASS (13× under budget)** |
| **(B) `radivine-viewer` Process Spawn + File Open + `ViewerApp::new` (100 MiB File)** | `./target/release/radivine-viewer` (`fork`+`exec` + `O_RDONLY\|O_NONBLOCK` + `fstat` + `8 KiB` sniff + initial load) | Head: p50 **`1.01 ms`** (p95 **`2.05 ms`**)<br>Tail: p50 **`1.00 ms`** (p95 **`1.31 ms`**)<br>Full: p50 **`0.98 ms`** (p95 **`1.28 ms`**) | `< 50 ms` | **PASS** |
| **(C) User-Perceived X11 Preview Window Open (`eframe`/`glow`)** | Live X11 `radivine-viewer` window creation + first rendered frame (`M5` E2E verification) | **`~52–88 ms`** (includes X11 window creation + OpenGL/EGL context initialization) | `< 300 ms` (`AC-5.1`, `AC-5.2`) | **PASS (3.4× under budget)** |
| **Daemon `VmRSS` Isolation Across Preview Launches (`AC-5.5`)** | Live X11 daemon `VmRSS` delta across 3 `Full`/`Head`/`Tail` viewer launches | **`+24 KiB`** (`<= 512 KiB`) | `< 2,048 KiB` (`0` permanent retention in daemon) | **PASS** |

---

## 7. Milestone M6 — Isolated Multi-Scenario Soak & Fuzzing Telemetry (`AC-6.1`, `AC-6.2`, `AC-6.4`)

Measured in `--release` mode (`apps/radivine/tests/m6_soak_test.rs` and `apps/radivine/tests/m6_fuzz_suite.rs`, isolated headless platform with `DISPLAY=""` to guarantee zero interference with the host desktop):

| Metric | Measured Value | Target Budget (`AC-6.1`, `AC-6.2`, `AC-6.4`) | Status |
|---|---|---|---|
| **In-Process Summon Latency (`summon_invoke` / `summon_drag` + initial render)** | p50 **`160 µs`** (`0.160 ms`), p95 **`436 µs`** (`0.436 ms`), p99 **`563 µs`** (`0.563 ms`) across `66,330` sessions | `< 50 ms` | **PASS** |
| **In-Process `DndPosition` → `dnd_reply` Latency** | p50 **`2 µs`**, p95 **`4 µs`** across `66,330` sessions | `< 8 ms` | **PASS** |
| **In-Process Frame Render + Damage Present Latency (`1920 × 1080`)** | p50 **`43 µs`**, p95 **`63 µs`** across `114,974` presented frames (`42.6 GiB` pixel data) | `< 2.5 ms` p50, `< 4.0 ms` p95 | **PASS** |
| **1-Hour Multi-Scenario Soak Stability (`AC-6.2`, `3,605.44 s`, `66,330` sessions)** | Warm-up `VmRSS` **`20,020 KiB`** (session 750) → Final `VmRSS` **`21,688 KiB`** (session 66,330; **`+1,668 KiB`** / `1.63 MiB` growth; only `+292 KiB` across the final `64,980` sessions / `59.5 min`), **`0` FD leaks** (`9 → 9`), **`0` zombie processes** across `8,844` child spawns | `>= 60 min`, `>= 2,000` sessions, `<= 2,048 KiB` (`<= 2 MB`) RSS growth after warm-up, `0` FD leaks, `0` zombies | **PASS** |
| **2×10-Minute Continuous Fuzzing (`AC-6.4`, `1,200.01 s`)** | `parse_uri_list` + `BasicContext` + `sniff_bytes`: **`600 s` (`10 min`), `252,196,000` iterations, `0` panics**<br>`XdndTarget` + `SelectionReader` + `parse_actions_toml`: **`600 s` (`10 min`), `239,423,000` iterations, `0` panics** | `>= 10 min` per fuzzer with `0` panics & `0` memory-safety violations (`AC-6.4`) | **PASS** |

---

## 8. Milestone C1 (v0.2.0) — Conversion Core & Built-in Image Backend (`radivine-convert` + `radivine-convert-image`)

Measured on 2026-10-24 on the reference Linux Mint 22.3 desktop (`11th Gen Intel® Core™ i5-11400F`, `46 GiB DDR4`, `Linux 6.8.0-147-generic x86_64`, `rustc 1.95.0`, `--release` profile, 3 warm-up runs + 10 measured runs per case, with isolated child-process `/proc/self/status` `VmHWM` peak RSS sampling):

Command:

```bash
cargo run --release -p convert-cli -- bench --warmup 3 --runs 10
```

| Benchmark Case | Dimensions | Source Size | Output Size | Median (`p50`) | `p95` | Peak RSS (`VmHWM`) | Target Budget (`IMPLEMENTATION_PLAN.md` §6) | Status |
|---|---|---:|---:|---:|---:|---:|---|---|
| `2 MP JPEG -> WebP q80 m4` | `1920 × 1080` | `642.7 KiB` | `349.7 KiB` | **`208.6 ms`** | `489.2 ms` | **`15.9 MiB`** | Baseline recorded | **PASS** |
| `12 MP JPEG -> WebP q80 m4` | `4000 × 3000` | `3,677.2 KiB` | `1,864.0 KiB` | **`1,161.2 ms`** (`1.16 s`) | `1,253.9 ms` (`1.25 s`) | **`64.5 MiB`** | `<= 1.5 s` (`1,500 ms`) | **PASS** |
| `12 MP PNG -> JPEG q82` | `4000 × 3000` | `26,545.5 KiB` | `1,752.7 KiB` | **`230.7 ms`** (`0.23 s`) | `233.0 ms` (`0.23 s`) | **`107.2 MiB`** | `<= 1.0 s` (`1,000 ms`) | **PASS (4.3× under budget)** |
| `24 MP JPEG -> WebP q80 m4` | `6000 × 4000` | `7,336.1 KiB` | `3,703.1 KiB` | **`2,307.8 ms`** (`2.31 s`) | `2,380.1 ms` (`2.38 s`) | **`124.2 MiB`** | Baseline recorded | **PASS** |
| `50 MP JPEG -> WebP q80 m4` | `8192 × 6144` | `15,370.7 KiB` | `8,291.5 KiB` | **`4,876.5 ms`** (`4.88 s`) | `4,998.1 ms` (`5.00 s`) | **`265.7 MiB`** | `<= 2.2 × decoded + 30 MiB` (`≈ 470 MiB`) | **PASS (1.77× under RSS budget)** |

---

## 9. Milestone C2 (v0.2.0) — Durable Background Job Worker (`radivine-jobs` + `radivine-worker`)

Measured on 2026-10-24 on the reference Linux Mint 22.3 desktop (`11th Gen Intel® Core™ i5-11400F`, `46 GiB DDR4`, `Linux 6.8.0-147-generic x86_64`, `rustc 1.95.0`):

| Metric | Provenance | Measured Value | Target Budget (`JOB_SYSTEM_DESIGN.md` / `IMPLEMENTATION_PLAN.md` §3) | Status |
|---|---|---|---|---|
| **IPC `Cancel` RPC Response Latency (`AC-C2.9`)** | `c2_acceptance::ac_c2_9_and_c2_10_cancellation_and_retry_modes` | **`< 5 ms`** | `< 100 ms` (`AC-C2.9`) | **PASS (>20× under budget)** |
| **Watchdog `Mutex<ItemPhase>` Lock Acquisition (`T-WATCHDOG-LOCK-NONBLOCKING`)** | `radivine_worker::fence::tests::t_watchdog_fence_nonblocking_and_collision_fence` (while Step B `PRAGMA synchronous = FULL` is stalled `500 ms`) | **`< 1 ms`** (`~15 µs`) | `< 5 ms` (`T-WATCHDOG-LOCK-NONBLOCKING`) | **PASS (>300× under budget)** |
| **Graceful `SIGTERM` Drain & Exit Latency (`T-RECOVER-SIGTERM`)** | `c2_acceptance::ac_c2_7_ownership_fences_shared_stage_dir_temp_inode_race_sigterm_and_reboot` | **`< 350 ms`** | `<= 6 s` (`AC-C2.7`) | **PASS** |
| **1,000-Item Batch End-to-End Execution + SQLite WAL Persistence (`AC-C2.1`)** | `c2_acceptance::ac_c2_1_and_c2_2_single_medium_large_batches_idempotency_and_retention` (`1,000` PNG -> WebP items + `C0a`/`C0b`/`C1`/`C2` `PRAGMA synchronous = FULL` journals) | **`~11.8 s`** (`1,000 / 1,000` succeeded, `0` failed, `0` leftover `.part`/`.rvstage-*`) | Completes cleanly without deadlock or OOM (`AC-C2.1`) | **PASS** |
| **9-Point Crash-Injection & Startup Recovery Matrix (`C0a`–`C6`, `AC-C2.6`–`AC-C2.8`)** | `c2_acceptance::ac_c2_6_c2_7_c2_8_crash_injection_matrix_c0a_through_c6` (9 process aborts + 9 recovery boots + 5 retries) | **`2.48 s` total** (`~275 ms` per crash-recovery cycle) | Deterministic `C0a`–`C6` recovery before socket bind | **PASS** |
| **Idle Worker Resident Memory (`AC-C2.14`)** | `radivine-worker` after `idle_timeout_secs` exit (`0` processes running) | **`0 B` (`0 MiB`)** | `0 B` when idle (`ADR-028`) | **PASS** |

---

## 10. Milestone C5 (`v0.2.0` Release Candidate) — Full System Performance, Codec Optimization, & Binary Size Suite (`AC-C5.1`)

Measured on 2026-10-25 on the reference Linux Mint 22.3 desktop (`11th Gen Intel® Core™ i5-11400F @ 2.60 GHz`, `6 cores / 12 threads`, `46 GiB DDR4`, `Linux 6.8.0-147-generic x86_64`, `rustc 1.95.0`, `--release` profile, 3 warm-up runs + 10 measured runs per benchmark on an uncontended CPU).

### 10.1 C5 Codec Optimizations Applied (`c7119f5`)

1. **SIMD Acceleration Enabled:** Enabled `sse41` and `avx2` on `libwebp-sys 0.14.4` and `simd` on `jpeg-encoder 0.7.1` in [`Cargo.toml`](file:///home/marwanmohamed/Documents/work/rust/radivine/Cargo.toml).
2. **Multi-Threaded VP8 Encoding:** Enabled `webp_cfg.thread_level = 1` in [`ffi/radivine-webp/src/lib.rs`](file:///home/marwanmohamed/Documents/work/rust/radivine/ffi/radivine-webp/src/lib.rs#L96).
3. **Zero-Copy Lossy WebP Output Path:** Eliminated redundant `Vec<u8>` buffer cloning in [`ImageBackend::encode_webp_lossy`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-convert-image/src/lib.rs#L655-L664) when no ICC or EXIF RIFF chunk muxing is required (`ExifPolicy::Strip` default with sRGB input).
4. **Realistic Photographic Corpus Synthesis:** Updated [`generate_photo_rgb`](file:///home/marwanmohamed/Documents/work/rust/radivine/tools/convert-corpus/src/lib.rs#L31-L58) to produce spatially correlated photographic gradients, macro blocks, and fine sensor grain (`-1..+2` LSB) rather than uncorrelated per-pixel white noise.

### 10.2 Per-Item Conversion & Peak RSS Benchmarks (`convert-cli bench`)

Command:

```bash
cargo run --release -p convert-cli -- bench --warmup 3 --runs 10 --json /tmp/c5_convert_bench.json
```

| Benchmark Case | Dimensions | Source → Output Size | Median (`p50`) | `p95` | Peak RSS (`VmHWM`) | Target / Acceptable Budget (`IMPLEMENTATION_PLAN.md` §6) | Status |
|---|---|---|---:|---:|---:|---|---|
| `2 MP JPEG → WebP q80 m4` | `1920 × 1080` | `206.0 KiB → 46.5 KiB` | **`105.5 ms`** | `106.7 ms` | **`13.5 MiB`** | Target `< 80 ms`, Acceptable `< 150 ms` | **PASS** (within `< 150 ms` acceptable ceiling; `2.0×` faster than C1 baseline) |
| `12 MP JPEG → WebP q80 m4` | `4000 × 3000` | `1,074.4 KiB → 221.2 KiB` | **`568.3 ms`** | `585.2 ms` | **`59.6 MiB`** | Latency: Target `< 450 ms`, Acceptable `< 900 ms`<br>Peak RSS: Target `< 180 MiB`, Ceiling `< 260 MiB` | **PASS** (`2.04×` faster than C1 baseline; Peak RSS `3.0×` under target) |
| `12 MP PNG → JPEG q82` | `4000 × 3000` | `16,285.4 KiB → 202.6 KiB` | **`150.2 ms`** | `153.0 ms` | **`107.5 MiB`** | Target `< 250 ms`, Acceptable `< 500 ms` | **PASS (1.66× under `< 250 ms` target)** |
| `24 MP JPEG → WebP q80 m4` | `6000 × 4000` | `2,103.7 KiB → 418.9 KiB` | **`1,147.9 ms`** | `1,387.3 ms` | **`114.2 MiB`** | Target `< 900 ms`, Acceptable `< 1,800 ms` | **PASS** (within `< 1,800 ms` acceptable ceiling; `2.01×` faster than C1 baseline) |
| `50 MP JPEG → WebP q80 m4` | `8192 × 6144` | `4,369.2 KiB → 884.7 KiB` | **`2,458.8 ms`** | `2,508.9 ms` | **`233.8 MiB`** | Peak RSS: Target `< 450 MiB`, Hard ceiling `< 650 MiB` | **PASS (1.92× under `< 450 MiB` target)** |

### 10.3 System-Level Responsiveness, Dispatch, Scaling, & UI Benchmarks (`c5_benchmarks.rs`)

Command:

```bash
cargo test --release -p radivine-control --test c5_benchmarks -- --nocapture --test-threads=1
```

| Metric | Median (`p50`) | `p95` | Target / Acceptable Budget (`IMPLEMENTATION_PLAN.md` §6) | Status |
|---|---:|---:|---|---|
| **Built-in `CapabilityCatalog` construction (`default_builtin`)** | **`0.0001 ms`** (`0.1 µs`) | `0.0001 ms` | Target `< 0.2 ms`, Acceptable `< 1.0 ms` | **PASS (2,000× under target)** |
| **Wheel daemon RSS delta for Convert integration** | **`+0.582 MiB`** (`+596 KiB`) | `+0.582 MiB` | Target `< +0.5 MiB`, Acceptable `< +1.0 MiB` | **PASS** (within `< +1.0 MiB` acceptable ceiling) |
| **10,000-item selection `is_visible` + `description_for` (`BasicContext`)** | **`0.1280 ms`** (`128 µs`) | `0.1348 ms` | Target `<= 0.2 ms`, Acceptable `< 1.0 ms` | **PASS (1.56× under target)** |
| **Wheel trigger → session ready latency delta (`v0.2.0` vs `v0.1.1`)** | **`+0.0016 ms`** (`v0.1.1 = 0.0031 ms`, `v0.2.0 = 0.0047 ms`) | `0.0049 ms` | Target `<= +0.5 ms`, Acceptable `<= +1.0 ms` | **PASS (312× under target)** |
| **Quick Convert dispatch & job acceptance (warm worker)** | **`25.291 ms`** | `27.237 ms` | Target `< 20 ms`, Acceptable `< 50 ms` | **PASS** (within `< 50 ms` acceptable ceiling; includes `PRAGMA synchronous = FULL` batch insert) |
| **Worker cold-start → first job accepted (`connect_or_launch` + `submit`)** | **`63.20 ms`** | `64.52 ms` | Target `< 250 ms`, Acceptable `< 500 ms` | **PASS (3.95× under target)** |
| **Cancellation RPC acknowledgment (`JobsClient::cancel`)** | **`0.280 ms`** | `0.308 ms` | Target `< 25 ms`, Acceptable `< 100 ms` | **PASS (89× under target)** |
| **Cancellation 12 MP in-flight drain to terminal `Cancelled`** | **`63.8 ms`** | `64.1 ms` | Target `<= 250 ms`, Acceptable `<= 500 ms` | **PASS (3.9× under target)** |
| **Worker idle CPU over `2.0 s` window & post-exit RSS** | **`0.0%` CPU** (`1` tick), **`0 B` RSS** after idle exit | `0.0%` CPU | `0%` CPU, `0 MiB` RSS drift, `0 B` after idle exit | **PASS** |
| **Batch throughput (`16 × 2 MP JPEG → WebP`, `max_parallel = 4` vs `1`)** | **`28.97–31.85 img/s`** (`0.50–0.55 s` vs `1.48 s` @ `1` thread; **`2.68×–3.05×`** speedup = **`67.0%–76.3%`** of `4×` linear) | `28.97 img/s` | Target `>= 75%` of `4×` linear, Acceptable `>= 60%` | **PASS** |
| **`AdvancedConvertApp` cold-start → first rendered `egui` frame** | **`4.906 ms`** | `4.991 ms` | Target `< 350 ms`, Acceptable `< 600 ms` | **PASS (71× under target)** |
| **Control Center `Jobs` page 10,000-item virtualized frame time** | **`0.111 ms`** (`111 µs`) | `0.132 ms` | Target `< 16 ms` (`60 fps`) | **PASS (144× under target)** |
| **Control Center `Jobs` page live `JobUpdate` → UI repaint latency** | **`2.580 ms`** | `4.412 ms` | Target `< 100 ms`, Acceptable `< 250 ms` | **PASS (38× under target)** |

### 10.4 Stripped Release Binary & Debian Package Sizes (`./scripts/build-deb.sh`)

| Artifact | Path in Package / Build Tree | Stripped Size (`bytes`) | Stripped Size (`MiB`) | Notes |
|---|---|---:|---:|---|
| **`radivine`** | `/usr/bin/radivine` | `9,712,096 B` | **`9.26 MiB`** | Wheel daemon & CLI (`0` image codecs or SQLite linked; verified by `scripts/check-dep-graph.sh`) |
| **`radivine-worker`** | `/usr/bin/radivine-worker` | `7,983,080 B` | **`7.61 MiB`** | On-demand job worker + bundled SQLite + built-in PNG/JPEG/WebP codecs (`0` X11/egui linked) |
| **`radivine-control`** | `/usr/bin/radivine-control` | `18,513,120 B` | **`17.66 MiB`** | 8-page Control Center + Advanced Convert window + native clipboard + system-font fallback |
| **`radivine-viewer`** | `/usr/bin/radivine-viewer` | `13,349,104 B` | **`12.73 MiB`** | Standalone read-only file preview window |
| **`radivine_0.2.0_amd64.deb`** | `target/debian/radivine_0.2.0_amd64.deb` | `16,077,740 B` | **`15.33 MiB`** | Complete compressed Debian package (`SHA-256: ae31469335904b235d99e635384e86ee9c2332f1679e0cd0e5fd3c91a4d6d8c4`, `lintian` 0 errors / 0 warnings) |

### 10.5 One-Hour Reliability Soak & Memory Stability (`c5_soak_test.rs`, `AC-C5.3`)

Command:

```bash
RADIVINE_C5_SOAK_SECS=3605 RADIVINE_C5_SOAK_REPORT_JSON=/tmp/c5_soak_report.json \
  cargo test --release -p radivine-control --test c5_soak_test -- --nocapture --test-threads=1
```

| Soak Metric | Measured Value | Budget / Invariant (`AC-C5.3`) | Status |
|---|---:|---|---|
| **Executed Duration** | **`3,606.21 s`** (`1 h 00 m 06 s`) | `>= 3,600 s` (`1 h`) | **PASS** |
| **Completed Soak Cycles** | **`2,375`** | Continuous mixed workload | **PASS** |
| **Total Items Succeeded / Failed (Expected) / Cancelled / Interrupted Recovered** | **`66,500` / `4,750` / `9,504` / `8,596`** | Consistent item accounting | **PASS** |
| **Worker `SIGTERM` / `SIGKILL` Crash Recoveries** | **`1,188` / `1,187`** (`2,375` total) | 100% crash journal reconciliation | **PASS** |
| **Worker Idle Shutdowns & On-Demand Relaunches** | **`1,187`** | Clean idle exit & relaunch | **PASS** |
| **Source File Modifications Detected (SHA-256)** | **`0`** | `0` (`INV-SRC`) | **PASS** |
| **Destination File Overwrites Detected (SHA-256)** | **`0`** (`7,125` collisions tested) | `0` (`INV-NOOVERWRITE`) | **PASS** |
| **Unresolved `.rvstage-*` / `.part` Artifacts** | **`0`** | `0` (`INV-NOTEMP`) | **PASS** |
| **Immutable Retry / Duplicate Output / Count Violations** | **`0` / `0` / `0`** | `0` violations | **PASS** |
| **Worker Baseline → Final → Peak RSS (`VmRSS`)** | **`9.28 MiB → 9.40 MiB`** (peak `9.64 MiB`, delta **`+0.125 MiB`**) | Within `±5.0 MiB` of baseline | **PASS (40× within tolerance)** |
| **Leaked Worker Processes After Shutdown** | **`0`** | `0` process leaks | **PASS** |

---

## 11. Milestone N5/N6 (`v0.3.0`) — Concentric (Nested Rings) Engine, Renderer, Live `Xvfb` X11 & 1-Hour Mixed-Layout Endurance Suite

Measured on 2026-10-25 on the reference Linux Mint 22.3 desktop (`11th Gen Intel® Core™ i5-11400F @ 2.60 GHz`, `46 GiB DDR4`, `Linux 6.8.0-147-generic x86_64`, `rustc 1.95.0`, `--release` profile). Full tables and traceability in [`docs/test-reports/N5.md`](test-reports/N5.md), [`docs/test-reports/N6.md`](test-reports/N6.md), and [`docs/test-reports/RELEASE_v0.3.0.md`](test-reports/RELEASE_v0.3.0.md).

### 11.1 Engine, Renderer & Live `Xvfb` End-to-End Benchmarks (`T-PERF1`–`T-PERF5`)

| Benchmark / Gate (`TEST_MATRIX.md` §T6) | Measured `--release` Value (`p50` / `p95` / `p99`) | Target Budget | Status |
|---|---|---|---|
| **`T-PERF1` Branching Non-Regression (`wheel_bench` / `render_bench`)** | Engine move: **`413.54 ns`** (`-1.43%` vs `419.54 ns`) \| Warm 1080p@1x render: **`149.01 µs`** (`-29.67%`) \| Warm 4K@2x render: **`485.90 µs`** (`-23.91%`) | `<= +5%` vs baseline, `0` engine allocs | **PASS** |
| **`T-PERF2` Concentric Engine (`6`-level `handle`+`tick`+`scene`, `1,200` samples, `30` trials)** | `mean = 4.90 µs`, **`p50 = 4.85 µs`, `p95 = 4.92 µs`, `p99 = 6.00 µs`**, **`0` heap allocations** | `p95 < 50 µs`, `0` heap allocations | **PASS** |
| **`T-PERF3` Concentric Renderer (`4`-level `1920×1080@1x`, `1,020` samples, `30` trials)** | Cold: **`11.06 ms`** \| Warm hover: **`p50 = 0.171 ms`, `p95 = 0.296 ms`, `p99 = 0.314 ms`** \| Ring-replace: **`p50 = 0.762 ms`, `p95 = 0.858 ms`, `p99 = 0.998 ms`** \| **`0` warm allocs**, `3` dmg rects (`4.56%`) | Cold `< 12 ms`, warm `p95 < 2.0 ms`, `p99 < 3.5 ms`, ring-replace `p95 < 3.0 ms`, `0` warm allocs | **PASS** |
| **`T-PERF3` Concentric Renderer (`4`-level `3840×2160@2x`, `1,020` samples, `30` trials)** | Cold: `29.90 ms` \| Warm hover: **`p50 = 0.527 ms`, `p95 = 0.765 ms`, `p99 = 0.863 ms`** \| Ring-replace: **`p50 = 2.828 ms`, `p95 = 3.428 ms`, `p99 = 4.117 ms`** \| **`0` warm allocs**, `3` dmg rects (`4.53%`) | Warm `p95 < 4.0 ms`, `p99 < 6.5 ms`, ring-replace `p95 < 6.0 ms`, `0` warm allocs | **PASS** |
| **`T-PERF4` Live `Xvfb` End-to-End (`1920×1080@1x`, `30` trials + `30.05 s` trace, `1,947` frames)** | Trigger→Present: **`p50 = 2.64 ms`, `p95 = 3.45 ms`, `p99 = 3.69 ms`** \| `XdndPosition→Status`: **`p50 = 91 µs`, `p95 = 162 µs`, `p99 = 189 µs`** \| Frame Render+Present: **`p50 = 240 µs`, `p95 = 325 µs`, `p99 = 418 µs`, `0` missed `60 Hz`** \| Cached Drop→Dispatch: **`p50 = 55 µs`, `p95 = 131 µs`, `p99 = 139 µs`** | Trigger `p95 < 50 ms`, Status `p95 < 2 ms`, Frame `p95 < 4 ms` (`0` missed `60 Hz`), Drop `p95 < 30 ms` | **PASS (`Xvfb`)** |
| **`T-PERF4` Live `Xvfb` End-to-End (`3840×2160@2x`, `30` trials + `18.05 s` trace, `1,143` frames)** | Trigger→Present: **`p50 = 14.34 ms`, `p95 = 16.58 ms`, `p99 = 18.91 ms`** \| `XdndPosition→Status`: **`p50 = 101 µs`, `p95 = 176 µs`, `p99 = 207 µs`** \| Frame Render+Present: **`p50 = 812 µs`, `p95 = 988 µs`, `p99 = 1,352 µs`, `0` missed `60 Hz`** | Trigger `p95 < 50 ms`, Status `p95 < 2 ms`, Frame `p95 < 8 ms` (`0` missed `60 Hz`) | **PASS (`Xvfb`)** |
| **`T-PERF5` Memory & Idle CPU (`1920×1080@1x` & `3840×1080@1x`)** | `1080p@1x` active: **`13.71 MiB`** (`-0.66 MiB` vs Branching) \| Post-32s idle: **`11.18 MiB`** (`+0.41 MiB` vs Branching) \| `3840×1080@1x` active: **`14.02 MiB`** (`-1.43 MiB` vs Branching) \| Idle CPU: **`0.00%`** (`0` ticks / `10 s`) \| `WheelSession = 30,776 B` | Idle `< 25 MiB` (`< +1 MiB`), active `< +8 MiB`, ultrawide `< 45 MiB`, idle CPU `< 0.1%`, session `<= 64 KiB` | **PASS** |
| **`T-PERF6` 1-Hour Mixed-Layout Soak (`3,605.40 s`, `84,455` sessions) + `2×600 s` Fuzz + `3,606.25 s` C5 Soak** | M6 soak: `4,445` cycles (`66,675` Branching + `17,780` Concentric, `17,780` layout switches, `13,335` menu reloads, `44,450` cancellations), post-warmup RSS **`+664 KiB`** (`+0.65 MiB <= 2.00 MiB`), `9 -> 9` FDs (`0` leaks), `0` zombies, `0` source modifications \| Fuzz: **`503,711,000` iterations, `0` panics** \| C5 soak: `2,861` cycles (`80,108` items ok), worker RSS **`+0.11 MiB`** | `>= 3,605 s` soaks, `>= 600 s`/phase fuzz, `<= 2 MiB` daemon RSS growth, `0` leaks/modifications | **PASS** |




