# Radivine v0.2.0 — Conversion Engine & Background Jobs (Planning Set)

Status: **Architecture Approved (`ADR-027`–`ADR-044`, `D1`–`D9`) — Milestone C1 Authorized ONLY** · Prepared: 2026-10-24 · Baseline: `v0.1.1` (`872d5d4`)

> [!IMPORTANT]
> Per `AGENTS.md` §3, each milestone (`C1`–`C5`) requires **explicit owner authorization**.
> The project owner has approved `ADR-C01`–`ADR-C18` (promoted to [`docs/DECISIONS.md`](../../DECISIONS.md) as `ADR-027`–`ADR-044`, subject to verification gates) and owner decisions `D1`–`D9`, and has explicitly authorized **Milestone C1 ONLY**. Milestones `C2`–`C5` remain unauthorized until explicitly approved by the owner.

## Labels used throughout

| Label | Meaning |
|-------|---------|
| **[EXISTING]** | Verified in the `v0.1.1` source tree (file/line references given) |
| **[APPROVED]** | Product decision already settled by the owner in the v0.2.0 brief |
| **[PROPOSED]** | Architectural decision made in this planning set, pending review |
| **[VERIFY]** | Open question that needs a proof-of-concept or measurement before it is relied on |
| **[TARGET]** | Engineering budget, not a measurement |

## Document inventory

| Document | Purpose |
|----------|---------|
| [CONVERSION_PRODUCT_SPEC.md](CONVERSION_PRODUCT_SPEC.md) | Feature scope, format matrix, Quick/Advanced, batch, presets, metadata, error behaviour |
| [CONVERSION_ARCHITECTURE.md](CONVERSION_ARCHITECTURE.md) | Codebase assessment, crate boundaries, dependency graph, Rust API sketches, integration with the Action System |
| [CONVERSION_BACKENDS.md](CONVERSION_BACKENDS.md) | Image library research, comparison, selected strategy, optional backends, licensing |
| [JOB_SYSTEM_DESIGN.md](JOB_SYSTEM_DESIGN.md) | Worker process model, state machine, persistence, IPC, progress, cancellation, recovery |
| [CONVERSION_UX.md](CONVERSION_UX.md) | Radial menu design, user flows 1–10, Advanced Convert window, Jobs page, settings |
| [SECURITY_AND_FILE_SAFETY.md](SECURITY_AND_FILE_SAFETY.md) | Threat model, output commit protocol, filesystem invariants, resource limits |
| [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) | Milestones C1–C5, acceptance criteria, testing, benchmarks, risks |
| [ARCHITECTURE_DECISIONS.md](ARCHITECTURE_DECISIONS.md) | Proposed ADRs C01–C18, alternatives, consequences, owner decisions |
| [INDEPENDENT_ARCHITECTURE_REVIEW.md](INDEPENDENT_ARCHITECTURE_REVIEW.md) | Source-grounded independent architecture & security review (`B1`–`B6`, `R1`–`R5`, `A1`–`A7`) |
| [IMPLEMENTATION_HANDOFF.md](IMPLEMENTATION_HANDOFF.md) | Consolidated implementation handoff guide for the implementing agent (once C1 is authorized) |

## One-paragraph summary

Conversion becomes a **built-in action family** (`Convert ▸ Image ▸ PNG | JPEG | WebP | Advanced…`) registered in the existing `ActionRegistry`. Visibility comes from file extensions in `BasicContext`. Enablement comes from sniffed MIME in `FileFacts`. The actions do not convert anything themselves: their `run()` method submits a **durable job** to a separate, on-demand process, `radivine-worker`. That process starts as its own systemd user unit (or as a detached process when systemd is not available), so restarting the wheel daemon or closing the Control Center does not stop running jobs. The worker persists jobs in SQLite, runs a reusable job scheduler, and does the actual conversion work through `radivine-convert` and `radivine-convert-image`:
- `radivine-convert` is a pure-Rust, desktop-agnostic library. It holds the format catalog, the backend contracts, option and preset validation, planning, and the safe no-overwrite output commit.
- `radivine-convert-image` is the built-in image backend. It decodes with the pure-Rust `image` crate and encodes lossy WebP with libwebp.

The Control Center gets a **Jobs** page and a **Conversion** settings page. **Advanced Convert** is a separate window of `radivine-control`.
