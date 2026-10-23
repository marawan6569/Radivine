//! `eframe`/`egui` graphical Control Center interface (`apps/radivine-control/src/ui.rs`).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Layout, Margin, RichText, ScrollArea, Stroke, TextureHandle, TextureOptions, Vec2,
};

use radivine::config::{AppConfig, ReducedMotionSetting, ScaleSetting};
use radivine::ipc::DaemonStatus;

use crate::ControlPage;
use crate::diagnostics::{
    DiagnosticsSnapshot, LogEntry, LogSeverity, collect_diagnostics_snapshot,
    format_diagnostic_report,
};
use crate::persistence::{
    DiskFileSnapshot, EditableAction, EditableActionsDoc, apply_actions_to_toml_doc,
    apply_app_config_to_toml_doc, atomic_write_with_backup,
};
use crate::screenshot::EguiSoftRasterizer;
use crate::service::{
    AutostartConflict, AutostartStatus, DaemonHealthState, ServiceController, ServicePaths,
    ServiceSnapshot, SystemdUnitInfo,
};
use crate::single_instance::ControlInstanceLock;

const EMBEDDED_INTER_TTF: &[u8] = include_bytes!("../../../assets/fonts/Inter-Variable.ttf");
const EMBEDDED_APP_ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/app-icon.png");
const EMBEDDED_LOGO_HORIZONTAL_PNG: &[u8] =
    include_bytes!("../../../assets/branding/logo-horizontal.png");

const AVAILABLE_ICONS: &[(&str, &str)] = &[
    ("lucide:sparkles", "Sparkles (Custom)"),
    ("lucide:terminal", "Terminal"),
    ("lucide:code", "Code / Hash"),
    ("lucide:file-text", "Text Document"),
    ("lucide:eye", "Preview / Eye"),
    ("lucide:copy", "Copy"),
    ("lucide:info", "Info / Properties"),
    ("lucide:folder", "Folder"),
    ("lucide:file", "File"),
    ("lucide:files", "Multiple Files"),
    ("lucide:image", "Image"),
    ("lucide:link", "Symbolic Link"),
    ("lucide:arrow-up-to-line", "Top / Head"),
    ("lucide:arrow-down-to-line", "Bottom / Tail"),
];

const PLACEHOLDER_CHIPS: &[(&str, &str)] = &[
    ("{path}", "Single local file path"),
    ("{paths}", "All selected local paths"),
    ("{uri}", "Single file:// URI"),
    ("{uris}", "All selected URIs"),
    ("{dir}", "Parent folder path"),
    ("{name}", "Filename only"),
];

#[derive(Clone, Copy)]
struct BrandPalette {
    bg_canvas: Color32,
    bg_sidebar: Color32,
    bg_card: Color32,
    bg_elevated: Color32,
    fg_primary: Color32,
    fg_secondary: Color32,
    fg_muted: Color32,
    vine_green: Color32,
    leaf_green: Color32,
    accent_bg: Color32,
    fg_on_accent: Color32,
    border: Color32,
    ok_fg: Color32,
    ok_bg: Color32,
    warn_fg: Color32,
    warn_bg: Color32,
    err_fg: Color32,
    err_bg: Color32,
}

impl BrandPalette {
    const fn dark() -> Self {
        Self {
            bg_canvas: Color32::from_rgb(0x0A, 0x1A, 0x16),
            bg_sidebar: Color32::from_rgb(0x07, 0x13, 0x10),
            bg_card: Color32::from_rgb(0x10, 0x26, 0x20),
            bg_elevated: Color32::from_rgb(0x16, 0x33, 0x2B),
            fg_primary: Color32::from_rgb(0xE8, 0xF5, 0xF0),
            fg_secondary: Color32::from_rgb(0x9F, 0xC9, 0xBC),
            fg_muted: Color32::from_rgb(0x64, 0x8C, 0x80),
            vine_green: Color32::from_rgb(0x04, 0x4C, 0x3C),
            leaf_green: Color32::from_rgb(0x00, 0xA8, 0x7C),
            accent_bg: Color32::from_rgb(0x0E, 0x42, 0x35),
            fg_on_accent: Color32::from_rgb(0x04, 0x24, 0x1C),
            border: Color32::from_rgb(0x1E, 0x4D, 0x42),
            ok_fg: Color32::from_rgb(0x34, 0xD3, 0x99),
            ok_bg: Color32::from_rgb(0x0A, 0x36, 0x29),
            warn_fg: Color32::from_rgb(0xFB, 0xBF, 0x24),
            warn_bg: Color32::from_rgb(0x3B, 0x2A, 0x10),
            err_fg: Color32::from_rgb(0xFB, 0x71, 0x85),
            err_bg: Color32::from_rgb(0x3B, 0x12, 0x19),
        }
    }

    const fn light() -> Self {
        Self {
            bg_canvas: Color32::from_rgb(0xF4, 0xFB, 0xF8),
            bg_sidebar: Color32::from_rgb(0xE6, 0xF4, 0xEE),
            bg_card: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            bg_elevated: Color32::from_rgb(0xEB, 0xF7, 0xF2),
            fg_primary: Color32::from_rgb(0x07, 0x3B, 0x30),
            fg_secondary: Color32::from_rgb(0x2B, 0x63, 0x56),
            fg_muted: Color32::from_rgb(0x5F, 0x8A, 0x7E),
            vine_green: Color32::from_rgb(0x04, 0x4C, 0x3C),
            leaf_green: Color32::from_rgb(0x00, 0x8F, 0x68),
            accent_bg: Color32::from_rgb(0xD1, 0xF2, 0xE6),
            fg_on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            border: Color32::from_rgb(0xB8, 0xDC, 0xD1),
            ok_fg: Color32::from_rgb(0x04, 0x78, 0x57),
            ok_bg: Color32::from_rgb(0xD1, 0xFA, 0xE5),
            warn_fg: Color32::from_rgb(0xB4, 0x53, 0x09),
            warn_bg: Color32::from_rgb(0xFE, 0xF3, 0xC7),
            err_fg: Color32::from_rgb(0xBE, 0x12, 0x3C),
            err_bg: Color32::from_rgb(0xFF, 0xE4, 0xE6),
        }
    }
}

enum WorkerCommand {
    RefreshStatus,
    StartDaemon,
    StopDaemon,
    RestartDaemon,
    ReloadDaemon,
    SetAutostart(bool),
    ResolveConflict(AutostartConflict),
    RefreshDiagnostics,
}

