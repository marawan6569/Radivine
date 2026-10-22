# Radivine — Implementation Handoff (for Gemini 4 Argon High)

Read this first, then the documents it points to. This handoff is binding: **do not change the architecture without recording the reason** (see §10).

Last updated: 2026-10-22 · Planning by: Principal Architect (Antigravity) · Implementation: not started.

---

## 1. What you are building

Radivine is a Rust daemon for Linux (X11 first) that shows a **hierarchical radial action wheel** at the cursor while the user drags a file from Nemo and presses `Shift`. Dropping the file on a wheel item runs a context-aware action. Child wheels open **beside** their parent (parent stays visible). **The wheel interaction is the product**; everything else is secondary.

Key documents (normative):
| Doc | Use it for |
|-----|-----------|
| [FEASIBILITY_STUDY.md](FEASIBILITY_STUDY.md) | Why/how the drag interaction works on X11; spikes S-1…S-7; evidence |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, dependency graph, threads, session state machine, lifecycle, perf targets |
| [WHEEL_ENGINE_DESIGN.md](WHEEL_ENGINE_DESIGN.md) | Geometry, placement, hit-testing, hover intent, animation, Scene, renderer spec, config |
| [PLATFORM_INTEGRATION.md](PLATFORM_INTEGRATION.md) | X11 backend details (overlay, SHM, XDND messages, arming, grabs), `Platform` trait, Wayland/Win/mac plans |
| [ACTION_SYSTEM.md](ACTION_SYSTEM.md) | Context model, `Action` trait, registry/menu building, dispatcher, built-ins, custom actions, viewer contract |
| [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) | Milestones M0–M6 with tasks and acceptance criteria |
| [TESTING_STRATEGY.md](TESTING_STRATEGY.md) | Test layers, scenario DSL, Xvfb/E2E, manual matrix, CI |
| [RISKS_AND_MITIGATIONS.md](RISKS_AND_MITIGATIONS.md) | Risk register |
| [DECISIONS.md](DECISIONS.md) | ADR-001…024 |

## 2. Selected architecture (one paragraph)

A single-process daemon (`apps/radivine`) runs a **calloop** loop on one UI thread. The **X11 backend** (`radivine-platform::x11`, built on **x11rb**) idles watching only XFixes ownership changes of `XdndSelection`. When another client starts a drag while a button is held, it arms and selects XI2 raw key events; a **fresh `Shift` press** triggers. The app then maps a **pre-created, monitor-sized, override-redirect ARGB overlay** with `XdndAware=5` under the pointer; Nemo's GTK retargets to it on the next motion, so Radivine becomes the **legitimate XDND drop target**, receiving `XdndPosition` coordinates (navigation input) and finally `XdndDrop` + `text/uri-list` via the selection. The **pure wheel engine** (`radivine-wheel`) turns positions + time into hover/open/commit decisions and a `Scene`; the **CPU renderer** (`radivine-render`: tiny-skia, cosmic-text, resvg) draws the Scene into a premultiplied RGBA pixmap, which the backend uploads through **MIT-SHM** with damage rectangles. **`radivine-core`** builds a context from the URIs, filters actions into a `MenuTree`, and dispatches the chosen action on a worker thread; **`radivine-actions`** provides built-ins. Previews open in a separate **egui** process (`radivine-viewer`). **Invoke mode** (`radivine invoke <uris>`, e.g. from a Nemo right-click action) shows the same wheel with click/keyboard selection — it is the M2 integration vehicle, the accessibility path and the cross-platform fallback.

## 3. Key technical decisions (do not revisit without an ADR)

