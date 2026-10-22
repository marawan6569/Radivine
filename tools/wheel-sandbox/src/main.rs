//! `wheel-sandbox` — Interactive standalone development sandbox for `radivine-wheel` and `radivine-render`.
//!
//! Demonstrates the actual `radivine-wheel` state machine and `radivine-render` software renderer
//! in an interactive `winit` + `softbuffer` window (or via `--export-screenshots <dir>`).

#![forbid(unsafe_code)]

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap};
use radivine_render::{AssetSource, FrameRef, RectPx, Renderer, SizePx, Theme};
use radivine_wheel::{
    DismissReason, DropTarget, InputMode, ItemPath, MenuNode, MenuTree, NavKey, NodeKind, Point,
    SessionPhase, Shape, Viewport, WheelConfig, WheelEvent, WheelInput, WheelSession,
};
use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

const EMBEDDED_INTER_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter-Variable.ttf");

/// Builds the required M1b demonstration menu (`IMPLEMENTATION_PLAN.md` §M1b).
fn build_demo_menu() -> MenuTree {
    MenuTree::new(vec![
        MenuNode::submenu(
            "radivine.preview",
            "File Preview",
            vec![
                MenuNode::leaf("radivine.preview.full", "Full Preview")
                    .with_icon("lucide:file-text")
                    .with_description("Open full interactive document preview"),
                MenuNode::leaf("radivine.preview.head", "Head")
                    .with_icon("lucide:arrow-up-to-line")
                    .with_description("Preview first 50 lines of file"),
                MenuNode::leaf("radivine.preview.tail", "Tail")
                    .with_icon("lucide:arrow-down-to-line")
                    .with_description("Preview last 50 lines of file"),
            ],
        )
        .with_icon("lucide:eye")
        .with_description("Inspect file contents without opening an editor"),
        MenuNode::leaf("radivine.copy", "Copy")
            .with_icon("lucide:copy")
            .with_description("Copy selected file to clipboard"),
        MenuNode::leaf("radivine.properties", "Properties")
            .with_icon("lucide:info")
            .with_description("Inspect permissions, size, and MIME metadata"),
        MenuNode::submenu(
            "radivine.custom",
            "Custom Actions",
            vec![
                MenuNode::leaf("custom.terminal", "Open in Terminal")
                    .with_icon("lucide:terminal")
                    .with_description("Launch terminal in parent directory"),
                MenuNode::submenu(
                    "custom.scripts",
                    "Scripts",
                    vec![
                        MenuNode::leaf("custom.scripts.sha256", "Compute SHA-256")
                            .with_icon("lucide:code")
                            .with_description("Calculate SHA-256 digest"),
                        MenuNode::leaf("custom.scripts.chmod", "Make Executable")
                            .with_icon("lucide:sparkles")
                            .with_description("Set +x permission bit"),
                    ],
                )
                .with_icon("lucide:code")
                .with_description("Run user automation scripts"),
                MenuNode::leaf("custom.gpg_sign", "Sign with GPG")
                    .with_icon("lucide:link")
                    .with_disabled("No GPG signing key configured"),
            ],
        )
        .with_icon("lucide:sparkles")
        .with_description("Extensible custom tools and scripts"),
    ])
}

/// Builds a 10-item menu to demonstrate automatic `More…` overflow (`> 8` items).
fn build_overflow_menu() -> MenuTree {
    let mut items = Vec::new();
    let icons = [
        "lucide:eye",
        "lucide:copy",
        "lucide:info",
        "lucide:terminal",
        "lucide:code",
        "lucide:file-text",
        "lucide:folder",
        "lucide:image",
        "lucide:link",
        "lucide:sparkles",
    ];
    for i in 1..=10 {
        items.push(
            MenuNode::leaf(format!("overflow.action_{i}"), format!("Action {i}"))
                .with_icon(icons[(i - 1) % icons.len()])
                .with_description(format!("Mock overflow action #{i}")),
        );
    }
    MenuTree::new(items)
}

/// Resolves a human-readable breadcrumb trail (`File Preview › Full Preview`) and action ID for a menu path.
fn resolve_path_info(tree: &MenuTree, path: &ItemPath) -> (String, String) {
    let mut labels = Vec::new();
    let mut last_id = String::from("<root>");
    let mut cur = &tree.root;
    for &idx in path.as_slice() {
        let Some(children) = cur.children() else {
            labels.push(format!("#{idx}"));
            break;
        };
        let Some(node) = children.get(idx as usize) else {
            labels.push(format!("#{idx}"));
            break;
        };
        labels.push(node.label.to_string());
        last_id = node.id.to_string();
        cur = node;
    }
    let crumb = if labels.is_empty() {
        "<root>".to_string()
    } else {
        labels.join(" › ")
    };
    (last_id, crumb)
}

#[derive(Clone, Debug)]
struct ActionToast {
    title: String,
    action_id: String,
    breadcrumb: String,
    raw_path: String,
    is_cancel: bool,
}