enum WorkerEvent {
    StatusUpdated {
        snapshot: Box<ServiceSnapshot>,
        banner_ok: Option<String>,
        banner_err: Option<String>,
    },
    DiagnosticsUpdated(Box<DiagnosticsSnapshot>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActionEditorMode {
    Closed,
    Create,
    Edit(usize),
}

/// Interactive state for the Radivine Control Center (`radivine-control`).
pub struct ControlCenterApp {
    page: ControlPage,
    service_ctrl: ServiceController,
    instance_lock: Option<ControlInstanceLock>,
    worker_tx: Sender<WorkerCommand>,
    worker_rx: Receiver<WorkerEvent>,
    transition_in_flight: Option<DaemonHealthState>,
    service_snapshot: ServiceSnapshot,
    diagnostics_snapshot: Option<DiagnosticsSnapshot>,
    config_path: PathBuf,
    actions_path: PathBuf,
    config_disk_snapshot: DiskFileSnapshot,
    actions_disk_snapshot: DiskFileSnapshot,
    /// Last saved configuration loaded from or written to `config.toml`.
    pub saved_config: AppConfig,
    /// In-memory editable configuration draft.
    pub draft_config: AppConfig,
    /// Last saved custom actions loaded from or written to `actions.toml`.
    pub saved_actions: EditableActionsDoc,
    /// In-memory editable custom actions draft.
    pub draft_actions: EditableActionsDoc,
    actions_load_warnings: Vec<String>,
    config_conflict_pending: bool,
    actions_conflict_pending: bool,
    action_search_query: String,
    action_editor_mode: ActionEditorMode,
    editor_draft: EditableAction,
    editor_args_text: String,
    editor_extensions_text: String,
    editor_mime_text: String,
    editor_timeout_text: String,
    editor_errors: Vec<String>,
    pending_delete_index: Option<usize>,
    log_filter_severity: Option<LogSeverity>,
    log_search_query: String,
    redact_before_export: bool,
    exported_report_preview: Option<String>,
    banner_ok: Option<String>,
    banner_err: Option<String>,
    last_poll: Instant,
    fonts_installed: bool,
    app_icon_tex: Option<TextureHandle>,
    logo_horiz_tex: Option<TextureHandle>,
}

impl ControlCenterApp {
    /// Creates a new [`ControlCenterApp`] instance.
    #[must_use]
    pub fn new(
        initial_page: ControlPage,
        service_paths: ServicePaths,
        instance_lock: Option<ControlInstanceLock>,
        egui_ctx: Option<egui::Context>,
        allow_x11_probe: bool,
    ) -> Self {
        let config_path = service_paths
            .config_path
            .clone()
            .unwrap_or_else(AppConfig::default_path);
        let actions_path = AppConfig::actions_path_for(Some(&config_path));

        let config_disk_snapshot = DiskFileSnapshot::load(&config_path);
        let saved_config = AppConfig::load_or_default(Some(&config_path));
        let draft_config = saved_config.clone();

        let (saved_actions, actions_disk_snapshot, actions_load_warnings) =
            match EditableActionsDoc::load_with_snapshot(&actions_path) {
                Ok(tuple) => tuple,
                Err(err) => (
                    EditableActionsDoc::default(),
                    DiskFileSnapshot::load(&actions_path),
                    vec![err],
                ),
            };
        let draft_actions = saved_actions.clone();

        let service_ctrl = ServiceController::new(service_paths);
        let initial_snapshot = service_ctrl.query_snapshot(None);

        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>();
        let (ev_tx, ev_rx) = mpsc::channel::<WorkerEvent>();

        let worker_ctrl = service_ctrl.clone();
        let repaint_ctx = egui_ctx;
        let _ = thread::Builder::new()
            .name("radivine-control-worker".to_string())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    let ev = match cmd {
                        WorkerCommand::RefreshStatus => WorkerEvent::StatusUpdated {
                            snapshot: Box::new(worker_ctrl.query_snapshot(None)),
                            banner_ok: None,
                            banner_err: None,
                        },
                        WorkerCommand::StartDaemon => match worker_ctrl.start_daemon() {
                            Ok(snap) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(snap),
                                banner_ok: Some(
                                    "Radivine daemon is running and verified over IPC.".to_string(),
                                ),
                                banner_err: None,
                            },
                            Err(err) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(worker_ctrl.query_snapshot(Some(err.clone()))),
                                banner_ok: None,
                                banner_err: Some(err),
                            },
                        },
                        WorkerCommand::StopDaemon => match worker_ctrl.stop_daemon() {
                            Ok(snap) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(snap),
                                banner_ok: Some("Radivine daemon stopped cleanly.".to_string()),
                                banner_err: None,
                            },
                            Err(err) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(worker_ctrl.query_snapshot(Some(err.clone()))),
                                banner_ok: None,
                                banner_err: Some(err),
                            },
                        },
                        WorkerCommand::RestartDaemon => match worker_ctrl.restart_daemon() {
                            Ok(snap) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(snap),
                                banner_ok: Some(
                                    "Radivine daemon restarted and verified over IPC.".to_string(),
                                ),
                                banner_err: None,
                            },
                            Err(err) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(worker_ctrl.query_snapshot(Some(err.clone()))),
                                banner_ok: None,
                                banner_err: Some(err),
                            },
                        },
                        WorkerCommand::ReloadDaemon => match worker_ctrl.reload_daemon() {
                            Ok(snap) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(snap),
                                banner_ok: Some(
                                    "Reloaded configuration and custom actions in running daemon."
                                        .to_string(),
                                ),
                                banner_err: None,
                            },
                            Err(err) => WorkerEvent::StatusUpdated {
                                snapshot: Box::new(worker_ctrl.query_snapshot(None)),
                                banner_ok: None,
                                banner_err: Some(err),
                            },
                        },
                        WorkerCommand::SetAutostart(enable) => {
                            match worker_ctrl.set_autostart(enable) {
                                Ok(snap) => {
                                    let msg = if enable {
                                        "Enabled Launch Radivine at Login.".to_string()
                                    } else {
                                        "Disabled Launch Radivine at Login (running daemon unchanged)."
                                            .to_string()
                                    };
                                    WorkerEvent::StatusUpdated {
                                        snapshot: Box::new(snap),
                                        banner_ok: Some(msg),
                                        banner_err: None,
                                    }
                                }
                                Err(err) => WorkerEvent::StatusUpdated {
                                    snapshot: Box::new(worker_ctrl.query_snapshot(None)),
                                    banner_ok: None,
                                    banner_err: Some(err),
                                },
                            }
                        }
                        WorkerCommand::ResolveConflict(conflict) => {
                            match worker_ctrl.resolve_conflict(&conflict) {
                                Ok(snap) => WorkerEvent::StatusUpdated {
                                    snapshot: Box::new(snap),
                                    banner_ok: Some(
                                        "Resolved startup conflict and created .bak backup."
                                            .to_string(),
                                    ),
                                    banner_err: None,
                                },
                                Err(err) => WorkerEvent::StatusUpdated {
                                    snapshot: Box::new(worker_ctrl.query_snapshot(None)),
                                    banner_ok: None,
                                    banner_err: Some(err),
                                },
                            }
                        }
                        WorkerCommand::RefreshDiagnostics => {
                            let svc = worker_ctrl.query_snapshot(None);
                            let diag = collect_diagnostics_snapshot(
                                &svc,
                                &worker_ctrl.paths().daemon_log_path(),
                                allow_x11_probe,
                            );
                            WorkerEvent::DiagnosticsUpdated(Box::new(diag))
                        }
                    };
                    if ev_tx.send(ev).is_err() {
                        break;
                    }
                    if let Some(ref ctx) = repaint_ctx {
                        ctx.request_repaint();
                    }
                }
            });

        let _ = cmd_tx.send(WorkerCommand::RefreshDiagnostics);

        Self {
            page: initial_page,
            service_ctrl,
            instance_lock,
            worker_tx: cmd_tx,
            worker_rx: ev_rx,
            transition_in_flight: None,
            service_snapshot: initial_snapshot,
            diagnostics_snapshot: None,
            config_path,
            actions_path,
            config_disk_snapshot,
            actions_disk_snapshot,
            saved_config,
            draft_config,
            saved_actions,
            draft_actions,
            actions_load_warnings,
            config_conflict_pending: false,
            actions_conflict_pending: false,
            action_search_query: String::new(),
            action_editor_mode: ActionEditorMode::Closed,
            editor_draft: EditableAction::default(),
            editor_args_text: String::new(),
            editor_extensions_text: String::new(),
            editor_mime_text: String::new(),
            editor_timeout_text: String::new(),
            editor_errors: Vec::new(),
            pending_delete_index: None,
            log_filter_severity: None,
            log_search_query: String::new(),
            redact_before_export: true,
            exported_report_preview: None,
            banner_ok: None,
            banner_err: None,
            last_poll: Instant::now(),
            fonts_installed: false,
            app_icon_tex: None,
            logo_horiz_tex: None,
        }
    }

    /// Switches the active sidebar page.
    pub fn set_page(&mut self, page: ControlPage) {
        self.page = page;
        if page == ControlPage::Diagnostics && self.diagnostics_snapshot.is_none() {
            let _ = self.worker_tx.send(WorkerCommand::RefreshDiagnostics);
        }
    }

    /// Saves `draft_config` to `config.toml` atomically, handling external modification conflicts.
    pub fn save_config(&mut self, force_overwrite_with_backup: bool) -> Result<(), String> {
        if !force_overwrite_with_backup && self.config_disk_snapshot.has_external_conflict() {
            self.config_conflict_pending = true;
            let msg = format!(
                "{} was modified outside Control Center. Choose 'Reload from Disk' or 'Back Up & Overwrite'.",
                self.config_path.display()
            );
            self.banner_err = Some(msg.clone());
            return Err(msg);
        }

        let updated_toml =
            apply_app_config_to_toml_doc(&self.config_disk_snapshot.raw_text, &self.draft_config)
                .map_err(|errs| {
                let msg = errs.join(" · ");
                self.banner_err = Some(msg.clone());
                msg
            })?;

        let new_snap = atomic_write_with_backup(
            &self.config_path,
            &updated_toml,
            force_overwrite_with_backup,
        )
        .map_err(|e| {
            let msg = format!("Failed to write {}: {e}", self.config_path.display());
            self.banner_err = Some(msg.clone());
            msg
        })?;

        self.config_disk_snapshot = new_snap;
        self.saved_config = self.draft_config.clone();
        self.config_conflict_pending = false;
        self.banner_err = None;

        if self.service_snapshot.health == DaemonHealthState::Running {
            let _ = self.worker_tx.send(WorkerCommand::ReloadDaemon);
            self.banner_ok = Some(
                "Saved config.toml and triggered hot reload in the running daemon.".to_string(),
            );
        } else {
            self.banner_ok = Some(
                "Saved config.toml. Settings will apply automatically when the daemon starts."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Saves `draft_actions` to `actions.toml` atomically, handling external modification conflicts.
    pub fn save_actions(&mut self, force_overwrite_with_backup: bool) -> Result<(), String> {
        if !force_overwrite_with_backup && self.actions_disk_snapshot.has_external_conflict() {
            self.actions_conflict_pending = true;
            let msg = format!(
                "{} was modified outside Control Center. Choose 'Reload from Disk' or 'Back Up & Overwrite'.",
                self.actions_path.display()
            );
            self.banner_err = Some(msg.clone());
            return Err(msg);
        }

        let updated_toml =
            apply_actions_to_toml_doc(&self.actions_disk_snapshot.raw_text, &self.draft_actions)
                .map_err(|errs| {
                    let msg = errs.join(" · ");
                    self.banner_err = Some(msg.clone());
                    msg
                })?;

        let new_snap = atomic_write_with_backup(
            &self.actions_path,
            &updated_toml,
            force_overwrite_with_backup,
        )
        .map_err(|e| {
            let msg = format!("Failed to write {}: {e}", self.actions_path.display());
            self.banner_err = Some(msg.clone());
            msg
        })?;

        self.actions_disk_snapshot = new_snap;
        self.saved_actions = self.draft_actions.clone();
        self.actions_conflict_pending = false;
        self.actions_load_warnings.clear();
        self.banner_err = None;

        if self.service_snapshot.health == DaemonHealthState::Running {
            let _ = self.worker_tx.send(WorkerCommand::ReloadDaemon);
            self.banner_ok = Some(
                "Saved actions.toml and triggered hot reload in the running daemon.".to_string(),
            );
        } else {
            self.banner_ok = Some(
                "Saved actions.toml. Custom actions will load on next daemon start.".to_string(),
            );
        }
        Ok(())
    }

    /// Reloads both `config.toml` and `actions.toml` from disk into the Control Center.
    pub fn reload_files_from_disk(&mut self) {
        self.config_disk_snapshot = DiskFileSnapshot::load(&self.config_path);
        self.saved_config = AppConfig::load_or_default(Some(&self.config_path));
        self.draft_config = self.saved_config.clone();
        self.config_conflict_pending = false;

        match EditableActionsDoc::load_with_snapshot(&self.actions_path) {
            Ok((doc, snap, warnings)) => {
                self.saved_actions = doc.clone();
                self.draft_actions = doc;
                self.actions_disk_snapshot = snap;
                self.actions_load_warnings = warnings;
                self.actions_conflict_pending = false;
                self.banner_err = None;
                self.banner_ok =
                    Some("Reloaded config.toml and actions.toml from disk.".to_string());
            }
            Err(err) => {
                self.banner_err = Some(err);
            }
        }
    }

    fn drain_worker_events(&mut self) {
        while let Ok(ev) = self.worker_rx.try_recv() {
            match ev {
                WorkerEvent::StatusUpdated {
                    snapshot,
                    banner_ok,
                    banner_err,
                } => {
                    self.transition_in_flight = None;
                    self.service_snapshot = *snapshot;
                    if let Some(ok) = banner_ok {
                        self.banner_ok = Some(ok);
                        self.banner_err = None;
                    }
                    if let Some(err) = banner_err {
                        self.banner_err = Some(err);
                    }
                }
                WorkerEvent::DiagnosticsUpdated(diag) => {
                    self.diagnostics_snapshot = Some(*diag);
                }
            }
        }
    }

    fn ensure_fonts_and_textures(&mut self, ctx: &egui::Context) {
        if !self.fonts_installed {
            let mut fonts = FontDefinitions::default();
            fonts.font_data.insert(
                "Inter".to_owned(),
                Arc::new(FontData::from_static(EMBEDDED_INTER_TTF)),
            );
            if let Some(prop) = fonts.families.get_mut(&FontFamily::Proportional) {
                prop.insert(0, "Inter".to_owned());
            }
            ctx.set_fonts(fonts);
            self.fonts_installed = true;
        }
        if self.app_icon_tex.is_none()
            && let Some(img) = decode_png_to_color_image(EMBEDDED_APP_ICON_PNG, 128)
        {
            self.app_icon_tex =
                Some(ctx.load_texture("radivine-app-icon", img, TextureOptions::LINEAR));
        }
        if self.logo_horiz_tex.is_none()
            && let Some(img) = decode_png_to_color_image(EMBEDDED_LOGO_HORIZONTAL_PNG, 380)
        {
            self.logo_horiz_tex =
                Some(ctx.load_texture("radivine-logo-horiz", img, TextureOptions::LINEAR));
        }
    }

    fn is_dark_theme(&self) -> bool {
        !matches!(
            self.draft_config
                .appearance
                .theme
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "light"
        )
    }

    fn apply_visuals(&self, ctx: &egui::Context, pal: &BrandPalette) {
        let mut visuals = if self.is_dark_theme() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.panel_fill = pal.bg_canvas;
        visuals.window_fill = pal.bg_card;
        visuals.extreme_bg_color = pal.bg_sidebar;
        visuals.faint_bg_color = pal.bg_elevated;
        visuals.override_text_color = Some(pal.fg_primary);
        visuals.widgets.noninteractive.bg_fill = pal.bg_card;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.fg_primary);
        visuals.widgets.inactive.bg_fill = pal.bg_elevated;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.border);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.fg_primary);
        visuals.widgets.hovered.bg_fill = pal.accent_bg;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.5, pal.leaf_green);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.fg_primary);
        visuals.widgets.open.bg_fill = pal.bg_elevated;
        visuals.widgets.open.fg_stroke = Stroke::new(1.0, pal.fg_primary);
        visuals.widgets.active.bg_fill = pal.leaf_green;
        visuals.widgets.active.fg_stroke = Stroke::new(1.5, pal.fg_on_accent);
        visuals.selection.bg_fill = pal.leaf_green;
        visuals.selection.stroke = Stroke::new(1.0, pal.fg_on_accent);
        ctx.set_visuals(visuals);
    }

    /// Renders the complete Control Center frame into `ui`.
    pub fn render_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.ensure_fonts_and_textures(&ctx);
        self.drain_worker_events();

        if let Some(ref lock) = self.instance_lock
            && let Some(req_page) = lock.poll_activation()
        {
            if let Some(p) = req_page {
                self.set_page(p);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        if self.last_poll.elapsed() >= Duration::from_millis(2500) {
            self.last_poll = Instant::now();
            let _ = self.worker_tx.send(WorkerCommand::RefreshStatus);
        }

        let pal = if self.is_dark_theme() {
            BrandPalette::dark()
        } else {
            BrandPalette::light()
        };
        self.apply_visuals(&ctx, &pal);

        egui::Panel::left("control_sidebar")
            .resizable(false)
            .exact_size(236.0)
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg_sidebar)
                    .inner_margin(Margin::symmetric(14, 16))
                    .stroke(Stroke::new(1.0, pal.border)),
            )
            .show(ui, |ui| {
                self.render_sidebar(ui, &pal);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg_canvas)
                    .inner_margin(Margin::symmetric(24, 20)),
            )
            .show(ui, |ui| {
                self.render_page_header(ui, &pal);
                ui.add_space(10.0);
                self.render_banners(ui, &pal);

                ScrollArea::vertical()
                    .id_salt(self.page.slug())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.page {
                            ControlPage::Dashboard => self.render_dashboard_page(ui, &pal),
                            ControlPage::General => self.render_general_page(ui, &pal),
                            ControlPage::Wheel => self.render_wheel_page(ui, &pal),
                            ControlPage::Actions => self.render_actions_page(ui, &pal),
                            ControlPage::Diagnostics => {
                                self.render_diagnostics_page(ui, &ctx, &pal)
                            }
                            ControlPage::About => self.render_about_page(ui, &pal),
                        }
                        ui.add_space(24.0);
                    });
            });
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        ui.horizontal(|ui| {
            if let Some(ref tex) = self.app_icon_tex {
                ui.add(
                    egui::Image::from_texture(tex)
                        .fit_to_exact_size(Vec2::new(34.0, 34.0))
                        .corner_radius(6.0),
                );
                ui.add_space(6.0);
            }
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("RADIVINE")
                        .font(FontId::new(16.0, FontFamily::Proportional))
                        .strong()
                        .color(pal.fg_primary),
                );
                ui.label(
                    RichText::new(format!("Control Center · v{}", env!("CARGO_PKG_VERSION")))
                        .size(11.5)
                        .color(pal.fg_secondary),
                );
            });
        });

        ui.add_space(14.0);

        // Live daemon status badge card in sidebar
        let effective_health = self
            .transition_in_flight
            .unwrap_or(self.service_snapshot.health);
        let (badge_fg, badge_bg, dot) = health_colors(effective_health, pal);

        egui::Frame::NONE
            .fill(badge_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 8))
            .stroke(Stroke::new(1.0, badge_fg))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(dot).color(badge_fg).size(13.0).strong());
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("Daemon: {}", effective_health.label()))
                                .color(badge_fg)
                                .size(12.5)
                                .strong(),
                        );
                        if let Some(ref d) = self.service_snapshot.daemon {
                            ui.label(
                                RichText::new(format!(
                                    "PID {} · State: {}",
                                    d.pid, d.session_state
                                ))
                                .color(pal.fg_secondary)
                                .size(11.0),
                            );
                        } else {
                            ui.label(
                                RichText::new(&self.service_snapshot.management_mode)
                                    .color(pal.fg_secondary)
                                    .size(11.0),
                            );
                        }
                    });
                });
            });

        ui.add_space(16.0);
        ui.label(
            RichText::new("NAVIGATION")
                .size(10.5)
                .strong()
                .color(pal.fg_muted),
        );
        ui.add_space(4.0);

        for item in ControlPage::ALL {
            let selected = self.page == item;
            let fill = if selected {
                pal.leaf_green
            } else {
                Color32::TRANSPARENT
            };
            let fg = if selected {
                pal.fg_on_accent
            } else {
                pal.fg_primary
            };

            let stroke = if selected {
                Stroke::new(1.0, pal.vine_green)
            } else {
                Stroke::NONE
            };

            let resp = egui::Frame::NONE
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut label_text = item.title().to_string();
                    let has_unsaved = ((item == ControlPage::General
                        || item == ControlPage::Wheel)
                        && self.draft_config != self.saved_config)
                        || (item == ControlPage::Actions
                            && self.draft_actions != self.saved_actions);
                    if has_unsaved {
                        label_text.push_str(" •");
                    }
                    ui.add(
                        egui::Label::new(RichText::new(label_text).size(13.5).strong().color(fg))
                            .selectable(false),
                    )
                });
            if resp.response.interact(egui::Sense::click()).clicked()
                || resp.inner.interact(egui::Sense::click()).clicked()
            {
                self.set_page(item);
            }
            ui.add_space(3.0);
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.label(
                RichText::new("Closing this window leaves the Radivine daemon running.")
                    .size(11.0)
                    .color(pal.fg_muted),
            );
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Theme:").size(12.0).color(pal.fg_secondary));
                for (code, label) in [("dark", "Dark"), ("light", "Light"), ("auto", "Auto")] {
                    let active = self
                        .draft_config
                        .appearance
                        .theme
                        .eq_ignore_ascii_case(code);
                    if ui.selectable_label(active, label).clicked() {
                        self.draft_config.appearance.theme = code.to_string();
                    }
                }
            });
        });
    }

    fn render_page_header(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(self.page.title())
                        .font(FontId::new(22.0, FontFamily::Proportional))
                        .strong()
                        .color(pal.fg_primary),
                );
                ui.label(
                    RichText::new(self.page.subtitle())
                        .size(13.0)
                        .color(pal.fg_secondary),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Refresh Status").clicked() {
                    let _ = self.worker_tx.send(WorkerCommand::RefreshStatus);
                    if self.page == ControlPage::Diagnostics {
                        let _ = self.worker_tx.send(WorkerCommand::RefreshDiagnostics);
                    }
                }
            });
        });
        ui.add_space(6.0);
        ui.separator();
    }

    fn render_banners(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        if self.config_conflict_pending {
            egui::Frame::NONE
                .fill(pal.warn_bg)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0, pal.warn_fg))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(
                                "External edit detected on config.toml since it was loaded.",
                            )
                            .color(pal.warn_fg)
                            .strong(),
                        );
                        if ui.button("Reload from Disk").clicked() {
                            self.reload_files_from_disk();
                        }
                        if ui.button("Back Up (.bak) & Overwrite").clicked() {
                            let _ = self.save_config(true);
                        }
                    });
                });
            ui.add_space(8.0);
        }

        if self.actions_conflict_pending {
            egui::Frame::NONE
                .fill(pal.warn_bg)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0, pal.warn_fg))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(
                                "External edit detected on actions.toml since it was loaded.",
                            )
                            .color(pal.warn_fg)
                            .strong(),
                        );
                        if ui.button("Reload from Disk").clicked() {
                            self.reload_files_from_disk();
                        }
                        if ui.button("Back Up (.bak) & Overwrite").clicked() {
                            let _ = self.save_actions(true);
                        }
                    });
                });
            ui.add_space(8.0);
        }

        let mut dismiss_err = false;
        if let Some(ref err) = self.banner_err {
            egui::Frame::NONE
                .fill(pal.err_bg)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0, pal.err_fg))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Error: {err}")).color(pal.err_fg));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Dismiss").clicked() {
                                dismiss_err = true;
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
        if dismiss_err {
            self.banner_err = None;
        }

        let mut dismiss_ok = false;
        if let Some(ref ok) = self.banner_ok {
            egui::Frame::NONE
                .fill(pal.ok_bg)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0, pal.ok_fg))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ok).color(pal.ok_fg));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Dismiss").clicked() {
                                dismiss_ok = true;
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
        if dismiss_ok {
            self.banner_ok = None;
        }
    }

    fn render_dashboard_page(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        let effective_health = self
            .transition_in_flight
            .unwrap_or(self.service_snapshot.health);
        let busy = self.transition_in_flight.is_some()
            || matches!(
                effective_health,
                DaemonHealthState::Starting | DaemonHealthState::Stopping
            );

        // 1. Service Status & Lifecycle Controls Card
        section_card(ui, pal, "Background Daemon Status & Controls", |ui| {
            let (badge_fg, badge_bg, dot) = health_colors(effective_health, pal);
            ui.horizontal(|ui| {
                egui::Frame::NONE
                    .fill(badge_bg)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 6))
                    .stroke(Stroke::new(1.0, badge_fg))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{dot}  {}", effective_health.label()))
                                .color(badge_fg)
                                .size(14.5)
                                .strong(),
                        );
                    });

                ui.add_space(12.0);

                let can_start = !busy && effective_health != DaemonHealthState::Running;
                let can_stop = !busy && effective_health != DaemonHealthState::Stopped;
                let can_restart = !busy;

                if ui
                    .add_enabled(can_start, egui::Button::new("  Start Daemon  "))
                    .clicked()
                {
                    self.transition_in_flight = Some(DaemonHealthState::Starting);
                    let _ = self.worker_tx.send(WorkerCommand::StartDaemon);
                }

                if ui
                    .add_enabled(can_stop, egui::Button::new("  Stop Daemon  "))
                    .clicked()
                {
                    self.transition_in_flight = Some(DaemonHealthState::Stopping);
                    let _ = self.worker_tx.send(WorkerCommand::StopDaemon);
                }

                if ui
                    .add_enabled(can_restart, egui::Button::new("  Restart Daemon  "))
                    .clicked()
                {
                    self.transition_in_flight = Some(DaemonHealthState::Starting);
                    let _ = self.worker_tx.send(WorkerCommand::RestartDaemon);
                }

                if ui
                    .add_enabled(
                        effective_health == DaemonHealthState::Running && !busy,
                        egui::Button::new("Reload Config"),
                    )
                    .clicked()
                {
                    let _ = self.worker_tx.send(WorkerCommand::ReloadDaemon);
                }
            });

            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "Note: Stopping or restarting the daemon dismisses any currently open radial wheel session without modifying your files. \
                     Closing the Control Center window leaves the daemon running in the background.",
                )
                .size(12.0)
                .color(pal.fg_secondary),
            );
        });

        ui.add_space(12.0);

        // 2. Runtime Telemetry Grid Card
        section_card(ui, pal, "Runtime Information & Telemetry", |ui| {
            let d_opt = self.service_snapshot.daemon.as_ref();
            egui::Grid::new("dashboard_telemetry_grid")
                .num_columns(4)
                .spacing([16.0, 9.0])
                .striped(false)
                .show(ui, |ui| {
                    kv_cell(
                        ui,
                        pal,
                        "Radivine Version",
                        &format!(
                            "v{}",
                            d_opt
                                .filter(|d| !d.version.is_empty())
                                .map_or(env!("CARGO_PKG_VERSION"), |d| d.version.as_str())
                        ),
                    );
                    kv_cell(
                        ui,
                        pal,
                        "Daemon Process ID",
                        &d_opt.map_or_else(|| "Not running".to_string(), |d| d.pid.to_string()),
                    );
                    ui.end_row();

                    kv_cell(
                        ui,
                        pal,
                        "Uptime",
                        &d_opt.map_or_else(|| "—".to_string(), |d| format_uptime(d.uptime_secs)),
                    );
                    kv_cell(
                        ui,
                        pal,
                        "Resident Memory (VmRSS)",
                        &d_opt.and_then(|d| d.rss_kib).map_or_else(
                            || "—".to_string(),
                            |kib| format!("{:.2} MiB ({kib} KiB)", kib as f64 / 1024.0),
                        ),
                    );
                    ui.end_row();

                    kv_cell(
                        ui,
                        pal,
                        "Desktop Session Type",
                        &self.service_snapshot.session_type,
                    );
                    kv_cell(
                        ui,
                        pal,
                        "Desktop Environment",
                        &self.service_snapshot.desktop_environment,
                    );
                    ui.end_row();

                    kv_cell(
                        ui,
                        pal,
                        "Startup Management Mode",
                        &self.service_snapshot.management_mode,
                    );
                    kv_cell(
                        ui,
                        pal,
                        "Launch at Login (Autostart)",
                        if self.service_snapshot.autostart.enabled {
                            "Enabled"
                        } else {
                            "Disabled"
                        },
                    );
                    ui.end_row();

                    kv_cell(
                        ui,
                        pal,
                        "IPC Socket Connectivity",
                        if d_opt.is_some() {
                            "Connected (Responsive)"
                        } else {
                            "Disconnected"
                        },
                    );
                    kv_cell(
                        ui,
                        pal,
                        "Session State / Scale",
                        &d_opt.map_or_else(
                            || "Idle".to_string(),
                            |d| {
                                format!(
                                    "{} · {:.2}× (Drag: {}, Invoke: {})",
                                    d.session_state, d.scale, d.drag_session_count, d.invoke_count
                                )
                            },
                        ),
                    );
                    ui.end_row();

                    kv_cell(
                        ui,
                        pal,
                        "Loaded Custom Actions",
                        &d_opt.map_or_else(
                            || format!("{} in actions.toml", self.draft_actions.actions.len()),
                            |d| format!("{} active", d.custom_action_count),
                        ),
                    );
                    kv_cell(
                        ui,
                        pal,
                        "Last Frame / Summon Latency",
                        &d_opt.map_or_else(
                            || "—".to_string(),
                            |d| {
                                let render = d.last_frame_render_us.map_or_else(
                                    || "—".to_string(),
                                    |us| format!("{:.2} ms", us as f64 / 1000.0),
                                );
                                let summon = d.last_summon_us.map_or_else(
                                    || "—".to_string(),
                                    |us| format!("{:.2} ms", us as f64 / 1000.0),
                                );
                                format!("Render: {render} · Summon: {summon}")
                            },
                        ),
                    );
                    ui.end_row();
                });
        });

        ui.add_space(12.0);

        // 3. Health & Quick Actions Card
        section_card(ui, pal, "Health Summary & Quick Actions", |ui| {
            if let Some(ref err) = self.service_snapshot.last_error {
                ui.label(
                    RichText::new(format!("Last reported warning/error: {err}"))
                        .color(pal.warn_fg)
                        .strong(),
                );
                ui.add_space(6.0);
            } else {
                ui.label(RichText::new("No daemon or action errors reported.").color(pal.ok_fg));
                ui.add_space(6.0);
            }

            for conflict in self.service_snapshot.autostart.conflicts.clone() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(conflict.description()).color(pal.warn_fg));
                    if ui.button("Resolve Conflict Safely").clicked() {
                        let _ = self
                            .worker_tx
                            .send(WorkerCommand::ResolveConflict(conflict));
                    }
                });
                ui.add_space(4.0);
            }

            ui.horizontal_wrapped(|ui| {
                let mut login_enabled = self.service_snapshot.autostart.enabled;
                if ui
                    .checkbox(
                        &mut login_enabled,
                        "Launch Radivine automatically at desktop login",
                    )
                    .changed()
                {
                    let _ = self
                        .worker_tx
                        .send(WorkerCommand::SetAutostart(login_enabled));
                }
                ui.add_space(16.0);
                if ui.button("Manage Custom Actions →").clicked() {
                    self.set_page(ControlPage::Actions);
                }
                if ui.button("Open Diagnostics & Logs →").clicked() {
                    self.set_page(ControlPage::Diagnostics);
                }
            });
        });
    }

    fn render_general_page(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        // 1. Startup & Autostart Card
        section_card(ui, pal, "Startup & Login Preferences", |ui| {
            let mut autostart_en = self.service_snapshot.autostart.enabled;
            if ui
                .checkbox(
                    &mut autostart_en,
                    "Launch Radivine at Login (starts background daemon automatically when you log in)",
                )
                .changed()
            {
                let _ = self.worker_tx.send(WorkerCommand::SetAutostart(autostart_en));
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "Startup method: {} · Disabling autostart prevents future login launches without stopping the currently running daemon.",
                    self.service_snapshot.autostart.method_label
                ))
                .size(12.0)
                .color(pal.fg_secondary),
            );

            for conflict in self.service_snapshot.autostart.conflicts.clone() {
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(conflict.description()).color(pal.warn_fg));
                    if ui.button("Resolve Conflict").clicked() {
                        let _ = self
                            .worker_tx
                            .send(WorkerCommand::ResolveConflict(conflict));
                    }
                });
            }

            if self.service_snapshot.health != DaemonHealthState::Running {
                ui.add_space(6.0);
                if ui.button("Start Radivine Daemon Now").clicked() {
                    self.transition_in_flight = Some(DaemonHealthState::Starting);
                    let _ = self.worker_tx.send(WorkerCommand::StartDaemon);
                }
            }
        });

        ui.add_space(12.0);

        // 2. Appearance Card
        section_card(ui, pal, "Appearance & Scaling", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Color Theme:")
                        .strong()
                        .color(pal.fg_secondary),
                );
                for (code, label) in [
                    ("dark", "Dark (Vine & Leaf)"),
                    ("light", "Light (Mint Porcelain)"),
                    ("auto", "Follow System Theme"),
                ] {
                    let selected = self
                        .draft_config
                        .appearance
                        .theme
                        .eq_ignore_ascii_case(code);
                    if ui.selectable_label(selected, label).clicked() {
                        self.draft_config.appearance.theme = code.to_string();
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("UI Scale Factor:")
                        .strong()
                        .color(pal.fg_secondary),
                );
                let is_auto = matches!(self.draft_config.appearance.scale, ScaleSetting::Auto);
                if ui.selectable_label(is_auto, "Auto (Xft.dpi)").clicked() {
                    self.draft_config.appearance.scale = ScaleSetting::Auto;
                }
                let mut fixed_val = match self.draft_config.appearance.scale {
                    ScaleSetting::Auto => 1.0,
                    ScaleSetting::Fixed(v) => v,
                };
                if ui.selectable_label(!is_auto, "Custom Scale").clicked() && is_auto {
                    self.draft_config.appearance.scale = ScaleSetting::Fixed(1.25);
                }
                if !is_auto
                    && ui
                        .add(
                            egui::Slider::new(&mut fixed_val, 0.5..=4.0)
                                .step_by(0.05)
                                .suffix("×"),
                        )
                        .changed()
                {
                    self.draft_config.appearance.scale = ScaleSetting::Fixed(fixed_val);
                }
            });
        });

        ui.add_space(12.0);

        // 3. Notifications Card
        section_card(ui, pal, "Desktop Notifications", |ui| {
            ui.checkbox(
                &mut self.draft_config.notifications.enabled,
                "Enable desktop notifications (org.freedesktop.Notifications)",
            );
            ui.add_enabled_ui(self.draft_config.notifications.enabled, |ui| {
                ui.checkbox(
                    &mut self.draft_config.notifications.notify_on_action_error,
                    "Notify when an action or custom command fails",
                );
                ui.checkbox(
                    &mut self.draft_config.notifications.notify_on_config_warning,
                    "Notify when actions.toml has syntax or validation warnings on reload",
                );
            });
        });

        ui.add_space(12.0);

        // 4. File Preview Defaults Card
        section_card(ui, pal, "File Preview Defaults (radivine-viewer)", |ui| {
            ui.label(
                RichText::new(
                    "Configure the default number of lines shown by 'File Preview -> Head' and 'File Preview -> Tail' (1..=10,000).",
                )
                .size(12.0)
                .color(pal.fg_secondary),
            );
            ui.add_space(6.0);
            egui::Grid::new("preview_lines_grid")
                .num_columns(2)
                .spacing([20.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Head preview line count:");
                    ui.add(
                        egui::DragValue::new(&mut self.draft_config.preview.head_lines)
                            .range(1..=10_000)
                            .suffix(" lines"),
                    );
                    ui.end_row();

                    ui.label("Tail preview line count:");
                    ui.add(
                        egui::DragValue::new(&mut self.draft_config.preview.tail_lines)
                            .range(1..=10_000)
                            .suffix(" lines"),
                    );
                    ui.end_row();
                });
        });

        ui.add_space(12.0);

        // 5. Configuration Management Card
        section_card(ui, pal, "Configuration Storage & Actions", |ui| {
            ui.label(
                RichText::new(format!(
                    "Settings file: {}\nCustom actions file: {}",
                    self.config_path.display(),
                    self.actions_path.display()
                ))
                .size(12.0)
                .color(pal.fg_secondary),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let has_unsaved = self.draft_config != self.saved_config;
                if ui
                    .add_enabled(has_unsaved, egui::Button::new("  Save General Settings  "))
                    .clicked()
                {
                    let _ = self.save_config(false);
                }
                if ui
                    .add_enabled(has_unsaved, egui::Button::new("Discard Unsaved Changes"))
                    .clicked()
                {
                    self.draft_config = self.saved_config.clone();
                }
                if ui.button("Open Config Folder").clicked()
                    && let Some(parent) = self.config_path.parent()
                {
                    let _ = std::fs::create_dir_all(parent);
                    let _ = Command::new("xdg-open")
                        .arg(parent)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn();
                }
                if ui.button("Reload from Disk").clicked() {
                    self.reload_files_from_disk();
                }
                if ui.button("Restore General Defaults").clicked() {
                    let def = AppConfig::default();
                    self.draft_config.appearance = def.appearance;
                    self.draft_config.notifications = def.notifications;
                    self.draft_config.preview = def.preview;
                }
            });
        });
    }

    fn render_wheel_page(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(
            ui,
            pal,
            "Branching Radial Wheel Geometry (Logical Pixels)",
            |ui| {
                ui.label(
                    RichText::new(
                        "Adjust the inner dead-zone and outer ring radii of the root wheel and adjacent branching child wheels. \
                         Changes apply via hot reload without restarting the daemon or interrupting active drag sessions.",
                    )
                    .size(12.0)
                    .color(pal.fg_secondary),
                );
                ui.add_space(8.0);

                egui::Grid::new("wheel_radii_grid")
                    .num_columns(2)
                    .spacing([24.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Root wheel inner radius (dead-zone):");
                        ui.add(
                            egui::Slider::new(
                                &mut self.draft_config.wheel.root_inner_radius,
                                20.0..=120.0,
                            )
                            .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Root wheel outer radius:");
                        ui.add(
                            egui::Slider::new(
                                &mut self.draft_config.wheel.root_outer_radius,
                                80.0..=260.0,
                            )
                            .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Child submenu inner radius:");
                        ui.add(
                            egui::Slider::new(
                                &mut self.draft_config.wheel.child_inner_radius,
                                20.0..=100.0,
                            )
                            .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Child submenu outer radius:");
                        ui.add(
                            egui::Slider::new(
                                &mut self.draft_config.wheel.child_outer_radius,
                                70.0..=220.0,
                            )
                            .suffix(" px"),
                        );
                        ui.end_row();

                        ui.label("Parent-to-child radial gap:");
                        ui.add(
                            egui::Slider::new(
                                &mut self.draft_config.wheel.child_offset_gap,
                                4.0..=80.0,
                            )
                            .suffix(" px"),
                        );
                        ui.end_row();
                    });
            },
        );

        ui.add_space(12.0);

        section_card(ui, pal, "Interaction Timing, Motion & Hysteresis", |ui| {
            egui::Grid::new("wheel_timing_grid")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Submenu hover dwell time:");
                    ui.add(
                        egui::Slider::new(&mut self.draft_config.wheel.hover_dwell_ms, 30..=600)
                            .suffix(" ms"),
                    );
                    ui.end_row();

                    ui.label("Diagonal corridor grace period:");
                    ui.add(
                        egui::Slider::new(&mut self.draft_config.wheel.submenu_grace_ms, 50..=800)
                            .suffix(" ms"),
                    );
                    ui.end_row();

                    ui.label("Outside-cluster leave grace period:");
                    ui.add(
                        egui::Slider::new(&mut self.draft_config.wheel.leave_grace_ms, 50..=1500)
                            .suffix(" ms"),
                    );
                    ui.end_row();

                    ui.label("Segment boundary angular hysteresis:");
                    ui.add(
                        egui::Slider::new(
                            &mut self.draft_config.wheel.angular_hysteresis_deg,
                            0.5..=12.0,
                        )
                        .suffix("°"),
                    );
                    ui.end_row();

                    ui.label("Monitor backdrop dim opacity:");
                    ui.add(
                        egui::Slider::new(
                            &mut self.draft_config.appearance.backdrop_dim,
                            0.0..=0.5,
                        )
                        .step_by(0.02),
                    );
                    ui.end_row();

                    ui.label("Reduced Motion (Animations):");
                    ui.horizontal(|ui| {
                        for (val, label) in [
                            (ReducedMotionSetting::Auto, "Auto (Follow Desktop)"),
                            (ReducedMotionSetting::Fixed(false), "Animations Enabled"),
                            (ReducedMotionSetting::Fixed(true), "Reduced Motion"),
                        ] {
                            if ui
                                .selectable_label(
                                    self.draft_config.appearance.reduced_motion == val,
                                    label,
                                )
                                .clicked()
                            {
                                self.draft_config.appearance.reduced_motion = val;
                            }
                        }
                    });
                    ui.end_row();
                });
        });

        ui.add_space(12.0);

        section_card(ui, pal, "Mid-Drag Trigger & Input Safety", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Drag Trigger Modifier Key:")
                        .strong()
                        .color(pal.fg_secondary),
                );
                for (code, label) in [
                    ("shift", "Shift (Recommended)"),
                    ("ctrl", "Ctrl"),
                    ("alt", "Alt"),
                    ("super", "Super"),
                    ("disabled", "Disabled (Invoke Only)"),
                ] {
                    let active = self
                        .draft_config
                        .trigger
                        .drag_modifier
                        .eq_ignore_ascii_case(code);
                    if ui.selectable_label(active, label).clicked() {
                        self.draft_config.trigger.drag_modifier = code.to_string();
                    }
                }
            });

            ui.add_space(6.0);
            ui.checkbox(
                &mut self.draft_config.trigger.toggle_off_on_repress,
                "Press trigger key a second time mid-drag to hide the wheel and continue native drag",
            );
            ui.checkbox(
                &mut self.draft_config.trigger.hold_to_keep_open,
                "Hold-to-keep-open mode (dismiss wheel when trigger key is released)",
            );
            ui.checkbox(
                &mut self.draft_config.input.passthrough,
                "Allow outside-wheel pointer pass-through during drag (Default: off for drop safety — ADR-0001)",
            );

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let has_unsaved = self.draft_config != self.saved_config;
                if ui
                    .add_enabled(has_unsaved, egui::Button::new("  Save Wheel Settings  "))
                    .clicked()
                {
                    let _ = self.save_config(false);
                }
                if ui
                    .add_enabled(has_unsaved, egui::Button::new("Discard Changes"))
                    .clicked()
                {
                    self.draft_config = self.saved_config.clone();
                }
                if ui.button("Restore Wheel Defaults").clicked() {
                    let def = AppConfig::default();
                    self.draft_config.wheel = def.wheel;
                    self.draft_config.trigger = def.trigger;
                    self.draft_config.input = def.input;
                }
            });
        });
    }

    fn render_actions_page(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        if !self.actions_load_warnings.is_empty() {
            egui::Frame::NONE
                .fill(pal.warn_bg)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 8))
                .stroke(Stroke::new(1.0, pal.warn_fg))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Warnings in existing actions.toml:")
                            .color(pal.warn_fg)
                            .strong(),
                    );
                    for w in &self.actions_load_warnings {
                        ui.label(
                            RichText::new(format!("• {w}"))
                                .color(pal.warn_fg)
                                .size(12.0),
                        );
                    }
                });
            ui.add_space(8.0);
        }

        // Delete confirmation bar
        if let Some(del_idx) = self.pending_delete_index {
            if let Some(target_act) = self.draft_actions.actions.get(del_idx) {
                let target_label = target_act.label.clone();
                let target_id = target_act.canonical_id();
                egui::Frame::NONE
                    .fill(pal.err_bg)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .stroke(Stroke::new(1.0, pal.err_fg))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Confirm deletion of custom action '{target_label}' ({target_id})?"
                                ))
                                .color(pal.err_fg)
                                .strong(),
                            );
                            if ui.button("Confirm Delete").clicked() {
                                self.draft_actions.actions.remove(del_idx);
                                self.pending_delete_index = None;
                                self.action_editor_mode = ActionEditorMode::Closed;
                                let _ = self.save_actions(false);
                            }
                            if ui.button("Cancel").clicked() {
                                self.pending_delete_index = None;
                            }
                        });
                    });
                ui.add_space(8.0);
            } else {
                self.pending_delete_index = None;
            }
        }

        // Action Editor Form (Create or Edit)
        if self.action_editor_mode != ActionEditorMode::Closed {
            self.render_action_editor_card(ui, pal);
            ui.add_space(12.0);
        }

        // Top toolbar: Search + Create Action + Save
        section_card(ui, pal, "Custom Actions (actions.toml)", |ui| {
            ui.horizontal(|ui| {
                ui.label("Filter:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.action_search_query)
                        .hint_text("Search by label, ID, or executable…")
                        .desired_width(240.0),
                );
                if !self.action_search_query.is_empty() && ui.small_button("Clear").clicked() {
                    self.action_search_query.clear();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("+ Create Custom Action").clicked() {
                        let draft = EditableAction::default();
                        self.open_action_editor(ActionEditorMode::Create, draft);
                    }
                    let has_unsaved = self.draft_actions != self.saved_actions;
                    if ui
                        .add_enabled(has_unsaved, egui::Button::new("Save All Action Changes"))
                        .clicked()
                    {
                        let _ = self.save_actions(false);
                    }
                });
            });

            ui.add_space(10.0);

            if self.draft_actions.actions.is_empty() {
                ui.label(
                    RichText::new(
                        "No custom actions defined yet in ~/.config/radivine/actions.toml. \
                         Click '+ Create Custom Action' to add a shell-free custom command to the Custom wheel submenu.",
                    )
                    .color(pal.fg_secondary),
                );
            } else {
                let query = self.action_search_query.trim().to_ascii_lowercase();
                let mut edit_request: Option<(usize, EditableAction)> = None;
                let mut toggle_save = false;

                for (idx, act) in self.draft_actions.actions.iter_mut().enumerate() {
                    if !query.is_empty()
                        && !act.label.to_ascii_lowercase().contains(&query)
                        && !act.id.to_ascii_lowercase().contains(&query)
                        && !act.executable.to_ascii_lowercase().contains(&query)
                    {
                        continue;
                    }

                    egui::Frame::NONE
                        .fill(pal.bg_elevated)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(12, 10))
                        .stroke(Stroke::new(1.0, pal.border))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut act.enabled, "").changed() {
                                    toggle_save = true;
                                }
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&act.label)
                                                .size(14.0)
                                                .strong()
                                                .color(pal.fg_primary),
                                        );
                                        ui.label(
                                            RichText::new(format!("({})", act.canonical_id()))
                                                .size(11.5)
                                                .color(pal.leaf_green),
                                        );
                                        if !act.icon.is_empty() {
                                            ui.label(
                                                RichText::new(format!("· {}", act.icon))
                                                    .size(11.5)
                                                    .color(pal.fg_muted),
                                            );
                                        }
                                    });
                                    if !act.description.is_empty() {
                                        ui.label(
                                            RichText::new(&act.description)
                                                .size(12.0)
                                                .color(pal.fg_secondary),
                                        );
                                    }
                                    let argv_preview = if act.args.is_empty() {
                                        act.executable.clone()
                                    } else {
                                        format!("{} {}", act.executable, act.args.join(" "))
                                    };
                                    ui.label(
                                        RichText::new(format!(
                                            "Exec: {argv_preview}  ·  count={}  ·  per_item={}  ·  notify={}",
                                            act.count, act.per_item, act.notify
                                        ))
                                        .font(FontId::new(11.5, FontFamily::Monospace))
                                        .color(pal.fg_muted),
                                    );
                                });

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.button("Delete").clicked() {
                                        self.pending_delete_index = Some(idx);
                                    }
                                    if ui.button("Edit").clicked() {
                                        edit_request = Some((idx, act.clone()));
                                    }
                                });
                            });
                        });
                    ui.add_space(6.0);
                }

                if let Some((idx, draft)) = edit_request {
                    self.open_action_editor(ActionEditorMode::Edit(idx), draft);
                }
                if toggle_save {
                    let _ = self.save_actions(false);
                }
            }
        });
    }

    fn open_action_editor(&mut self, mode: ActionEditorMode, draft: EditableAction) {
        self.editor_args_text = draft.args.join("\n");
        self.editor_extensions_text = draft.extensions.join(", ");
        self.editor_mime_text = draft.mime_globs.join(", ");
        self.editor_timeout_text = draft.timeout_ms.map_or_else(String::new, |v| v.to_string());
        self.editor_draft = draft;
        self.editor_errors.clear();
        self.action_editor_mode = mode;
    }

    fn render_action_editor_card(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        let title = match self.action_editor_mode {
            ActionEditorMode::Create => "Create New Custom Action (Shell-Free Execution)",
            ActionEditorMode::Edit(_) => "Edit Custom Action (Shell-Free Execution)",
            ActionEditorMode::Closed => return,
        };

        section_card(ui, pal, title, |ui| {
            if !self.editor_errors.is_empty() {
                egui::Frame::NONE
                    .fill(pal.err_bg)
                    .corner_radius(CornerRadius::same(5))
                    .inner_margin(Margin::symmetric(10, 6))
                    .stroke(Stroke::new(1.0, pal.err_fg))
                    .show(ui, |ui| {
                        for e in &self.editor_errors {
                            ui.label(RichText::new(format!("• {e}")).color(pal.err_fg).size(12.0));
                        }
                    });
                ui.add_space(8.0);
            }

            egui::Grid::new("action_editor_grid")
                .num_columns(2)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Action ID (custom.*):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_draft.id)
                            .hint_text("custom.open_terminal")
                            .desired_width(320.0),
                    );
                    ui.end_row();

                    ui.label("Display Label:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_draft.label)
                            .hint_text("Open Terminal")
                            .desired_width(320.0),
                    );
                    ui.end_row();

                    ui.label("Description (Caption):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_draft.description)
                            .hint_text("Open terminal in the file's parent directory")
                            .desired_width(420.0),
                    );
                    ui.end_row();

                    ui.label("Wheel Icon:");
                    egui::ComboBox::from_id_salt("action_icon_combo")
                        .selected_text(&self.editor_draft.icon)
                        .show_ui(ui, |ui| {
                            for &(key, desc) in AVAILABLE_ICONS {
                                ui.selectable_value(
                                    &mut self.editor_draft.icon,
                                    key.to_string(),
                                    format!("{desc} ({key})"),
                                );
                            }
                        });
                    ui.end_row();

                    ui.label("Executable Program (exec[0]):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_draft.executable)
                            .font(FontId::new(12.5, FontFamily::Monospace))
                            .hint_text("/usr/bin/sha256sum")
                            .desired_width(420.0),
                    );
                    ui.end_row();

                    ui.label("Arguments (one per line):");
                    ui.vertical(|ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.editor_args_text)
                                .font(FontId::new(12.5, FontFamily::Monospace))
                                .desired_rows(3)
                                .desired_width(420.0)
                                .hint_text("--working-directory\n{dir}"),
                        );
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Insert whole-argument placeholder:")
                                    .size(11.5)
                                    .color(pal.fg_secondary),
                            );
                            for &(token, tip) in PLACEHOLDER_CHIPS {
                                if ui.small_button(token).on_hover_text(tip).clicked() {
                                    if !self.editor_args_text.is_empty()
                                        && !self.editor_args_text.ends_with('\n')
                                    {
                                        self.editor_args_text.push('\n');
                                    }
                                    self.editor_args_text.push_str(token);
                                }
                            }
                        });
                    });
                    ui.end_row();

                    ui.label("Working Directory (cwd):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_draft.cwd)
                            .font(FontId::new(12.5, FontFamily::Monospace))
                            .hint_text("Optional: {dir}, {path}, or /absolute/path")
                            .desired_width(320.0),
                    );
                    ui.end_row();

                    ui.label("Item Count Filter (when.count):");
                    ui.horizontal(|ui| {
                        for preset in ["1", "1..", "1..=16", "2.."] {
                            if ui
                                .selectable_label(self.editor_draft.count == preset, preset)
                                .clicked()
                            {
                                self.editor_draft.count = preset.to_string();
                            }
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor_draft.count)
                                .desired_width(90.0),
                        );
                    });
                    ui.end_row();

                    ui.label("File Extensions (comma-separated):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_extensions_text)
                            .hint_text("Optional: rs, toml, md, log")
                            .desired_width(320.0),
                    );
                    ui.end_row();

                    ui.label("MIME Globs (comma-separated):");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editor_mime_text)
                            .hint_text("Optional: text/*, application/json")
                            .desired_width(320.0),
                    );
                    ui.end_row();

                    ui.label("Execution Options:");
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.editor_draft.enabled, "Enabled");
                        ui.checkbox(
                            &mut self.editor_draft.per_item,
                            "Run once per item (per_item)",
                        );
                        ui.checkbox(&mut self.editor_draft.wait, "Wait for exit status (wait)");
                        ui.checkbox(
                            &mut self.editor_draft.require_text,
                            "Require text file (when.require_text)",
                        );
                    });
                    ui.end_row();

                    ui.label("Timeout (ms) & Notification:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editor_timeout_text)
                                .hint_text("Optional ms (e.g. 10000)")
                                .desired_width(140.0),
                        );
                        ui.label("Notify:");
                        for pol in ["on-error", "always", "never"] {
                            if ui
                                .selectable_label(self.editor_draft.notify == pol, pol)
                                .clicked()
                            {
                                self.editor_draft.notify = pol.to_string();
                            }
                        }
                    });
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("  Validate & Save Action  ").clicked() {
                    self.commit_action_editor();
                }
                if ui.button("Cancel").clicked() {
                    self.action_editor_mode = ActionEditorMode::Closed;
                    self.editor_errors.clear();
                }
            });
        });
    }

    fn commit_action_editor(&mut self) {
        self.editor_draft.args = self
            .editor_args_text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        self.editor_draft.extensions = self
            .editor_extensions_text
            .split(',')
            .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        self.editor_mime_text = self.editor_mime_text.trim().to_string();
        self.editor_draft.mime_globs = self
            .editor_mime_text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();

        if self.editor_timeout_text.trim().is_empty() {
            self.editor_draft.timeout_ms = None;
        } else {
            match self.editor_timeout_text.trim().parse::<u64>() {
                Ok(ms) => self.editor_draft.timeout_ms = Some(ms),
                Err(_) => {
                    self.editor_errors =
                        vec!["Timeout must be a positive integer in milliseconds.".to_string()];
                    return;
                }
            }
        }

        if let Err(errs) = self.editor_draft.validate() {
            self.editor_errors = errs;
            return;
        }

        let mut candidate_doc = self.draft_actions.clone();
        match self.action_editor_mode {
            ActionEditorMode::Create => candidate_doc.actions.push(self.editor_draft.clone()),
            ActionEditorMode::Edit(idx) => {
                if let Some(slot) = candidate_doc.actions.get_mut(idx) {
                    *slot = self.editor_draft.clone();
                }
            }
            ActionEditorMode::Closed => return,
        }

        if let Err(errs) =
            apply_actions_to_toml_doc(&self.actions_disk_snapshot.raw_text, &candidate_doc)
        {
            self.editor_errors = errs;
            return;
        }

        self.draft_actions = candidate_doc;
        self.action_editor_mode = ActionEditorMode::Closed;
        self.editor_errors.clear();
        let _ = self.save_actions(false);
    }

    fn render_diagnostics_page(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        pal: &BrandPalette,
    ) {
        // 1. Compatibility & Desktop Environment Summary
        section_card(ui, pal, "Desktop Compatibility & Doctor Summary", |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Run Doctor & Refresh Logs").clicked() {
                    let _ = self.worker_tx.send(WorkerCommand::RefreshDiagnostics);
                }
                ui.checkbox(
                    &mut self.redact_before_export,
                    "Redact personal paths, usernames, and file URIs when copying/exporting",
                );
                if let Some(ref diag) = self.diagnostics_snapshot {
                    if ui
                        .button("Copy Diagnostic Report for GitHub Issue")
                        .clicked()
                    {
                        let report = format_diagnostic_report(
                            &self.service_snapshot,
                            diag,
                            self.redact_before_export,
                        );
                        ctx.copy_text(report.clone());
                        self.exported_report_preview = Some(report);
                        self.banner_ok = Some(
                            "Copied privacy-conscious diagnostic report to clipboard.".to_string(),
                        );
                    }
                    if ui.button("Export Report to File").clicked() {
                        let report = format_diagnostic_report(
                            &self.service_snapshot,
                            diag,
                            self.redact_before_export,
                        );
                        let export_path = self
                            .service_ctrl
                            .paths()
                            .state_dir
                            .join("radivine-diagnostic-report.md");
                        let _ = std::fs::create_dir_all(&self.service_ctrl.paths().state_dir);
                        if std::fs::write(&export_path, report.as_bytes()).is_ok() {
                            self.exported_report_preview = Some(report);
                            self.banner_ok = Some(format!(
                                "Exported diagnostic report to {} (never uploaded automatically).",
                                export_path.display()
                            ));
                        }
                    }
                }
            });

            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Privacy Notice: Diagnostic reports and logs are generated strictly locally and are never uploaded automatically.",
                )
                .size(11.5)
                .color(pal.fg_muted),
            );

            if let Some(ref diag) = self.diagnostics_snapshot {
                ui.add_space(8.0);
                for note in &diag.compatibility_notes {
                    ui.label(
                        RichText::new(format!("• {note}"))
                            .size(12.5)
                            .color(pal.fg_secondary),
                    );
                }
                ui.add_space(8.0);
                egui::CollapsingHeader::new(
                    RichText::new("X11 Doctor Probe Output (radivine doctor)")
                        .strong()
                        .color(pal.fg_primary),
                )
                .default_open(true)
                .show(ui, |ui| {
                    egui::Frame::NONE
                        .fill(pal.bg_sidebar)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(10, 8))
                        .stroke(Stroke::new(1.0, pal.border))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(diag.doctor_text.trim())
                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                    .color(pal.fg_primary),
                            );
                        });
                });
            } else {
                ui.label("Collecting diagnostics…");
            }
        });

        ui.add_space(12.0);

        // 2. Bounded Log Viewer Card
        section_card(ui, pal, "Recent Daemon Logs (Bounded History)", |ui| {
            let Some(ref diag) = self.diagnostics_snapshot else {
                ui.label("Loading recent daemon logs…");
                return;
            };

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Source: {}", diag.log_source))
                        .size(12.0)
                        .color(pal.fg_secondary),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.log_search_query)
                            .hint_text("Filter logs…")
                            .desired_width(180.0),
                    );
                    for (sev, label) in [
                        (None, "All"),
                        (Some(LogSeverity::Error), "Errors"),
                        (Some(LogSeverity::Warn), "Warnings"),
                        (Some(LogSeverity::Info), "Info"),
                    ] {
                        if ui
                            .selectable_label(self.log_filter_severity == sev, label)
                            .clicked()
                        {
                            self.log_filter_severity = sev;
                        }
                    }
                });
            });

            ui.add_space(8.0);
            egui::Frame::NONE
                .fill(pal.bg_sidebar)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 8))
                .stroke(Stroke::new(1.0, pal.border))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if diag.logs.is_empty() {
                        ui.label(
                            RichText::new("No recent log entries recorded yet.")
                                .color(pal.fg_muted),
                        );
                    } else {
                        let q = self.log_search_query.trim().to_ascii_lowercase();
                        ScrollArea::vertical()
                            .id_salt("diagnostics_log_scroll")
                            .max_height(240.0)
                            .show(ui, |ui| {
                                for entry in &diag.logs {
                                    if let Some(req_sev) = self.log_filter_severity
                                        && entry.severity != req_sev
                                    {
                                        continue;
                                    }
                                    if !q.is_empty() && !entry.raw.to_ascii_lowercase().contains(&q)
                                    {
                                        continue;
                                    }
                                    let sev_color = match entry.severity {
                                        LogSeverity::Error => pal.err_fg,
                                        LogSeverity::Warn => pal.warn_fg,
                                        LogSeverity::Info => pal.leaf_green,
                                        LogSeverity::Debug => pal.fg_muted,
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "[{:5}]",
                                                entry.severity.label()
                                            ))
                                            .font(FontId::new(11.5, FontFamily::Monospace))
                                            .color(sev_color)
                                            .strong(),
                                        );
                                        if !entry.timestamp.is_empty() {
                                            ui.label(
                                                RichText::new(&entry.timestamp)
                                                    .font(FontId::new(11.5, FontFamily::Monospace))
                                                    .color(pal.fg_muted),
                                            );
                                        }
                                        ui.label(
                                            RichText::new(&entry.message)
                                                .font(FontId::new(11.5, FontFamily::Monospace))
                                                .color(pal.fg_primary),
                                        );
                                    });
                                }
                            });
                    }
                });
        });
    }

    fn render_about_page(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(ui, pal, "Radivine — Branch into action.", |ui| {
            if let Some(ref logo) = self.logo_horiz_tex {
                ui.add(
                    egui::Image::from_texture(logo)
                        .max_width(300.0)
                        .corner_radius(6.0),
                );
                ui.add_space(8.0);
            }
            ui.label(
                RichText::new(format!("Version v{}", env!("CARGO_PKG_VERSION")))
                    .size(15.0)
                    .strong()
                    .color(pal.leaf_green),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "A fast, extensible, context-aware hierarchical radial action wheel for Linux desktops.",
                )
                .size(13.5)
                .color(pal.fg_primary),
            );
            ui.add_space(10.0);
            egui::Grid::new("about_info_grid")
                .num_columns(2)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    kv_cell(ui, pal, "Open-Source License", "MIT OR Apache-2.0");
                    ui.end_row();
                    kv_cell(
                        ui,
                        pal,
                        "Repository",
                        "https://github.com/marawan6569/Radivine",
                    );
                    ui.end_row();
                    kv_cell(
                        ui,
                        pal,
                        "Components",
                        "radivine (daemon/CLI), radivine-control (Control Center), radivine-viewer (Preview)",
                    );
                    ui.end_row();
                    kv_cell(
                        ui,
                        pal,
                        "Bundled Assets",
                        "Inter Variable Font (OFL-1.1), Lucide Icons (ISC)",
                    );
                    ui.end_row();
                });
        });
    }

    /// Populates realistic running-daemon telemetry and diagnostics for headless screenshot export.
    pub fn load_demo_showcase_state(&mut self) {
        self.config_path = PathBuf::from("~/.config/radivine/config.toml");
        self.actions_path = PathBuf::from("~/.config/radivine/actions.toml");
        self.service_snapshot = ServiceSnapshot {
            health: DaemonHealthState::Running,
            daemon: Some(DaemonStatus {
                pid: 4821,
                overlay_window_id: 0x04a0_0002,
                session_state: "Idle".to_string(),
                scale: 1.0,
                monitor_rect: (0, 0, 3840, 1080),
                root_wheel_center_px: None,
                root_wheel_outer_radius_px: None,
                invoke_count: 14,
                drag_session_count: 38,
                last_committed_action: Some("radivine.preview.full".to_string()),
                last_committed_path: Some(vec![0, 0]),
                last_committed_items: vec!["file:///home/user/Documents/notes.md".to_string()],
                last_committed_source_app: Some("nemo".to_string()),
                last_committed_mode: Some("Drag".to_string()),
                last_job_action: Some("radivine.preview.full".to_string()),
                last_job_error: None,
                last_notification: None,
                notifications: Vec::new(),
                config_warnings: Vec::new(),
                active_item_states: Vec::new(),
                last_summon_us: Some(640),
                last_dnd_status_us: Some(38),
                last_drop_dispatch_us: Some(180),
                last_frame_render_us: Some(212),
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime_secs: 9_420,
                desktop_session: "x11".to_string(),
                desktop_environment: "X-Cinnamon".to_string(),
                display: ":0".to_string(),
                custom_action_count: 2,
                rss_kib: Some(18_840),
            }),
            systemd: SystemdUnitInfo {
                available: true,
                load_state: "loaded".to_string(),
                active_state: "active".to_string(),
                sub_state: "running".to_string(),
                unit_file_state: "enabled".to_string(),
                main_pid: 4821,
                result: "success".to_string(),
            },
            management_mode: "systemd --user (radivine.service)".to_string(),
            autostart: AutostartStatus {
                enabled: true,
                method_label: "Systemd User Service (radivine.service)".to_string(),
                systemd_available: true,
                systemd_enabled: true,
                xdg_enabled: false,
                conflicts: Vec::new(),
            },
            session_type: "x11".to_string(),
            desktop_environment: "X-Cinnamon".to_string(),
            socket_path: PathBuf::from("/run/user/1000/radivine/radivine.sock"),
            last_error: None,
        };

        if self.draft_actions.actions.is_empty() {
            self.draft_actions.actions = vec![
                EditableAction {
                    id: "custom.open_terminal_here".to_string(),
                    label: "Open Terminal".to_string(),
                    enabled: true,
                    icon: "lucide:terminal".to_string(),
                    description: "Open terminal in the file's parent folder".to_string(),
                    executable: "/usr/bin/x-terminal-emulator".to_string(),
                    args: vec!["--working-directory".to_string(), "{dir}".to_string()],
                    cwd: String::new(),
                    per_item: false,
                    wait: false,
                    timeout_ms: None,
                    notify: "on-error".to_string(),
                    schemes: vec!["file".to_string()],
                    count: "1".to_string(),
                    mime_globs: Vec::new(),
                    extensions: Vec::new(),
                    require_text: false,
                },
                EditableAction {
                    id: "custom.sha256_notify".to_string(),
                    label: "SHA-256 Digest".to_string(),
                    enabled: true,
                    icon: "lucide:code".to_string(),
                    description: "Calculate SHA-256 checksum for selected files".to_string(),
                    executable: "/usr/bin/sha256sum".to_string(),
                    args: vec!["{paths}".to_string()],
                    cwd: String::new(),
                    per_item: false,
                    wait: true,
                    timeout_ms: Some(10_000),
                    notify: "always".to_string(),
                    schemes: vec!["file".to_string()],
                    count: "1..=16".to_string(),
                    mime_globs: Vec::new(),
                    extensions: Vec::new(),
                    require_text: false,
                },
            ];
            self.saved_actions = self.draft_actions.clone();
        }

        self.diagnostics_snapshot = Some(DiagnosticsSnapshot {
            doctor: Err("Simulated in headless export".to_string()),
            doctor_text: "Session type       : x11\n\
                          DISPLAY            : :0\n\
                          Server vendor      : The X.Org Foundation\n\
                          Compositor         : active (_NET_WM_CM_S0 = 0x01a00004, comm \"cinnamon\")\n\
                          32-bit visual      : 0x62 TrueColor, depth 32\n\
                          Extensions         : Composite 0.4 (ok), XFixes 6.0 (ok), SHAPE 1.1 (ok), XInputExtension 2.4 (ok), RANDR 1.6 (ok), MIT-SHM 1.2 (ok)\n\
                          Capabilities       : drag_session=yes, invoke_at_pointer=yes, translucency=yes, clipboard_files=yes"
                .to_string(),
            log_source: "systemd user journal (radivine.service)".to_string(),
            logs: vec![
                LogEntry {
                    timestamp: "2026-10-23T19:00:01Z".to_string(),
                    severity: LogSeverity::Info,
                    message: "INFO radivine daemon ready pid=4821 overlay_window=\"0x04a00002\"".to_string(),
                    raw: "2026-10-23T19:00:01Z INFO radivine daemon ready pid=4821 overlay_window=\"0x04a00002\"".to_string(),
                },
                LogEntry {
                    timestamp: "2026-10-23T19:04:12Z".to_string(),
                    severity: LogSeverity::Info,
                    message: "INFO loaded actions.toml custom_actions=2 warnings=0".to_string(),
                    raw: "2026-10-23T19:04:12Z INFO loaded actions.toml custom_actions=2 warnings=0".to_string(),
                },
                LogEntry {
                    timestamp: "2026-10-23T19:11:45Z".to_string(),
                    severity: LogSeverity::Info,
                    message: "INFO dispatched action completed job_id=14 action_id=radivine.preview.full".to_string(),
                    raw: "2026-10-23T19:11:45Z INFO dispatched action completed job_id=14 action_id=radivine.preview.full".to_string(),
                },
            ],
            compatibility_notes: vec![
                "X11 session active: Full Nemo 'Drag + Shift' mid-drag interception and 'radivine invoke' are supported.".to_string(),
            ],
        });
    }
}