1. Join the drag as an XDND target; never try to grab the pointer during a drag (ADR-001).
2. One monitor-sized overlay window renders all wheels; input limited by a SHAPE input mask (ADR-002, ADR-016).
3. CPU rendering (tiny-skia + cosmic-text + resvg) → MIT-SHM (`memfd` + `AttachFd`) (ADR-003).
4. Own x11rb backend; no GUI toolkit for the overlay (ADR-004).
5. Wheel engine is pure, deterministic, time-injected, `std` + `smallvec` only (ADR-006).
6. calloop single UI thread; I/O, D-Bus and actions on worker threads; no async runtime in the daemon (ADR-007).
7. Raw key events only while armed; fresh-press trigger; pre-held Shift ignored (ADR-008).
8. Always `XdndActionPrivate`; never request `DELETE` (ADR-009).
9. Child wheels adjacent along the segment bisector with a bridge sector; single chain; wheels never move (ADR-010).
10. Visibility from the I/O-free BasicContext; enablement may update live (ADR-012).

## 4. Repository structure (target)

```text
radivine/
├── Cargo.toml  deny.toml  .github/workflows/{ci,x11,nightly,release}.yml
├── crates/
│   ├── radivine-wheel/       # pure engine
│   ├── radivine-render/      # renderer
│   ├── radivine-core/        # context, Action API, registry, dispatcher
│   ├── radivine-actions/     # built-ins
│   └── radivine-platform/    # Platform trait + x11/
├── apps/
│   ├── radivine/             # daemon + CLI
│   └── radivine-viewer/      # egui viewer (M5)
├── tools/wheel-sandbox/      # winit + softbuffer dev tool (M1b)
├── tests/e2e-x11/            # Xvfb + GTK3 python drag source + xdotool
├── spikes/                   # M0 only, excluded from workspace
├── assets/
│   ├── branding/  icons/     # EXISTING — never modify, rename or regenerate
│   ├── fonts/  wheel-icons/  third_party/  desktop/   # NEW
└── docs/
```

Allowed crate edges: `render→wheel`, `core→wheel` (MenuTree types only), `actions→core`, `app→{wheel,render,core,actions,platform}`, `sandbox→{wheel,render}`. `platform` and `viewer` depend on no Radivine crate. CI checks this.

## 5. Important interfaces (signatures are normative)

