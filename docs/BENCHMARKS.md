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
