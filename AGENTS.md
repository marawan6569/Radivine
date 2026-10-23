# Radivine — AI Coding Agent Instructions (`AGENTS.md`)

This document defines mandatory engineering and Git workflow requirements for all AI coding agents working on the **Radivine** repository.

---

## 1. Mandatory Git Workflow

### 1.1 Preserve Existing Work Before Starting Any Milestone
Before starting a new milestone or task:
1. Inspect the current Git repository status (`git status`) and recent commit history (`git log --oneline`).
2. Verify whether previous milestones have already been committed.
3. If any completed work remains uncommitted, organize it into meaningful, logically separated commits wherever practical.
4. Preserve the existing Git history.
5. Do not overwrite, discard, or accidentally include unrelated user changes.
6. Do not create duplicate commits for work already committed.

### 1.2 Commit Incrementally
**You MUST create Git commits continuously throughout development, not only at the end of a milestone.**

Create a commit whenever a logically complete, independently understandable unit of work is finished and verified. Examples include:
- Implementing a core component or crate.
- Adding an important feature.
- Fixing a bug.
- Adding or improving tests.
- Introducing a platform adapter.
- Completing a documented architectural change.
- Updating milestone documentation or test reports.

Avoid accumulating large amounts of unrelated changes into a single commit.

### 1.3 Conventional Commits
Use the [Conventional Commits](https://www.conventionalcommits.org/) specification for all commit messages:
- `feat(wheel): implement hierarchical navigation`
- `feat(x11): add transparent overlay support`
- `feat(core): add context resolver and action registry`
- `fix(render): correct alpha blending`
- `refactor(core): simplify session state transitions`
- `test(x11): add overlay lifecycle tests`
- `docs: update architecture decisions`
- `perf(render): optimize damage rectangle updates`
- `chore: update workspace dependencies`

Use descriptive, precise commit messages. Never use generic messages such as `"updates"`, `"fix stuff"`, or `"progress"`.

### 1.4 Commit Quality Requirements
Before each commit:
1. Review the diff (`git diff`).
2. Stage only files relevant to that logical change.
3. Run appropriate formatting (`cargo fmt --all --check`), linting (`cargo clippy`), and tests (`cargo test`).
4. Fix failures introduced by the change.
5. Ensure no build artifacts (`target/`), secrets, temporary logs, or unrelated files are included.
6. Create the commit.
7. Continue with the next implementation task.

Prefer focused tests during development and full workspace verification at milestone completion. If a commit cannot be verified, report the limitation explicitly rather than presenting it as fully tested.

### 1.5 Git Safety
- Never use `git push --force`.
- Never rewrite or amend existing commits without explicit approval.
- Never use destructive Git operations (`git reset --hard`, `git checkout -- .`) to discard user changes.
- Never commit credentials, private keys, or secrets.
- Never commit `target/`, temporary binaries, or generated build artifacts.
- Do not push to a remote unless explicitly requested.
- Respect the repository's existing branch strategy.
- Do not introduce unrelated refactoring into feature commits.

### 1.6 Milestone Completion Checklist
At the end of each milestone:
1. Run the full required test suite (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`).
2. Commit the final documentation and test reports (`docs/test-reports/M*.md`).
3. Verify `git status`.
4. Report any intentionally uncommitted files.
5. Display the commits created during the milestone using `git log --oneline`.
6. Include a concise Git history summary in the milestone completion report.

---

## 2. Project Invariants & Asset Protection

1. **Branding & Icons Protection:** Never modify, delete, rename, or regenerate any existing files under `assets/branding/` or `assets/icons/`.
2. **Engine Purity (`radivine-wheel`):** `crates/radivine-wheel` must remain `#![forbid(unsafe_code)]`, pure Rust, platform-independent, renderer-independent, and depend only on `smallvec` at runtime.
3. **Renderer Safety (`radivine-render`):** `crates/radivine-render` must remain `#![forbid(unsafe_code)]` and platform-independent.
4. **Input Pass-Through Safety Default:** On X11 drag mode, `input.passthrough` defaults to `false` (capture-all overlay during active sessions) to prevent accidental native file drops/moves into underlying file manager windows when releasing outside the wheel (`ADR-0001`, `FEASIBILITY_STUDY.md` §10).

---

## 3. Mandatory Milestone Authorization Policy

### 3.1 Explicit Authorization Required
- Never begin a new implementation milestone without explicit authorization from the user.
- Git workflow instructions, documentation updates, and maintenance requests do not constitute authorization to proceed to another milestone.
- Completion of a milestone does not automatically authorize the next one.
- Do not infer permission from the existence of an implementation plan or previously prepared prompts.
- When instructed to stop after a milestone, stop completely after verification, documentation, and commits.
- Do not begin the next milestone even if all acceptance criteria pass.
- If authorization is unclear, stop and request it.

### 3.2 Scope Boundaries
- Treat each explicitly authorized milestone as a hard scope boundary.
- Do not implement functionality belonging to future milestones unless the user explicitly approves expanding the scope.

### 3.3 Current Project State
- Milestones M0, M1a, M1b, M2, M3, M4, M5, and M6 are complete (`v0.1.0` prepared).
- **Do not begin any milestone beyond M6** without explicit authorization from the user.