```rust
// radivine-wheel
pub struct MenuTree { pub root: MenuNode }
pub struct MenuNode { pub id: Arc<str>, pub label: Arc<str>, pub icon: Option<IconKey>,
                      pub description: Option<Arc<str>>, pub disabled_reason: Option<Arc<str>>,
                      pub enabled: bool, pub kind: NodeKind }
pub enum NodeKind { Leaf, Submenu { children: Vec<MenuNode>, default_child: Option<usize> } }
pub enum WheelInput { PointerMoved(Point), PointerLeft, PointerEntered, Press, Release, Nav(NavKey) }
pub enum WheelEvent { HoverChanged(Option<ItemPath>), SubmenuOpened(ItemPath), SubmenuClosed(ItemPath),
                      Commit(ItemPath), CancelRequested, InputRegionChanged, Finished }
impl WheelSession {
    pub fn new(menu: Option<MenuTree>, anchor: Point, viewport: Viewport, mode: InputMode, cfg: &WheelConfig, now: Millis) -> Self;
    pub fn set_menu(&mut self, menu: MenuTree, now: Millis);
    pub fn handle(&mut self, input: WheelInput, now: Millis, out: &mut Vec<WheelEvent>);
    pub fn tick(&mut self, now: Millis, out: &mut Vec<WheelEvent>);
    pub fn drop_target(&self) -> DropTarget;
    pub fn commit_drop(&mut self, now: Millis) -> Option<ItemPath>;
    pub fn set_enabled(&mut self, path: &ItemPath, enabled: bool, disabled_reason: Option<Arc<str>>);
    pub fn dismiss(&mut self, reason: DismissReason, now: Millis);
    pub fn scene(&self) -> &Scene;
    pub fn input_region(&self) -> &[Shape];
    pub fn next_deadline(&self) -> Option<Millis>;
    pub fn is_animating(&self) -> bool;
    pub fn is_finished(&self) -> bool;
}

// radivine-render
impl Renderer {
    pub fn new(theme: Theme, assets: AssetSource) -> Result<Self, RenderError>;
    pub fn begin_session(&mut self, size_px: SizePx, scale: f32);
    pub fn render(&mut self, scene: &Scene) -> &[RectPx];
    pub fn frame(&self) -> FrameRef<'_>;
    pub fn end_session(&mut self);
}

// radivine-core
pub trait Action: Send + Sync + 'static {
    fn meta(&self) -> &ActionMeta;
    fn is_visible(&self, ctx: &BasicContext) -> bool;                  // no I/O
    fn enablement(&self, ctx: &Context) -> Enablement { Enablement::Enabled } // no I/O
    fn run(&self, ctx: &Context, job: &JobHandle) -> Result<Outcome, ActionError>; // worker thread
}
impl ActionRegistry {
    pub fn register(&mut self, a: Arc<dyn Action>) -> Result<(), RegistryError>;
    pub fn set_menu(&mut self, spec: Vec<MenuSpec>, overrides: MenuOverrides);
    pub fn build_menu(&self, ctx: &Context, limits: MenuLimits) -> Option<(MenuTree, ActionIndex)>;
    pub fn enablement_updates(&self, ctx: &Context, index: &ActionIndex) -> Vec<EnablementUpdate>;
}
impl Dispatcher { pub fn new(sink: JobSink, max_concurrent: usize) -> Self;
                  pub fn dispatch(&mut self, a: Arc<dyn Action>, ctx: Arc<Context>) -> JobId; pub fn cancel_all(&mut self); }

// radivine-platform — full trait in PLATFORM_INTEGRATION.md §7
pub trait Platform {
    fn capabilities(&self) -> &Capabilities;
    fn dispatch_pending(&mut self, out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError>;
    fn prepare_overlay(&mut self, at: PhysPoint) -> Result<OverlayInfo, PlatformError>; // configure, unmapped
    fn map_overlay(&mut self) -> Result<(), PlatformError>;
    fn present(&mut self, frame: FrameRef<'_>, damage: &[RectPx]) -> Result<(), PlatformError>;
    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError>;
    fn hide_overlay(&mut self);
    fn dnd_reply(&mut self, accept: bool);           // answer EVERY DndPosition in every state (reject unless Active)
    fn request_payload(&mut self, mime: &str);
    fn dnd_finish(&mut self, success: bool);
    fn grab_input(&mut self) -> Result<(), PlatformError>;   // Invoke mode only
    fn ungrab_input(&mut self);
    /* …monitors, scale, clipboard, deadlines: see PLATFORM_INTEGRATION.md §7 */
}
```

## 6. Implementation order