struct HudDrawer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl HudDrawer {
    fn new() -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(EMBEDDED_INTER_TTF.to_vec());
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        Self {
            font_system,
            swash_cache: SwashCache::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        font_px: f32,
        weight: u16,
        color: [u8; 4],
    ) {
        let metrics = Metrics::new(font_px, (font_px * 1.35).ceil());
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_wrap(&mut self.font_system, Wrap::None);
        buffer.set_size(&mut self.font_system, Some(2000.0), Some(font_px * 2.0));
        let attrs = Attrs::new()
            .family(Family::Name("Inter"))
            .weight(Weight(weight));
        buffer.set_text(&mut self.font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let w = pixmap.width() as i32;
        let h = pixmap.height() as i32;
        let data = pixmap.data_mut();

        for run in buffer.layout_runs() {
            let line_y = run.line_y;
            for glyph in run.glyphs.iter() {
                let physical = glyph.physical((x, y + line_y), 1.0);
                let Some(image) = self
                    .swash_cache
                    .get_image(&mut self.font_system, physical.cache_key)
                else {
                    continue;
                };
                let gx0 = physical.x + image.placement.left;
                let gy0 = physical.y - image.placement.top;
                let gw = image.placement.width as i32;
                let gh = image.placement.height as i32;

                if image.content == cosmic_text::SwashContent::Mask {
                    for row in 0..gh {
                        let py = gy0 + row;
                        if py < 0 || py >= h {
                            continue;
                        }
                        for col in 0..gw {
                            let px = gx0 + col;
                            if px < 0 || px >= w {
                                continue;
                            }
                            let cov = image.data[(row * gw + col) as usize];
                            if cov == 0 {
                                continue;
                            }
                            let a = ((u16::from(color[3]) * u16::from(cov) + 127) / 255) as u8;
                            blend_pixel(
                                data,
                                (py * w + px) as usize,
                                color[0],
                                color[1],
                                color[2],
                                a,
                            );
                        }
                    }
                }
            }
        }
    }
}

#[inline]
fn blend_pixel(buf: &mut [u8], idx: usize, r: u8, g: u8, b: u8, a: u8) {
    if a == 0 {
        return;
    }
    let off = idx * 4;
    let sr = (u16::from(r) * u16::from(a) + 127) / 255;
    let sg = (u16::from(g) * u16::from(a) + 127) / 255;
    let sb = (u16::from(b) * u16::from(a) + 127) / 255;
    let sa = u16::from(a);
    let inv_a = 255 - sa;

    let dr = u16::from(buf[off]);
    let dg = u16::from(buf[off + 1]);
    let db = u16::from(buf[off + 2]);
    let da = u16::from(buf[off + 3]);

    buf[off] = (sr + (dr * inv_a + 127) / 255).min(255) as u8;
    buf[off + 1] = (sg + (dg * inv_a + 127) / 255).min(255) as u8;
    buf[off + 2] = (sb + (db * inv_a + 127) / 255).min(255) as u8;
    buf[off + 3] = (sa + (da * inv_a + 127) / 255).min(255) as u8;
}

struct SandboxState {
    config: WheelConfig,
    dark_mode: bool,
    debug_overlay: bool,
    overflow_demo: bool,
    scale_override: f32,
    corner_preset_idx: usize,
    mode: InputMode,
    session: WheelSession,
    renderer: Renderer,
    hud: HudDrawer,
    start_instant: Instant,
    cursor_lp: Point,
    lmb_down: bool,
    modifiers: ModifiersState,
    hovered_path: Option<ItemPath>,
    last_toast: Option<ActionToast>,
    last_damage: Vec<RectPx>,
    events_buf: Vec<WheelEvent>,
}

impl SandboxState {
    fn new(width_px: u32, height_px: u32, scale: f32) -> Self {
        let config = WheelConfig::default();
        let vp = Viewport::new((width_px as f32) / scale, (height_px as f32) / scale, scale);
        let center = Point::new(vp.bounds.w * 0.5, vp.bounds.h * 0.52);
        let mut session = WheelSession::new(
            Some(build_demo_menu()),
            center,
            vp,
            InputMode::Pointer,
            &config,
            0,
        );
        session.set_root_badge(Some(1), 0);

        let mut renderer =
            Renderer::new(Theme::dark(), AssetSource::Embedded).expect("renderer init");
        renderer.begin_session(SizePx::new(width_px, height_px), scale);

        Self {
            config,
            dark_mode: true,
            debug_overlay: false,
            overflow_demo: false,
            scale_override: scale,
            corner_preset_idx: 0,
            mode: InputMode::Pointer,
            session,
            renderer,
            hud: HudDrawer::new(),
            start_instant: Instant::now(),
            cursor_lp: center,
            lmb_down: false,
            modifiers: ModifiersState::empty(),
            hovered_path: None,
            last_toast: None,
            last_damage: Vec::new(),
            events_buf: Vec::with_capacity(16),
        }
    }

    fn now_ms(&self) -> u64 {
        self.start_instant.elapsed().as_millis() as u64
    }

    fn sanitized_menu(&self) -> MenuTree {
        let mut m = if self.overflow_demo {
            build_overflow_menu()
        } else {
            build_demo_menu()
        };
        m.sanitize(&self.config);
        m
    }

    fn summon(&mut self, anchor: Point, mode: InputMode, now_ms: u64) {
        let size = self.renderer.frame();
        let w_px = size.width.max(1);
        let h_px = size.height.max(1);
        let vp = Viewport::new(
            (w_px as f32) / self.scale_override,
            (h_px as f32) / self.scale_override,
            self.scale_override,
        );
        self.mode = mode;
        self.session = WheelSession::new(
            Some(self.sanitized_menu()),
            anchor,
            vp,
            mode,
            &self.config,
            now_ms,
        );
        self.session
            .set_root_badge(Some(if mode == InputMode::Drag { 3 } else { 1 }), now_ms);
        self.hovered_path = None;
        self.renderer
            .begin_session(SizePx::new(w_px, h_px), self.scale_override);
        println!(
            "[wheel-sandbox] Summoned wheel at ({:.1}, {:.1}) lp in {:?} mode",
            anchor.x, anchor.y, mode
        );
    }

    fn dispatch_input(&mut self, input: WheelInput, now_ms: u64) {
        self.events_buf.clear();
        self.session.handle(input, now_ms, &mut self.events_buf);
        self.process_events(now_ms);
    }