impl eframe::App for ControlCenterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_frame(ui);
    }
}

/// Exports authentic PNG screenshots of all Control Center pages using the pure-Rust software
/// rasterizer (`DISPLAY=""`, zero X11 interaction).
pub fn export_all_page_screenshots(
    out_dir: &Path,
    service_paths: ServicePaths,
) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out_dir)?;
    let ctx = egui::Context::default();
    let mut app = ControlCenterApp::new(ControlPage::Dashboard, service_paths, None, None, false);
    app.load_demo_showcase_state();

    let mut rasterizer = EguiSoftRasterizer::new();
    let size = Vec2::new(1120.0, 720.0);
    let mut written = Vec::new();

    for (page, filename, theme) in [
        (ControlPage::Dashboard, "control-dashboard-dark.png", "dark"),
        (ControlPage::General, "control-general-dark.png", "dark"),
        (ControlPage::Wheel, "control-wheel-dark.png", "dark"),
        (ControlPage::Actions, "control-actions-dark.png", "dark"),
        (
            ControlPage::Diagnostics,
            "control-diagnostics-dark.png",
            "dark",
        ),
        (ControlPage::About, "control-about-light.png", "light"),
    ] {
        app.draft_config.appearance.theme = theme.to_string();
        app.saved_config.appearance.theme = theme.to_string();
        app.set_page(page);
        let target = out_dir.join(filename);
        rasterizer.render_to_png(&ctx, size, 1.0, &target, |ui| {
            app.drain_worker_events();
            app.load_demo_showcase_state();
            if page == ControlPage::Actions && !app.draft_actions.actions.is_empty() {
                let first = app.draft_actions.actions[0].clone();
                app.open_action_editor(ActionEditorMode::Edit(0), first);
            }
            app.render_frame(ui);
        })?;
        written.push(target);
    }

    Ok(written)
}