1. **M0** — spikes S-1…S-6 in `spikes/` (**requires the owner's explicit authorisation**; until then do not write code). Owner can first run the zero-code checks Q-1…Q-3. Record results; Go/No-Go (ADR-021).
2. **M1a** — workspace skeleton + `radivine-wheel` with full tests (may run in parallel with M0).
3. **M1b** — `radivine-render` + `tools/wheel-sandbox`; owner UX sign-off.
4. **M2** — X11 overlay + daemon + Invoke mode + minimal core with `LogAction`s.
5. **M3** — drag mode end-to-end with Nemo (the PoC; AC-3.1…AC-3.13).
6. **M4** — full action engine (enrichment, enablement, Copy, Properties, Custom).
7. **M5** — `radivine-viewer` + Full/Head/Tail.
8. **M6** — polish, benchmarks, packaging, docs.

Do not start a milestone before its predecessor's acceptance criteria pass (exception: M1a ∥ M0; M4 tasks 1–4 after M2).

## 7. Critical constraints (hard rules)

- **Never** modify, delete, rename or regenerate anything in `assets/branding/` or `assets/icons/`. (Note: `logo-primary.png` ≡ `logo-horizontal.png`, `logo-icon.png` ≡ `app-icon.png` byte-for-byte; `app-icon-monochrome.png` lives in `assets/icons/`.)
- Drag mode: **no `GrabPointer`/`GrabKeyboard`**, reply `XdndStatus` before rendering and to every `XdndPosition` in every state, re-check the drop target when `XdndDrop` arrives, always `XdndActionPrivate`, **never** convert `DELETE`, never use XTEST.
- Crates that must not depend on each other define their own small value types; the app converts (ARCHITECTURE §6 type-ownership table).
- Session rules (one session at a time, `DndLeave { drag_ended }`, watchdogs, empty input region on dismiss): ARCHITECTURE §5.
- Raw key events selected **only while armed**; never log key data.
- UI thread never touches the filesystem, D-Bus, or blocking calls.
- Visibility uses BasicContext only; enablement changes never change layout.
- Never open FIFOs/devices/sockets; open regular files with `O_NONBLOCK`, `fstat` before reading.
- Custom actions: argv only, whole-element placeholders, no implicit shell.
- No file conversion, no file-manager features, no plugin ABI, no settings GUI, no telemetry, no network.
- On Wayland: report capabilities honestly; no privileged input, no `/dev/input`, no `uinput`.
- Don't claim performance numbers without measurements recorded in `docs/BENCHMARKS.md`.
- Don't claim the PoC works until AC-3.x pass on the real Cinnamon desktop.

## 8. Testing commands

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
xvfb-run -s "-screen 0 1920x1080x24 +extension COMPOSITE +extension RANDR" \
  cargo test --workspace --features x11-integration
cargo bench -p radivine-wheel -p radivine-render
cargo deny check
./tests/e2e-x11/run_e2e.sh
cargo run -p wheel-sandbox                     # M1b dev tool
cargo run -p radivine -- daemon                # M2+
cargo run -p radivine -- invoke ~/some.txt     # M2+
cargo run -p radivine -- doctor
```

## 9. Known risks to watch (details in RISKS_AND_MITIGATIONS.md)

- **R-01** Nemo not retargeting to the mid-drag overlay → S-1 decides; fallbacks in FEASIBILITY_STUDY §8.5.
- **R-19** Hover-intent feel → scenario tests + sandbox sign-off; keep everything configurable.
- **R-06** Overlay stuck after drag → multiple end signals + watchdogs.
- **R-03** Shift not observed → announce XI 2.2; polling fallback.
- **R-02** Prefetch refused → build menu on first `XdndPosition`.
- **R-10/R-11** Wayland → capabilities + fallbacks; keep platform code isolated.
- **R-05** Data safety → `Private` + no `DELETE`; checksum checks in tests.

## 10. Rules for changing the plan

1. **Do not change the architecture, crate graph, public interfaces, or any Accepted ADR without adding a new ADR** in `docs/DECISIONS.md` that states the context, the evidence (spike/benchmark/test), alternatives, and consequences — and update the affected design documents in the same change.
2. Small, local API refinements (renaming a private type, adding a field) do not need an ADR but must be reflected in the design doc if the item is listed there as normative.
3. If a spike or test contradicts this plan, **stop, record the evidence, and propose the change** — do not silently work around it.
4. Keep `docs/` in sync with the code at every milestone completion.

## 11. Required acceptance criteria (summary)

- **M0:** AC-0.1…0.5 (S-1/S-2/S-6 pass; file untouched; results recorded).
- **M1a:** AC-1a.1…1a.7 (hit-test/placement/navigation/drop-target/keyboard/zero-alloc/deps).
- **M1b:** AC-1b.1…1b.5 (sandbox behaviour, animations, frame budgets, stable snapshots, owner sign-off).
- **M2:** AC-2.1…2.9 (invoke overlay, navigation, URIs, cancel, no stuck grabs, edges, scale, idle, doctor).
- **M3 (PoC):** AC-3.1…3.13 — activation, positioning, hierarchy, pointer navigation, file context, action selection, event routing, cancellation, clean dismissal, no stuck grabs, no interference, data safety, robustness.
- **M4:** AC-4.1…4.7 · **M5:** AC-5.1…5.5 · **M6:** AC-6.1…6.5.

Full text of every criterion: [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md).