    fn tick(&mut self, now_ms: u64) {
        self.events_buf.clear();
        self.session.tick(now_ms, &mut self.events_buf);
        self.process_events(now_ms);
    }

    fn commit_or_cancel_drag(&mut self, now_ms: u64) {
        let tree = self.sanitized_menu();
        if let Some(path) = self.session.commit_drop(now_ms) {
            let (id, crumb) = resolve_path_info(&tree, &path);
            println!(
                "[wheel-sandbox] ACTION SELECTED (Drag Drop): id=\"{}\" path={:?} (\"{}\")",
                id,
                path.as_slice(),
                crumb
            );
            self.last_toast = Some(ActionToast {
                title: "Action Executed (Drag Drop)".to_string(),
                action_id: id,
                breadcrumb: crumb,
                raw_path: format!("{:?}", path.as_slice()),
                is_cancel: false,
            });
        } else {
            self.session.dismiss(DismissReason::DragEnded, now_ms);
            println!("[wheel-sandbox] CANCELLED: Drag released on non-executable zone");
            self.last_toast = Some(ActionToast {
                title: "Drag Drop Rejected / Cancelled".to_string(),
                action_id: "DismissReason::DragEnded".to_string(),
                breadcrumb: "Released outside an enabled leaf action".to_string(),
                raw_path: "—".to_string(),
                is_cancel: true,
            });
        }
    }

    fn process_events(&mut self, now_ms: u64) {
        let tree = self.sanitized_menu();
        let events: Vec<WheelEvent> = self.events_buf.clone();
        for ev in events {
            match ev {
                WheelEvent::HoverChanged(path) => {
                    self.hovered_path = path.clone();
                    if let Some(ref p) = path {
                        let (id, crumb) = resolve_path_info(&tree, p);
                        println!(
                            "[wheel-sandbox] HoverChanged: id=\"{}\" path={:?} (\"{}\")",
                            id,
                            p.as_slice(),
                            crumb
                        );
                    } else {
                        println!("[wheel-sandbox] HoverChanged: None");
                    }
                }
                WheelEvent::SubmenuOpened(path) => {
                    let (id, crumb) = resolve_path_info(&tree, &path);
                    println!(
                        "[wheel-sandbox] SubmenuOpened: id=\"{}\" path={:?} (\"{}\")",
                        id,
                        path.as_slice(),
                        crumb
                    );
                }
                WheelEvent::SubmenuClosed(path) => {
                    let (_, crumb) = resolve_path_info(&tree, &path);
                    println!(
                        "[wheel-sandbox] SubmenuClosed: path={:?} (\"{}\")",
                        path.as_slice(),
                        crumb
                    );
                }
                WheelEvent::Commit(path) => {
                    let (id, crumb) = resolve_path_info(&tree, &path);
                    println!(
                        "[wheel-sandbox] ACTION SELECTED: id=\"{}\" path={:?} (\"{}\")",
                        id,
                        path.as_slice(),
                        crumb
                    );
                    self.last_toast = Some(ActionToast {
                        title: "Action Executed".to_string(),
                        action_id: id,
                        breadcrumb: crumb,
                        raw_path: format!("{:?}", path.as_slice()),
                        is_cancel: false,
                    });
                }
                WheelEvent::CancelRequested => {
                    println!("[wheel-sandbox] CANCELLED: DismissReason::Cancelled");
                    self.session.dismiss(DismissReason::Cancelled, now_ms);
                    self.last_toast = Some(ActionToast {
                        title: "Session Cancelled".to_string(),
                        action_id: "DismissReason::Cancelled".to_string(),
                        breadcrumb: "Center Hub / Outside / Escape".to_string(),
                        raw_path: "—".to_string(),
                        is_cancel: true,
                    });
                }
                WheelEvent::InputRegionChanged | WheelEvent::Finished => {}
            }
        }
    }

    /// Renders the complete composite sandbox frame (backdrop + HUD + radial wheel + optional debug overlay)
    /// into a `Pixmap`.
    fn render_composite_pixmap(&mut self) -> Pixmap {
        let dmg = self.renderer.render(self.session.scene());
        self.last_damage = dmg.to_vec();
        let (w, h) = {
            let wf = self.renderer.frame();
            (wf.width.max(1), wf.height.max(1))
        };

        let mut canvas = Pixmap::new(w, h).expect("allocate sandbox canvas");
        draw_desktop_backdrop(&mut canvas, self.dark_mode, self.scale_override);

        // Draw HUD panel and optional toast banner
        self.draw_hud_and_toast(&mut canvas);

        // Alpha-over the premultiplied RGBA8 wheel framebuffer onto canvas
        {
            let wheel_frame: FrameRef<'_> = self.renderer.frame();
            let dst = canvas.data_mut();
            let src = wheel_frame.data;
            for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let sa = u16::from(s[3]);
                if sa == 0 {
                    continue;
                }
                if sa == 255 {
                    d.copy_from_slice(s);
                } else {
                    let inv_a = 255 - sa;
                    d[0] = (u16::from(s[0]) + (u16::from(d[0]) * inv_a + 127) / 255).min(255) as u8;
                    d[1] = (u16::from(s[1]) + (u16::from(d[1]) * inv_a + 127) / 255).min(255) as u8;
                    d[2] = (u16::from(s[2]) + (u16::from(d[2]) * inv_a + 127) / 255).min(255) as u8;
                    d[3] = 255;
                }
            }
        }

        if self.debug_overlay {
            self.draw_debug_overlay(&mut canvas);
        }

        canvas
    }

