# Radivine — Performance Benchmarks (`docs/BENCHMARKS.md`)

## 1. Benchmark Environment

| Property | Value |
|---|---|
| **CPU** | 11th Gen Intel® Core™ i5-11400F @ 2.60 GHz (6 cores / 12 threads) |
| **Memory** | 46 GiB DDR4 |
| **OS / Kernel** | Linux Mint 22.x (Cinnamon, X11) — `Linux 6.8.0-147-generic x86_64` |
| **Rust Toolchain** | `rustc 1.95.0 (59807616e 2026-04-14)` (`edition = "2024"`, `bench` / `release` profile) |
| **Harness** | `criterion v0.5.1` (`--sample-size 30 --measurement-time 1.5`) |

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

### Damage-Rectangle Efficiency (`AC-1b.3`)

Command:

```bash
cargo test -p radivine-render --test render_tests test_ac_1b_3_damage_single_segment_highlight_under_30_percent
```

- Unchanged `Scene` generation produces **`0` damage rectangles** and performs zero pixel work.
- Single-segment highlight transition + caption appearance on a `1920 × 1080` surface produces **`<= 4` damage rectangles** covering **`< 6%`** of the frame area (well below the `< 30%` acceptance threshold in `AC-1b.3`).

---

## 4. Milestone M2 — X11 Desktop Integration & Daemon (`radivine-platform` + `apps/radivine`)

Measured on live Linux Mint 22.x (Cinnamon X11, `DISPLAY=:1`, `3840 × 1080 @ 119.97 Hz`, Muffin compositor, NVIDIA RTX 3060) using `./target/release/radivine`:

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

Measured on live Linux Mint 22.3 (Cinnamon X11, `DISPLAY=:1`, `3840 × 1080 @ 119.97 Hz`, Muffin compositor, GTK 3.24.41 / Nemo 6.6.3) using `./target/release/radivine` (`cargo test --release -p radivine --test m3_drag_acceptance -- --test-threads=1 --nocapture`):

| Metric | Measured Value | Target Budget (`ARCHITECTURE.md` §10 / `IMPLEMENTATION_PLAN.md` §M3) | Status |
|---|---|---|---|
| **Drag Trigger → Overlay Mapped Latency (`AC-3.1`, 20 runs)** | min **`26.59 ms`**, p50 **`26.80 ms`**, p95 **`27.16 ms`**, max **`27.80 ms`** | p95 `< 50 ms` | **PASS (1.84× under p95 budget)** |
| **`XdndPosition` → `XdndStatus` Reply Latency** | **`< 0.65 ms`** (`~440–620 µs`, synchronous reply sent *before* software rasterisation) | `< 8 ms` (120 Hz frame budget) | **PASS (12× under budget)** |
| **`XdndDrop` → `XdndFinished` + `LogAction` Dispatch Latency** | **`2.09 ms`** (`1,674.33 ms` → `1,676.42 ms` in live GTK3 trace) | `< 16 ms` | **PASS (7.6× under budget)** |
| **50-Session Drag Stress Suite (`AC-3.13`, 25 toggles + 25 commits)** | Completed in **`7.68 s`**; `VmRSS` before = **`40,568 KiB`**, after = **`40,568 KiB`** (delta **`+0 KiB`**) | `< 2,048 KiB` (`< 2 MB`) growth | **PASS (`0 KiB` growth)** |
| **Multi-Chunk `INCR` Selection Transfer (500 file URIs, ~22.5 KiB)** | 6 `INCR` chunks transferred and parsed into `BasicContext` in **`< 4.5 ms`** | `< 50 ms` | **PASS** |