/// Launches the native `eframe` desktop window for `radivine-control`.
pub fn run_control_window(
    initial_page: ControlPage,
    service_paths: ServicePaths,
    instance_lock: Option<ControlInstanceLock>,
) -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Radivine Control Center")
        .with_app_id("radivine-control")
        .with_inner_size(Vec2::new(1120.0, 720.0))
        .with_min_inner_size(Vec2::new(760.0, 500.0));

    if let Some(icon_data) = decode_png_to_icon_data(EMBEDDED_APP_ICON_PNG) {
        viewport = viewport.with_icon(Arc::new(icon_data));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let mut lock_opt = instance_lock;
    eframe::run_native(
        "radivine-control",
        native_options,
        Box::new(move |cc| {
            let lock = lock_opt.take();
            let app = ControlCenterApp::new(
                initial_page,
                service_paths,
                lock,
                Some(cc.egui_ctx.clone()),
                true,
            );
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}

fn section_card<F>(ui: &mut egui::Ui, pal: &BrandPalette, title: &str, body: F)
where
    F: FnOnce(&mut egui::Ui),
{
    let inner_w = (ui.available_width() - 36.0).max(200.0);
    egui::Frame::NONE
        .fill(pal.bg_card)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(16, 14))
        .stroke(Stroke::new(1.0, pal.border))
        .show(ui, |ui| {
            ui.set_min_width(inner_w);
            ui.set_max_width(inner_w);
            ui.label(
                RichText::new(title)
                    .font(FontId::new(15.0, FontFamily::Proportional))
                    .strong()
                    .color(pal.fg_primary),
            );
            ui.add_space(8.0);
            body(ui);
        });
}

fn kv_cell(ui: &mut egui::Ui, pal: &BrandPalette, label: &str, value: &str) {
    ui.label(RichText::new(label).color(pal.fg_secondary).size(12.5));
    ui.label(
        RichText::new(value)
            .color(pal.fg_primary)
            .size(12.5)
            .strong(),
    );
}

fn health_colors(
    health: DaemonHealthState,
    pal: &BrandPalette,
) -> (Color32, Color32, &'static str) {
    match health {
        DaemonHealthState::Running => (pal.ok_fg, pal.ok_bg, "●"),
        DaemonHealthState::Starting | DaemonHealthState::Stopping => {
            (pal.warn_fg, pal.warn_bg, "◐")
        }
        DaemonHealthState::Failed | DaemonHealthState::Unreachable => (pal.err_fg, pal.err_bg, "▲"),
        DaemonHealthState::Stopped => (pal.fg_muted, pal.bg_elevated, "○"),
    }
}

fn format_uptime(secs: u64) -> String {
    let hrs = secs / 3600;
    let mins = (secs % 3600) / 60;
    let rem = secs % 60;
    if hrs > 0 {
        format!("{hrs}h {mins}m {rem}s")
    } else if mins > 0 {
        format!("{mins}m {rem}s")
    } else {
        format!("{rem}s")
    }
}

fn decode_scaled_pixmap(png_bytes: &[u8], max_dim: u32) -> Option<tiny_skia::Pixmap> {
    let src = tiny_skia::Pixmap::decode_png(png_bytes).ok()?;
    let sw = src.width();
    let sh = src.height();
    if sw <= max_dim && sh <= max_dim {
        return Some(src);
    }
    let scale = (max_dim as f32 / sw.max(sh) as f32).clamp(0.05, 1.0);
    let dw = ((sw as f32 * scale).round() as u32).max(1);
    let dh = ((sh as f32 * scale).round() as u32).max(1);
    let mut dst = tiny_skia::Pixmap::new(dw, dh)?;
    let paint = tiny_skia::PixmapPaint {
        quality: tiny_skia::FilterQuality::Bicubic,
        ..Default::default()
    };
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &paint,
        tiny_skia::Transform::from_scale(dw as f32 / sw as f32, dh as f32 / sh as f32),
        None,
    );
    Some(dst)
}

fn decode_png_to_color_image(png_bytes: &[u8], max_dim: u32) -> Option<ColorImage> {
    let pm = decode_scaled_pixmap(png_bytes, max_dim)?;
    let w = pm.width() as usize;
    let h = pm.height() as usize;
    let data = pm.data();
    let mut pixels = Vec::with_capacity(w * h);
    for px in data.chunks_exact(4) {
        // `tiny_skia::Pixmap` stores premultiplied RGBA8, matching `Color32::from_rgba_premultiplied`
        pixels.push(Color32::from_rgba_premultiplied(px[0], px[1], px[2], px[3]));
    }
    Some(ColorImage {
        size: [w, h],
        pixels,
        source_size: Vec2::new(w as f32, h as f32),
    })
}

fn decode_png_to_icon_data(png_bytes: &[u8]) -> Option<egui::IconData> {
    let pm = decode_scaled_pixmap(png_bytes, 128)?;
    let width = pm.width();
    let height = pm.height();
    let mut rgba = Vec::with_capacity(pm.data().len());
    for px in pm.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}