    fn draw_hud_and_toast(&mut self, canvas: &mut Pixmap) {
        let s = self.scale_override;
        let card_bg = if self.dark_mode {
            Color::from_rgba8(15, 23, 42, 220)
        } else {
            Color::from_rgba8(255, 255, 255, 232)
        };
        let card_border = if self.dark_mode {
            Color::from_rgba8(51, 65, 85, 200)
        } else {
            Color::from_rgba8(203, 213, 225, 220)
        };
        let fg_primary = if self.dark_mode {
            [248, 250, 252, 255]
        } else {
            [15, 23, 42, 255]
        };
        let fg_muted = if self.dark_mode {
            [148, 163, 184, 255]
        } else {
            [71, 85, 105, 255]
        };
        let accent = [16, 185, 129, 255];

        // Top-left interactive status & controls card
        let x0 = 20.0 * s;
        let y0 = 20.0 * s;
        let cw = 450.0 * s;
        let ch = 152.0 * s;
        fill_rounded_rect(
            canvas,
            x0,
            y0,
            cw,
            ch,
            10.0 * s,
            card_bg,
            Some((card_border, 1.0 * s)),
        );

        self.hud.draw_text(
            canvas,
            "RADIVINE WHEEL SANDBOX (M1b)",
            x0 + 14.0 * s,
            y0 + 12.0 * s,
            13.0 * s,
            700,
            accent,
        );

        let dbg = self.session.debug_state();
        let theme_str = if self.dark_mode { "Dark" } else { "Light" };
        let motion_str = if self.config.reduced_motion {
            "Reduced"
        } else {
            "Standard"
        };
        let status_line = format!(
            "Mode: {:?}  |  Phase: {:?}  |  Theme: {}  |  Scale: {:.1}x  |  Motion: {}",
            self.mode, dbg.phase, theme_str, self.scale_override, motion_str
        );
        self.hud.draw_text(
            canvas,
            &status_line,
            x0 + 14.0 * s,
            y0 + 34.0 * s,
            11.2 * s,
            500,
            fg_primary,
        );

        let tree = self.sanitized_menu();
        let drop_ok = matches!(self.session.drop_target(), DropTarget::Accept(_));
        let hover_str = match self.hovered_path {
            Some(ref p) => {
                let (_, crumb) = resolve_path_info(&tree, p);
                format!(
                    "Hovered: {} {:?}  |  Drop Accept: {}",
                    crumb,
                    p.as_slice(),
                    if drop_ok { "YES" } else { "NO" }
                )
            }
            None => format!(
                "Hovered: <none>  |  Open Wheels: {}",
                self.session.scene().wheels.len()
            ),
        };
        self.hud.draw_text(
            canvas,
            &hover_str,
            x0 + 14.0 * s,
            y0 + 54.0 * s,
            11.2 * s,
            600,
            fg_primary,
        );

        self.hud.draw_text(
            canvas,
            "Right-Click / Space: Summon (Pointer)   |   Hold LMB + Shift / G: Summon (Drag)",
            x0 + 14.0 * s,
            y0 + 80.0 * s,
            10.2 * s,
            500,
            fg_muted,
        );
        self.hud.draw_text(
            canvas,
            "Keys: [T] Theme  [D] Debug Zones  [C] Corner Preset  [M] Motion  [S] Scale  [O] Overflow",
            x0 + 14.0 * s,
            y0 + 100.0 * s,
            10.2 * s,
            500,
            fg_muted,
        );
        self.hud.draw_text(
            canvas,
            "Nav:  [Tab/Arrows] Cycle  [Enter] Select  [Backspace] Up Level  [1-9] Index  [Esc] Cancel",
            x0 + 14.0 * s,
            y0 + 120.0 * s,
            10.2 * s,
            500,
            fg_muted,
        );

        // Action Executed / Cancelled toast banner at top-right
        if let Some(ref toast) = self.last_toast {
            let tw = 430.0 * s;
            let th = 76.0 * s;
            let tx = (canvas.width() as f32) - tw - 20.0 * s;
            let ty = 20.0 * s;
            let border_col = if toast.is_cancel {
                Color::from_rgba8(245, 158, 11, 230)
            } else {
                Color::from_rgba8(16, 185, 129, 240)
            };
            fill_rounded_rect(
                canvas,
                tx,
                ty,
                tw,
                th,
                10.0 * s,
                card_bg,
                Some((border_col, 1.5 * s)),
            );

            let badge_col = if toast.is_cancel {
                [245, 158, 11, 255]
            } else {
                [16, 185, 129, 255]
            };
            self.hud.draw_text(
                canvas,
                &toast.title.to_uppercase(),
                tx + 14.0 * s,
                ty + 10.0 * s,
                11.0 * s,
                700,
                badge_col,
            );
            self.hud.draw_text(
                canvas,
                &format!("ID: {}   (Path: {})", toast.action_id, toast.raw_path),
                tx + 14.0 * s,
                ty + 29.0 * s,
                12.5 * s,
                600,
                fg_primary,
            );
            self.hud.draw_text(
                canvas,
                &format!("Menu Path: {}", toast.breadcrumb),
                tx + 14.0 * s,
                ty + 50.0 * s,
                11.5 * s,
                500,
                fg_muted,
            );
        }
    }

