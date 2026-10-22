# Milestone M1a Test Report — Pure Radial Wheel Engine (`radivine-wheel`)

**Date:** 2026-10-22  
**Crate:** [`crates/radivine-wheel`](file:///home/marwanmohamed/Documents/work/rust/radivine/crates/radivine-wheel/src/lib.rs)  
**Toolchain:** `rustc 1.95.0` (`stable-x86_64-unknown-linux-gnu`), `edition = "2024"`, `resolver = "3"`  
**Status:** **PASS (All M1a Acceptance Criteria AC-1a.1 .. AC-1a.7 Verified)**

---

## 1. Acceptance Criteria Summary

| ID | Criterion | Verification Command / Test | Result |
|----|-----------|-----------------------------|--------|
| **AC-1a.1** | All 11 required scenario tests from `TESTING_STRATEGY.md` §3 pass | `cargo test -p radivine-wheel --test scenarios` (12/12 passed, including the normative walkthrough script) | **PASS** |
| **AC-1a.2** | Property test: for 10,000 random `(n, r_in, r_out, p_in_annulus)`, `hit_test` returns `{Item, Bridge, Empty}` | `cargo test -p radivine-wheel --test properties prop_annulus_partition` (10,000 cases) | **PASS** |
| **AC-1a.3** | Property test: for 5,000 random `(anchor, viewport >= 800x600, menu)`, placed wheels are inside viewport or flagged `fallback`, and non-parent wheels do not overlap | `cargo test -p radivine-wheel --test properties prop_placement_within_viewport_and_non_overlapping` (5,000 cases) | **PASS** |
| **AC-1a.4** | Property test: for 5,000 random input sequences of length `<= 50`, never panics, never commits disabled items, and `next_deadline() >= now` | `cargo test -p radivine-wheel --test properties prop_random_input_sequences_invariants` (5,000 cases) | **PASS** |
| **AC-1a.5** | Menu validation tests: `>12` items, depth `>6`, empty submenu, 0-item root handled without panic per `WHEEL_ENGINE_DESIGN.md` §3 | `cargo test -p radivine-wheel --test unit_tests test_menu_sanitization_overflow_depth_and_empty_pruning` | **PASS** |
| **AC-1a.6** | Zero heap allocations inside `handle`, `tick`, `scene` after session creation, verified by a counting global allocator | `cargo test -p radivine-wheel --test alloc_test` (`total_allocs == 0`) | **PASS** |
| **AC-1a.7** | `cargo tree -p radivine-wheel -e normal` shows only `smallvec`; `#![forbid(unsafe_code)]` enforced on library | `cargo tree -p radivine-wheel -e normal` → `smallvec v1.16.3` only | **PASS** |

---

## 2. Unit & Scenario Test Coverage

### 2.1 Unit Tests (`tests/unit_tests.rs`)
- `test_angle_normalization_and_diff`: Normalizes arbitrary angles (negative, `>= 2π`, `NaN`/`±∞`) into `[0, 2π)` and computes shortest signed differences in `(-π, π]`.
- `test_angle_in_span_wrap_around`: Verifies span membership and hysteresis widening across the `0 / 2π` wrap-around boundary.
- `test_root_segment_distribution_1_to_12`: Verifies item 0 centered at `3π/2` (top) and exact contiguous partition of `[0, 2π)` for all `n ∈ 1..=12` with zero boundary cracks or overlaps.
- `test_child_segment_distribution_all_quadrants`: Verifies 70° parent-facing bridge sector and `<= 72°` child item spans across all 8 cardinal/diagonal attachment directions for `n ∈ 1..=12`.
- `test_gap_insets_and_content_geometry`: Verifies constant linear gap insets (`δ(r) = (gap / 2) / r`) and segment label chord widths.
- `test_corridor_and_tangent_geometry`: Verifies convex hull navigation corridors (`child_corridor_polygon`, `anchor_corridor_polygon`), circle tangent points, and triangle inclusion.
- `test_hit_testing_hysteresis_and_submenu_overlap_guard`: Verifies deepest-first hit priority, `+3 lp` radial hub hysteresis, `±4°` angular segment hysteresis, and the **M0 S-1 Submenu Hit-Margin Overlap Guard** (`d > w.r_out` never preempts `d_parent <= parent.r_out`).
- `test_layout_root_and_child_edge_adaptation_and_caption`: Verifies root clamping, child rotation toward viewport interior near edges, proportional inner-radius scaling, stem Bézier control points, and caption placement (`Below` → `Above` → `Right` → `Left`).
- `test_tween_interpolation_and_retarget_continuity`: Verifies `Linear`, `OutCubic`, and `InOutCubic` easing and `C⁰`-continuous mid-flight retargeting.
- `test_menu_sanitization_overflow_depth_and_empty_pruning`: Verifies pruning of empty submenus, wrapping `> max_items_per_wheel` into `"More…"`, disabling submenus at `max_depth` with `"Too deeply nested"`, and clamping hard limits (`12` items, depth `6`).

### 2.2 Scenario Tests (`tests/scenarios.rs`)
- `test_normative_dsl_walkthrough_from_testing_strategy`
- `scenario_01_dwell_band_and_click_modes`
- `scenario_02_intent_triangle_diagonal_travel_and_grace_expiry`
- `scenario_03_hub_entry_closes_children_after_timeout`
- `scenario_04_outside_keeps_chain_and_reentry_restores_hover`
- `scenario_05_ancestor_item_switch_truncates_deeper_wheels`
- `scenario_06_activation_slop_blocks_hover_and_drop`
- `scenario_07_drop_target_matches_hover_disabled_and_default_child`
- `scenario_08_set_enabled_updates_drop_target_without_layout_change`
- `scenario_09_keyboard_navigation_complete`
- `scenario_10_dismiss_during_child_emerge_emits_finished_once`
- `scenario_11_placement_near_all_corners_2_and_3_levels`