    fn draw_debug_overlay(&mut self, canvas: &mut Pixmap) {
        let s = self.scale_override;
        let dbg = self.session.debug_state();

        // 1. Draw XShape / input-region shapes in cyan
        for shape in self.session.input_region() {
            match shape {
                Shape::Circle { c, r } => {
                    stroke_circle(
                        canvas,
                        c.x * s,
                        c.y * s,
                        *r * s,
                        Color::from_rgba8(56, 189, 248, 185),
                        1.5 * s,
                    );
                }
                Shape::Polygon(pts) => {
                    if pts.len() >= 3 {
                        let mut pb = PathBuilder::new();
                        pb.move_to(pts[0].x * s, pts[0].y * s);
                        for p in &pts[1..] {
                            pb.line_to(p.x * s, p.y * s);
                        }
                        pb.close();
                        if let Some(path) = pb.finish() {
                            let mut fill_paint = Paint::default();
                            fill_paint.set_color(Color::from_rgba8(56, 189, 248, 32));
                            fill_paint.anti_alias = true;
                            canvas.fill_path(
                                &path,
                                &fill_paint,
                                FillRule::Winding,
                                Transform::identity(),
                                None,
                            );

                            let mut stroke_paint = Paint::default();
                            stroke_paint.set_color(Color::from_rgba8(56, 189, 248, 190));
                            stroke_paint.anti_alias = true;
                            let stroke = Stroke {
                                width: 1.4 * s,
                                line_cap: LineCap::Round,
                                ..Stroke::default()
                            };
                            canvas.stroke_path(
                                &path,
                                &stroke_paint,
                                &stroke,
                                Transform::identity(),
                                None,
                            );
                        }
                    }
                }
            }
        }

        // 2. For each open wheel, draw outer fast-open band (r_in + band*(r_out - r_in)) and hub dead zone (r_in)
        for w in &dbg.wheels {
            let cx = w.center.x * s;
            let cy = w.center.y * s;
            let r_fast = (w.r_in + self.config.band * (w.r_out - w.r_in)) * s;
            stroke_circle(
                canvas,
                cx,
                cy,
                r_fast,
                Color::from_rgba8(16, 185, 129, 175),
                1.2 * s,
            );
            stroke_circle(
                canvas,
                cx,
                cy,
                w.r_in * s,
                Color::from_rgba8(244, 63, 94, 180),
                1.2 * s,
            );
        }

        // 3. Draw active intent-protection triangle if active, or illustrative tangent cone from active parent segment
        if let Some(([p0, p1, p2], _deadline)) = dbg.intent_triangle {
            draw_triangle(canvas, p0, p1, p2, s);
        } else if dbg.wheels.len() >= 2 {
            let scene = self.session.scene();
            for i in 0..(scene.wheels.len().saturating_sub(1)) {
                let parent = &scene.wheels[i];
                let child = &scene.wheels[i + 1];
                if let Some(seg) = parent.segments.iter().find(|seg| seg.on_path) {
                    let mid_a = (seg.a0 + seg.a1) * 0.5;
                    let mid_r = (parent.r_in + parent.r_out) * 0.5;
                    let apex = Point::new(
                        parent.center.x + mid_r * mid_a.cos(),
                        parent.center.y + mid_r * mid_a.sin(),
                    );
                    let dx = child.center.x - apex.x;
                    let dy = child.center.y - apex.y;
                    let d = dx.hypot(dy);
                    let r_hit = child.r_out + self.config.hit_margin;
                    if d > 1.0 {
                        let perp_x = -dy / d;
                        let perp_y = dx / d;
                        let t1 = Point::new(
                            child.center.x + perp_x * r_hit,
                            child.center.y + perp_y * r_hit,
                        );
                        let t2 = Point::new(
                            child.center.x - perp_x * r_hit,
                            child.center.y - perp_y * r_hit,
                        );
                        draw_triangle(canvas, apex, t1, t2, s);
                    }
                }
            }
        }

        // 4. Draw last frame's damage rectangles in magenta
        for r in &self.last_damage {
            let mut pb = PathBuilder::new();
            pb.move_to(r.x as f32, r.y as f32);
            pb.line_to((r.x + r.w as i32) as f32, r.y as f32);
            pb.line_to((r.x + r.w as i32) as f32, (r.y + r.h as i32) as f32);
            pb.line_to(r.x as f32, (r.y + r.h as i32) as f32);
            pb.close();
            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba8(232, 121, 249, 175));
                let stroke = Stroke {
                    width: 1.0 * s,
                    ..Stroke::default()
                };
                canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    }
}

fn draw_triangle(canvas: &mut Pixmap, p0: Point, p1: Point, p2: Point, s: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(p0.x * s, p0.y * s);
    pb.line_to(p1.x * s, p1.y * s);
    pb.line_to(p2.x * s, p2.y * s);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut fill_paint = Paint::default();
        fill_paint.set_color(Color::from_rgba8(250, 204, 21, 42));
        fill_paint.anti_alias = true;
        canvas.fill_path(
            &path,
            &fill_paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );

        let mut stroke_paint = Paint::default();
        stroke_paint.set_color(Color::from_rgba8(250, 204, 21, 215));
        stroke_paint.anti_alias = true;
        let stroke = Stroke {
            width: 1.5 * s,
            line_cap: LineCap::Round,
            ..Stroke::default()
        };
        canvas.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
    }
}

fn draw_desktop_backdrop(pixmap: &mut Pixmap, dark_mode: bool, scale: f32) {
    let bg = if dark_mode {
        Color::from_rgba8(9, 13, 22, 255)
    } else {
        Color::from_rgba8(241, 245, 249, 255)
    };
    pixmap.fill(bg);

    let grid_col = if dark_mode {
        Color::from_rgba8(30, 41, 59, 110)
    } else {
        Color::from_rgba8(203, 213, 225, 140)
    };
    let step = (32.0 * scale).round().max(8.0) as u32;
    let w = pixmap.width();
    let h = pixmap.height();

    let mut paint = Paint::default();
    paint.set_color(grid_col);
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };

    let mut pb = PathBuilder::new();
    let mut x = step;
    while x < w {
        pb.move_to(x as f32 + 0.5, 0.0);
        pb.line_to(x as f32 + 0.5, h as f32);
        x += step;
    }
    let mut y = step;
    while y < h {
        pb.move_to(0.0, y as f32 + 0.5);
        pb.line_to(w as f32, y as f32 + 0.5);
        y += step;
    }
    if let Some(path) = pb.finish() {
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect(
    pixmap: &mut Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    fill: Color,
    border: Option<(Color, f32)>,
) {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    let Some(path) = pb.finish() else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(fill);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    if let Some((bcol, bw)) = border {
        let mut bpaint = Paint::default();
        bpaint.set_color(bcol);
        bpaint.anti_alias = true;
        let stroke = Stroke {
            width: bw,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &bpaint, &stroke, Transform::identity(), None);
    }
}

fn stroke_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, col: Color, width: f32) {
    if r <= 0.5 {
        return;
    }
    let Some(path) = PathBuilder::from_circle(cx, cy, r) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(col);
    paint.anti_alias = true;
    let stroke = Stroke {
        width,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
}

struct SandboxApp {
    state: SandboxState,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    auto_quit_ms: Option<u64>,
}

impl SandboxApp {
    fn new(auto_quit_ms: Option<u64>) -> Self {
        Self {
            state: SandboxState::new(1280, 820, 1.0),
            window: None,
            surface: None,
            auto_quit_ms,
        }
    }

    fn present(&mut self) {
        let Some(ref window) = self.window else {
            return;
        };
        let Some(ref mut surface) = self.surface else {
            return;
        };

        let size = window.inner_size();
        let w = size.width.max(1);
        let h = size.height.max(1);
        let _ = surface.resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap());

        if self.state.renderer.frame().width != w || self.state.renderer.frame().height != h {
            self.state
                .renderer
                .begin_session(SizePx::new(w, h), self.state.scale_override);
        }

        let composite = self.state.render_composite_pixmap();
        if let Ok(mut buffer) = surface.buffer_mut() {
            for (dst, src) in buffer.iter_mut().zip(composite.data().chunks_exact(4)) {
                let r = u32::from(src[0]);
                let g = u32::from(src[1]);
                let b = u32::from(src[2]);
                *dst = (r << 16) | (g << 8) | b;
            }
            let _ = buffer.present();
        }
    }
}

impl ApplicationHandler for SandboxApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("Radivine — Wheel Engine Sandbox (M1b)")
            .with_inner_size(LogicalSize::new(1280.0, 820.0));
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("create sandbox window"),
        );
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        let sz = window.inner_size();
        let scale = window.scale_factor() as f32;
        self.state.scale_override = scale;
        self.state.summon(
            Point::new(
                (sz.width as f32) / scale * 0.5,
                (sz.height as f32) / scale * 0.52,
            ),
            InputMode::Pointer,
            0,
        );
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let now = self.state.now_ms();
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(phys_size) => {
                let w = phys_size.width.max(1);
                let h = phys_size.height.max(1);
                self.state
                    .renderer
                    .begin_session(SizePx::new(w, h), self.state.scale_override);
                if let Some(ref win) = self.window {
                    win.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                let prev_shift = self.state.modifiers.shift_key();
                let new_shift = mods.state().shift_key();
                self.state.modifiers = mods.state();
                if prev_shift != new_shift && new_shift {
                    let phase = self.state.session.debug_state().phase;
                    if self.state.lmb_down && phase == SessionPhase::Finished {
                        self.state
                            .summon(self.state.cursor_lp, InputMode::Drag, now);
                    } else if matches!(phase, SessionPhase::Appearing | SessionPhase::Open) {
                        self.state.session.dismiss(DismissReason::Toggled, now);
                        println!("[wheel-sandbox] CANCELLED: DismissReason::Toggled (Shift)");
                    }
                }
            }
            WindowEvent::CursorMoved {
                position: PhysicalPosition { x, y },
                ..
            } => {
                let pt = Point::new(
                    (x as f32) / self.state.scale_override,
                    (y as f32) / self.state.scale_override,
                );
                self.state.cursor_lp = pt;
                self.state.dispatch_input(WheelInput::PointerMoved(pt), now);
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    self.state.lmb_down = true;
                    if self.state.mode == InputMode::Pointer {
                        self.state.dispatch_input(WheelInput::Press, now);
                    }
                }
                (MouseButton::Left, ElementState::Released) => {
                    self.state.lmb_down = false;
                    if self.state.mode == InputMode::Drag {
                        let phase = self.state.session.debug_state().phase;
                        if matches!(phase, SessionPhase::Appearing | SessionPhase::Open) {
                            self.state.commit_or_cancel_drag(now);
                        }
                    } else {
                        self.state.dispatch_input(WheelInput::Release, now);
                    }
                }
                (MouseButton::Right, ElementState::Pressed) => {
                    self.state
                        .summon(self.state.cursor_lp, InputMode::Pointer, now);
                }
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.logical_key {
                    WinitKey::Named(NamedKey::Escape) => {
                        if self.state.mode == InputMode::Drag {
                            self.state.session.dismiss(DismissReason::Cancelled, now);
                            println!("[wheel-sandbox] CANCELLED: Escape in Drag mode");
                        } else {
                            self.state
                                .dispatch_input(WheelInput::Nav(NavKey::Cancel), now);
                        }
                    }
                    WinitKey::Named(NamedKey::Enter) => {
                        self.state
                            .dispatch_input(WheelInput::Nav(NavKey::Activate), now);
                    }
                    WinitKey::Named(NamedKey::Backspace) => {
                        self.state
                            .dispatch_input(WheelInput::Nav(NavKey::Back), now);
                    }
                    WinitKey::Named(NamedKey::Tab) => {
                        let key = if self.state.modifiers.shift_key() {
                            NavKey::Prev
                        } else {
                            NavKey::Next
                        };
                        self.state.dispatch_input(WheelInput::Nav(key), now);
                    }
                    WinitKey::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                        self.state
                            .dispatch_input(WheelInput::Nav(NavKey::Next), now);
                    }
                    WinitKey::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                        self.state
                            .dispatch_input(WheelInput::Nav(NavKey::Prev), now);
                    }
                    WinitKey::Named(NamedKey::Space) => {
                        self.state
                            .summon(self.state.cursor_lp, InputMode::Pointer, now);
                    }
                    WinitKey::Named(NamedKey::F2) => {
                        self.state.debug_overlay = !self.state.debug_overlay;
                        println!(
                            "[wheel-sandbox] Debug overlay: {}",
                            self.state.debug_overlay
                        );
                    }
                    WinitKey::Character(ref ch) => match ch.as_str() {
                        "t" | "T" => {
                            self.state.dark_mode = !self.state.dark_mode;
                            self.state.renderer.set_theme(if self.state.dark_mode {
                                Theme::dark()
                            } else {
                                Theme::light()
                            });
                            println!(
                                "[wheel-sandbox] Theme switched to {}",
                                if self.state.dark_mode {
                                    "Dark"
                                } else {
                                    "Light"
                                }
                            );
                        }
                        "d" | "D" => {
                            self.state.debug_overlay = !self.state.debug_overlay;
                            println!(
                                "[wheel-sandbox] Debug overlay: {}",
                                self.state.debug_overlay
                            );
                        }
                        "m" | "M" => {
                            self.state.config.reduced_motion = !self.state.config.reduced_motion;
                            println!(
                                "[wheel-sandbox] Reduced motion: {}",
                                self.state.config.reduced_motion
                            );
                            self.state
                                .summon(self.state.cursor_lp, InputMode::Pointer, now);
                        }
                        "g" | "G" => {
                            self.state
                                .summon(self.state.cursor_lp, InputMode::Drag, now);
                        }
                        "i" | "I" => {
                            self.state
                                .summon(self.state.cursor_lp, InputMode::Pointer, now);
                        }
                        "o" | "O" => {
                            self.state.overflow_demo = !self.state.overflow_demo;
                            println!(
                                "[wheel-sandbox] Overflow demo (10 items): {}",
                                self.state.overflow_demo
                            );
                            self.state
                                .summon(self.state.cursor_lp, InputMode::Pointer, now);
                        }
                        "s" | "S" => {
                            self.state.scale_override =
                                if (self.state.scale_override - 1.0).abs() < 0.1 {
                                    1.5
                                } else if (self.state.scale_override - 1.5).abs() < 0.1 {
                                    2.0
                                } else {
                                    1.0
                                };
                            println!(
                                "[wheel-sandbox] Scale factor set to {:.1}x",
                                self.state.scale_override
                            );
                            let f = self.state.renderer.frame();
                            let anchor = Point::new(
                                (f.width as f32) / self.state.scale_override * 0.5,
                                (f.height as f32) / self.state.scale_override * 0.52,
                            );
                            self.state.summon(anchor, InputMode::Pointer, now);
                        }
                        "c" | "C" => {
                            let f = self.state.renderer.frame();
                            let vw = (f.width as f32) / self.state.scale_override;
                            let vh = (f.height as f32) / self.state.scale_override;
                            let presets = [
                                ("Center", Point::new(vw * 0.5, vh * 0.52)),
                                ("Top-Right Edge", Point::new(vw - 85.0, 110.0)),
                                ("Bottom-Right Edge", Point::new(vw - 90.0, vh - 90.0)),
                                ("Bottom-Left Edge", Point::new(90.0, vh - 90.0)),
                                ("Top-Left Edge", Point::new(90.0, 110.0)),
                            ];
                            self.state.corner_preset_idx =
                                (self.state.corner_preset_idx + 1) % presets.len();
                            let (label, pt) = presets[self.state.corner_preset_idx];
                            println!(
                                "[wheel-sandbox] Corner preset: {label} at ({:.0}, {:.0})",
                                pt.x, pt.y
                            );
                            self.state.summon(pt, InputMode::Pointer, now);
                        }
                        digit if digit.len() == 1 => {
                            let b = digit.as_bytes()[0];
                            if (b'1'..=b'9').contains(&b) {
                                self.state
                                    .dispatch_input(WheelInput::Nav(NavKey::Digit(b - b'0')), now);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                self.state.tick(now);
                self.present();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = self.state.now_ms();
        if let Some(limit) = self.auto_quit_ms
            && now >= limit
        {
            event_loop.exit();
            return;
        }
        self.state.tick(now);
        if let Some(ref win) = self.window {
            win.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + std::time::Duration::from_millis(16),
        ));
    }
}

/// Deterministic screenshot generator for M1b visual verification (`--export-screenshots <dir>`).
fn export_screenshots(out_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let w = 1280u32;
    let h = 820u32;
    let center = Point::new(640.0, 440.0);

    let save = |state: &mut SandboxState, filename: &str| -> std::io::Result<()> {
        let pm = state.render_composite_pixmap();
        let frame = FrameRef {
            data: pm.data(),
            width: pm.width(),
            height: pm.height(),
            stride: (pm.width() as usize) * 4,
        };
        let path = out_dir.join(filename);
        radivine_render::save_frame_png(frame, &path)?;
        println!("[wheel-sandbox] Saved screenshot: {}", path.display());
        Ok(())
    };

    // 1. Root wheel (Dark theme, "Copy" hovered)
    {
        let mut st = SandboxState::new(w, h, 1.0);
        st.summon(center, InputMode::Drag, 0);
        st.tick(250);
        let p_copy = st.session.item_center(&ItemPath::from_slice(&[1])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_copy), 260);
        st.tick(320);
        save(&mut st, "01-root-wheel-dark.png")?;
    }

    // 2. Child wheel open ("File Preview" -> "Full Preview" hovered)
    {
        let mut st = SandboxState::new(w, h, 1.0);
        st.summon(center, InputMode::Drag, 0);
        st.tick(250);
        let p_preview = st.session.item_center(&ItemPath::from_slice(&[0])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_preview), 260);
        st.tick(450); // past 160ms dwell
        st.tick(650); // child appear settled
        let p_full = st
            .session
            .item_center(&ItemPath::from_slice(&[0, 0]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_full), 660);
        st.tick(720);
        save(&mut st, "02-child-wheel-file-preview.png")?;
    }

    // 3. Three-level nested hierarchy ("Custom Actions" -> "Scripts" -> "Compute SHA-256")
    {
        let mut st = SandboxState::new(w, h, 1.0);
        st.summon(center, InputMode::Drag, 0);
        st.tick(250);
        let p_custom = st.session.item_center(&ItemPath::from_slice(&[3])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_custom), 260);
        st.tick(450);
        st.tick(650);
        let p_scripts = st
            .session
            .item_center(&ItemPath::from_slice(&[3, 1]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_scripts), 660);
        st.tick(850);
        st.tick(1050);
        let p_sha = st
            .session
            .item_center(&ItemPath::from_slice(&[3, 1, 0]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_sha), 1060);
        st.tick(1120);
        save(&mut st, "03-three-level-nested-scripts.png")?;
    }

    // 4. Screen-edge / corner boundary adaptation (Top-Right corner summon + "File Preview" open)
    {
        let mut st = SandboxState::new(w, h, 1.0);
        let corner = Point::new((w as f32) - 95.0, 125.0);
        st.summon(corner, InputMode::Pointer, 0);
        st.tick(250);
        let p_preview = st.session.item_center(&ItemPath::from_slice(&[0])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_preview), 260);
        st.tick(450);
        st.tick(650);
        let p_head = st
            .session
            .item_center(&ItemPath::from_slice(&[0, 1]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_head), 660);
        st.tick(720);
        save(&mut st, "04-screen-edge-adaptation.png")?;
    }

    // 5. Light theme + disabled item hovered ("Custom Actions" -> "Sign with GPG")
    {
        let mut st = SandboxState::new(w, h, 1.0);
        st.dark_mode = false;
        st.renderer.set_theme(Theme::light());
        st.summon(center, InputMode::Pointer, 0);
        st.tick(250);
        let p_custom = st.session.item_center(&ItemPath::from_slice(&[3])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_custom), 260);
        st.tick(450);
        st.tick(650);
        let p_gpg = st
            .session
            .item_center(&ItemPath::from_slice(&[3, 2]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_gpg), 660);
        st.tick(720);
        save(&mut st, "05-light-theme-and-disabled-item.png")?;
    }

    // 6. Debug overlay active (hit zones, outer fast-open band, safe navigation corridor / intent triangle)
    {
        let mut st = SandboxState::new(w, h, 1.0);
        st.debug_overlay = true;
        st.summon(center, InputMode::Drag, 0);
        st.tick(250);
        let p_preview = st.session.item_center(&ItemPath::from_slice(&[0])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_preview), 260);
        st.tick(450);
        st.tick(650);
        let p_tail = st
            .session
            .item_center(&ItemPath::from_slice(&[0, 2]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_tail), 660);
        st.tick(720);
        save(&mut st, "06-debug-overlay-corridors.png")?;
    }

    // 7. Action selected confirmation toast ("File Preview -> Full Preview" committed)
    {
        let mut st = SandboxState::new(w, h, 1.0);
        st.summon(center, InputMode::Drag, 0);
        st.tick(250);
        let p_preview = st.session.item_center(&ItemPath::from_slice(&[0])).unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_preview), 260);
        st.tick(450);
        st.tick(650);
        let p_full = st
            .session
            .item_center(&ItemPath::from_slice(&[0, 0]))
            .unwrap();
        st.dispatch_input(WheelInput::PointerMoved(p_full), 660);
        st.commit_or_cancel_drag(680);
        // Advance 40ms into the 90ms commit flash so both the flashing segment and the toast banner are visible
        st.tick(720);
        save(&mut st, "07-action-selected-toast.png")?;
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    let mut export_dir: Option<PathBuf> = None;
    let mut auto_quit_ms: Option<u64> = None;

    while i < args.len() {
        match args[i].as_str() {
            "--export-screenshots" => {
                if let Some(dir) = args.get(i + 1) {
                    export_dir = Some(PathBuf::from(dir));
                    i += 2;
                } else {
                    eprintln!("Error: --export-screenshots requires a <dir> argument");
                    std::process::exit(2);
                }
            }
            "--auto-quit-ms" => {
                if let Some(ms_str) = args.get(i + 1) {
                    auto_quit_ms = ms_str.parse::<u64>().ok();
                    i += 2;
                } else {
                    eprintln!("Error: --auto-quit-ms requires a <ms> argument");
                    std::process::exit(2);
                }
            }
            "--help" | "-h" => {
                println!("Usage: wheel-sandbox [--export-screenshots <dir>] [--auto-quit-ms <ms>]");
                return;
            }
            other => {
                eprintln!("Unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    if let Some(ref dir) = export_dir {
        if let Err(err) = export_screenshots(dir) {
            eprintln!("Failed to export screenshots: {err}");
            std::process::exit(1);
        }
        return;
    }

    let event_loop = EventLoop::new().expect("create winit event loop");
    let mut app = SandboxApp::new(auto_quit_ms);
    let _ = NodeKind::Leaf;
    event_loop.run_app(&mut app).expect("run sandbox app");
}
