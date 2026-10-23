//! `eframe`/`egui` desktop preview window (`Full`, `Head`, `Tail`) for `radivine-viewer` (`ACTION_SYSTEM.md` §8.1).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, FontFamily, FontId, Key, Layout, Margin, RichText, ScrollArea, Stroke,
    TextFormat, Vec2, text::LayoutJob,
};

use crate::file_io::{
    DetectedEncoding, OpenedTextFile, PreviewLine, ViewerError, format_byte_size,
};
use crate::full::FullDocument;
use crate::head_tail::{HeadTailResult, read_head, read_tail};
use crate::search::{SearchController, find_highlight_ranges_in_line};
use crate::{PreviewMode, ViewerTheme};

/// JSON telemetry dump written when `RADIVINE_VIEWER_DUMP_PATH` is set (used by E2E tests).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ViewerSessionDump {
    /// Viewer process ID.
    pub pid: u32,
    /// `"full"`, `"head"`, or `"tail"`.
    pub mode: String,
    /// Canonical/target file path.
    pub path: String,
    /// Configured `--lines` parameter.
    pub requested_lines: usize,
    /// Number of initial lines loaded (`Head`/`Tail` line count, or initial indexed line count in `Full`).
    pub line_count: usize,
    /// First displayed line text, if any.
    pub first_line: Option<String>,
    /// Last displayed line text in the initial slice, if any.
    pub last_line: Option<String>,
    /// File size in bytes.
    pub file_size: u64,
    /// Detected encoding label (`"UTF-8"`, `"UTF-16 LE"`, etc.).
    pub encoding: String,
    /// Whether CRLF line endings were detected.
    pub has_crlf: bool,
    /// Whether the 8 MiB byte cap truncated the output.
    pub truncated_by_byte_limit: bool,
    /// Whether any line exceeded 64 KiB and was truncated.
    pub has_long_line_truncation: bool,
    /// Whether malformed encoding sequences were detected.
    pub has_encoding_warnings: bool,
}

enum DocumentBackend {
    HeadTail(HeadTailResult),
    Full(Box<FullDocument>),
}

#[derive(Clone, Copy)]
struct Palette {
    bg_primary: Color32,
    bg_elevated: Color32,
    bg_gutter: Color32,
    fg_primary: Color32,
    fg_muted: Color32,
    accent: Color32,
    accent_bg: Color32,
    match_bg: Color32,
    active_match_bg: Color32,
    active_row_bg: Color32,
    warn_bg: Color32,
    warn_fg: Color32,
    err_bg: Color32,
    err_fg: Color32,
    border: Color32,
}

impl Palette {
    const fn dark() -> Self {
        Self {
            bg_primary: Color32::from_rgb(0x12, 0x14, 0x1A),
            bg_elevated: Color32::from_rgb(0x1B, 0x1E, 0x27),
            bg_gutter: Color32::from_rgb(0x15, 0x18, 0x20),
            fg_primary: Color32::from_rgb(0xEC, 0xEF, 0xF4),
            fg_muted: Color32::from_rgb(0x7C, 0x85, 0x99),
            accent: Color32::from_rgb(0x38, 0xBD, 0xF8),
            accent_bg: Color32::from_rgb(0x16, 0x35, 0x4D),
            match_bg: Color32::from_rgb(0x52, 0x3E, 0x10),
            active_match_bg: Color32::from_rgb(0xD9, 0x77, 0x06),
            active_row_bg: Color32::from_rgb(0x1E, 0x26, 0x36),
            warn_bg: Color32::from_rgb(0x3B, 0x2A, 0x10),
            warn_fg: Color32::from_rgb(0xFB, 0xBF, 0x24),
            err_bg: Color32::from_rgb(0x3B, 0x12, 0x19),
            err_fg: Color32::from_rgb(0xFB, 0x71, 0x85),
            border: Color32::from_rgb(0x2A, 0x2F, 0x3D),
        }
    }

    const fn light() -> Self {
        Self {
            bg_primary: Color32::from_rgb(0xF8, 0xFA, 0xFC),
            bg_elevated: Color32::from_rgb(0xF1, 0xF5, 0xF9),
            bg_gutter: Color32::from_rgb(0xE2, 0xE8, 0xF0),
            fg_primary: Color32::from_rgb(0x0F, 0x17, 0x2A),
            fg_muted: Color32::from_rgb(0x64, 0x74, 0x8B),
            accent: Color32::from_rgb(0x02, 0x84, 0xC7),
            accent_bg: Color32::from_rgb(0xE0, 0xF2, 0xFE),
            match_bg: Color32::from_rgb(0xFE, 0xF0, 0x8A),
            active_match_bg: Color32::from_rgb(0xF5, 0x9E, 0x0B),
            active_row_bg: Color32::from_rgb(0xE0, 0xF2, 0xFE),
            warn_bg: Color32::from_rgb(0xFE, 0xF3, 0xC7),
            warn_fg: Color32::from_rgb(0xB4, 0x53, 0x09),
            err_bg: Color32::from_rgb(0xFF, 0xE4, 0xE6),
            err_fg: Color32::from_rgb(0xBE, 0x12, 0x3C),
            border: Color32::from_rgb(0xCB, 0xD5, 0xE1),
        }
    }
}

/// Interactive desktop application state for `radivine-viewer`.
pub struct ViewerApp {
    path: PathBuf,
    display_name: String,
    mode: PreviewMode,
    requested_lines: usize,
    is_dark: bool,
    backend: DocumentBackend,
    search: SearchController,
    search_open: bool,
    search_focus_requested: bool,
    search_query: String,
    search_case_sensitive: bool,
    goto_line_input: String,
    pending_scroll_to_line: Option<usize>,
    reload_error: Option<String>,
    started_at: Instant,
    auto_close_after: Option<Duration>,
    dump_path: Option<PathBuf>,
    dump_written: bool,
}

impl ViewerApp {
    /// Initializes a [`ViewerApp`] from an already-opened [`OpenedTextFile`].
    pub fn new(
        opened: OpenedTextFile,
        mode: PreviewMode,
        requested_lines: usize,
        theme: ViewerTheme,
        egui_ctx: Option<egui::Context>,
    ) -> Result<Self, ViewerError> {
        let path = opened.path.clone();
        let display_name = opened.display_name.clone();
        let is_dark = !matches!(theme, ViewerTheme::Light);

        let repaint_hook: Option<Arc<dyn Fn() + Send + Sync>> = egui_ctx.map(|ctx| {
            let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                ctx.request_repaint();
            });
            hook
        });

        let backend = match mode {
            PreviewMode::Head => DocumentBackend::HeadTail(read_head(&opened, requested_lines)?),
            PreviewMode::Tail => DocumentBackend::HeadTail(read_tail(&opened, requested_lines)?),
            PreviewMode::Full => DocumentBackend::Full(Box::new(FullDocument::with_repaint_hook(
                opened,
                repaint_hook,
            )?)),
        };

        let auto_close_after = std::env::var("RADIVINE_VIEWER_AUTO_CLOSE_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&ms| ms > 0)
            .map(Duration::from_millis);

        let dump_path = std::env::var("RADIVINE_VIEWER_DUMP_PATH")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        let mut app = Self {
            path,
            display_name,
            mode,
            requested_lines,
            is_dark,
            backend,
            search: SearchController::new(),
            search_open: false,
            search_focus_requested: false,
            search_query: String::new(),
            search_case_sensitive: false,
            goto_line_input: String::new(),
            pending_scroll_to_line: None,
            reload_error: None,
            started_at: Instant::now(),
            auto_close_after,
            dump_path,
            dump_written: false,
        };

        app.write_session_dump_if_configured();
        Ok(app)
    }

    /// Builds a [`ViewerSessionDump`] reflecting the current viewer state.
    pub fn build_session_dump(&mut self) -> ViewerSessionDump {
        match &mut self.backend {
            DocumentBackend::HeadTail(res) => ViewerSessionDump {
                pid: std::process::id(),
                mode: self.mode.as_str().to_string(),
                path: self.path.display().to_string(),
                requested_lines: self.requested_lines,
                line_count: res.lines.len(),
                first_line: res.lines.first().map(|l| l.text.clone()),
                last_line: res.lines.last().map(|l| l.text.clone()),
                file_size: res.file_size,
                encoding: res.encoding.label().to_string(),
                has_crlf: res.has_crlf,
                truncated_by_byte_limit: res.truncated_by_byte_limit,
                has_long_line_truncation: res.has_long_line_truncation,
                has_encoding_warnings: res.has_encoding_warnings,
            },
            DocumentBackend::Full(doc) => {
                let snap = doc.index_snapshot();
                let preview = doc.fetch_lines(0, 50);
                ViewerSessionDump {
                    pid: std::process::id(),
                    mode: self.mode.as_str().to_string(),
                    path: self.path.display().to_string(),
                    requested_lines: self.requested_lines,
                    line_count: snap.total_lines,
                    first_line: preview.first().map(|l| l.text.clone()),
                    last_line: preview.last().map(|l| l.text.clone()),
                    file_size: doc.file_size(),
                    encoding: doc.opened_file().encoding.label().to_string(),
                    has_crlf: doc.opened_file().has_crlf,
                    truncated_by_byte_limit: false,
                    has_long_line_truncation: doc.has_long_line_truncation,
                    has_encoding_warnings: doc.has_encoding_warnings,
                }
            }
        }
    }

    fn write_session_dump_if_configured(&mut self) {
        if self.dump_written {
            return;
        }
        let Some(ref target) = self.dump_path else {
            return;
        };
        let target_path = target.clone();
        let dump = self.build_session_dump();
        if let Ok(json) = serde_json::to_string_pretty(&dump) {
            let tmp_path = target_path.with_extension("json.tmp");
            if std::fs::write(&tmp_path, json.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp_path, &target_path);
                self.dump_written = true;
            }
        }
    }

    fn reload_from_disk(&mut self, ctx: &egui::Context) {
        match OpenedTextFile::open(&self.path) {
            Ok(opened) => {
                let c = ctx.clone();
                let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    c.request_repaint();
                });
                let new_backend = match self.mode {
                    PreviewMode::Head => {
                        read_head(&opened, self.requested_lines).map(DocumentBackend::HeadTail)
                    }
                    PreviewMode::Tail => {
                        read_tail(&opened, self.requested_lines).map(DocumentBackend::HeadTail)
                    }
                    PreviewMode::Full => FullDocument::with_repaint_hook(opened, Some(hook))
                        .map(|d| DocumentBackend::Full(Box::new(d))),
                };
                match new_backend {
                    Ok(b) => {
                        self.backend = b;
                        self.reload_error = None;
                        if !self.search_query.is_empty() {
                            self.trigger_search(ctx);
                        }
                    }
                    Err(err) => {
                        self.reload_error = Some(err.to_string());
                    }
                }
            }
            Err(err) => {
                self.reload_error = Some(err.to_string());
            }
        }
    }

    fn trigger_search(&mut self, ctx: &egui::Context) {
        if self.search_query.is_empty() {
            self.search.clear();
            return;
        }
        match &self.backend {
            DocumentBackend::HeadTail(res) => {
                self.search.search_in_memory_lines(
                    &res.lines,
                    &self.search_query,
                    self.search_case_sensitive,
                );
                if let Some((_, m)) = self.search.active_match() {
                    self.pending_scroll_to_line = Some(m.line_index);
                }
            }
            DocumentBackend::Full(doc) => {
                let c = ctx.clone();
                let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                    c.request_repaint();
                });
                if let Err(err) = self.search.start_file_search(
                    doc.opened_file(),
                    &self.search_query,
                    self.search_case_sensitive,
                    Some(hook),
                ) {
                    self.reload_error = Some(format!("Search error: {err}"));
                }
            }
        }
    }

    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        let mut toggle_search = false;
        let mut close_search = false;
        let mut step_next = false;
        let mut step_prev = false;

        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(Key::F) {
                toggle_search = true;
            } else if i.key_pressed(Key::Escape) && self.search_open {
                close_search = true;
            } else if self.search_open && i.key_pressed(Key::Enter) {
                if i.modifiers.shift {
                    step_prev = true;
                } else {
                    step_next = true;
                }
            }
        });

        if toggle_search {
            self.search_open = true;
            self.search_focus_requested = true;
        }
        if close_search {
            self.search_open = false;
            self.search.clear();
        }
        if step_next && let Some(m) = self.search.next_match() {
            self.pending_scroll_to_line = Some(m.line_index);
        }
        if step_prev && let Some(m) = self.search.prev_match() {
            self.pending_scroll_to_line = Some(m.line_index);
        }
    }

    fn apply_visuals(&self, ctx: &egui::Context, pal: &Palette) {
        let mut visuals = if self.is_dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visuals.panel_fill = pal.bg_primary;
        visuals.window_fill = pal.bg_elevated;
        visuals.extreme_bg_color = pal.bg_gutter;
        visuals.override_text_color = Some(pal.fg_primary);
        ctx.set_visuals(visuals);
    }

    fn render_top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pal: &Palette) {
        egui::Frame::NONE
            .fill(pal.bg_elevated)
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.border))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Mode badge pill
                    let badge_text = match self.mode {
                        PreviewMode::Full => "FULL PREVIEW".to_string(),
                        PreviewMode::Head => format!("HEAD · {} LINES", self.requested_lines),
                        PreviewMode::Tail => format!("TAIL · {} LINES", self.requested_lines),
                    };
                    egui::Frame::NONE
                        .fill(pal.accent_bg)
                        .corner_radius(4.0)
                        .inner_margin(Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(badge_text)
                                    .color(pal.accent)
                                    .strong()
                                    .size(11.5),
                            );
                        });

                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(&self.display_name)
                            .color(pal.fg_primary)
                            .strong()
                            .size(14.5),
                    )
                    .on_hover_text(self.path.display().to_string());

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let theme_label = if self.is_dark {
                            "Theme: Light"
                        } else {
                            "Theme: Dark"
                        };
                        if ui.button(theme_label).clicked() {
                            self.is_dark = !self.is_dark;
                        }

                        if ui.button("Reload").clicked() {
                            self.reload_from_disk(ctx);
                        }

                        let find_label = if self.search_open {
                            "Hide Find"
                        } else {
                            "Find (Ctrl+F)"
                        };
                        if ui.button(find_label).clicked() {
                            self.search_open = !self.search_open;
                            if self.search_open {
                                self.search_focus_requested = true;
                            }
                        }

                        if self.mode == PreviewMode::Full {
                            ui.add_space(6.0);
                            let go_clicked = ui.button("Go").clicked();
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.goto_line_input)
                                    .hint_text("Line #")
                                    .desired_width(68.0),
                            );
                            let enter_pressed =
                                resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if (go_clicked || enter_pressed)
                                && let Ok(line_num) = self.goto_line_input.trim().parse::<usize>()
                                && line_num >= 1
                            {
                                self.pending_scroll_to_line = Some(line_num - 1);
                            }
                        }
                    });
                });
            });
    }

    fn render_search_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pal: &Palette) {
        if !self.search_open {
            return;
        }
        egui::Frame::NONE
            .fill(pal.bg_elevated)
            .inner_margin(Margin::symmetric(12, 6))
            .stroke(Stroke::new(1.0, pal.border))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Find:").color(pal.fg_muted).size(12.5));
                    let edit_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Search text… (Enter = Next, Shift+Enter = Prev)")
                            .desired_width(260.0),
                    );
                    if self.search_focus_requested {
                        edit_resp.request_focus();
                        self.search_focus_requested = false;
                    }

                    let mut query_changed = edit_resp.changed();
                    if ui
                        .checkbox(&mut self.search_case_sensitive, "Case sensitive (Aa)")
                        .changed()
                    {
                        query_changed = true;
                    }

                    if query_changed {
                        self.trigger_search(ctx);
                    }

                    if ui.button("Prev").clicked()
                        && let Some(m) = self.search.prev_match()
                    {
                        self.pending_scroll_to_line = Some(m.line_index);
                    }
                    if ui.button("Next").clicked()
                        && let Some(m) = self.search.next_match()
                    {
                        self.pending_scroll_to_line = Some(m.line_index);
                    }

                    let snap = self.search.snapshot();
                    let active_info = self.search.active_match();
                    if !self.search_query.is_empty() {
                        if !snap.complete {
                            let pct = if snap.file_size > 0 {
                                ((snap.scanned_bytes as f64 / snap.file_size as f64) * 100.0)
                                    .clamp(0.0, 99.0)
                            } else {
                                0.0
                            };
                            ui.label(
                                RichText::new(format!(
                                    "Searching {pct:.0}%… ({} matches)",
                                    snap.total_occurrences
                                ))
                                .color(pal.accent)
                                .size(12.0),
                            );
                        } else if snap.total_occurrences == 0 {
                            ui.label(RichText::new("No matches").color(pal.err_fg).size(12.0));
                        } else {
                            let current_num = active_info.map_or(1, |(idx, _)| idx + 1);
                            ui.label(
                                RichText::new(format!(
                                    "{current_num} of {} ({} matching lines)",
                                    snap.matches.len(),
                                    snap.matching_line_count
                                ))
                                .color(pal.fg_muted)
                                .size(12.0),
                            );
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.search_open = false;
                            self.search.clear();
                        }
                    });
                });
            });
    }

    fn render_banners(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, pal: &Palette) {
        let mut error_banners: Vec<String> = Vec::new();
        let mut warn_banners: Vec<String> = Vec::new();

        if let Some(ref err) = self.reload_error {
            error_banners.push(err.clone());
        }

        match &self.backend {
            DocumentBackend::HeadTail(res) => {
                if res.truncated_by_byte_limit {
                    warn_banners.push(
                        "Output truncated: reached 8 MiB safety limit before completing all requested lines."
                            .to_string(),
                    );
                }
                if res.has_long_line_truncation {
                    warn_banners.push(
                        "One or more lines exceeded 64 KiB and were truncated with '…'."
                            .to_string(),
                    );
                }
                if res.has_encoding_warnings {
                    warn_banners.push(
                        "Malformed byte sequences were detected and replaced with '\u{FFFD}'."
                            .to_string(),
                    );
                }
            }
            DocumentBackend::Full(doc) => {
                if let Some(ref b) = doc.truncated_banner {
                    error_banners.push(b.clone());
                }
                if let Some(ref b) = doc.modified_banner {
                    warn_banners.push(b.clone());
                }
                if doc.has_long_line_truncation {
                    warn_banners.push(
                        "One or more lines exceeded 64 KiB and were truncated with '…'."
                            .to_string(),
                    );
                }
                if doc.has_encoding_warnings {
                    warn_banners.push(
                        "Malformed byte sequences were detected and replaced with '\u{FFFD}'."
                            .to_string(),
                    );
                }
            }
        }

        let mut request_reload = false;
        for msg in error_banners {
            egui::Frame::NONE
                .fill(pal.err_bg)
                .inner_margin(Margin::symmetric(12, 5))
                .stroke(Stroke::new(1.0, pal.err_fg))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("[!]").color(pal.err_fg).strong());
                        ui.label(RichText::new(msg).color(pal.err_fg).size(12.5));
                        if ui.button("Reload file").clicked() {
                            request_reload = true;
                        }
                    });
                });
        }

        for msg in warn_banners {
            egui::Frame::NONE
                .fill(pal.warn_bg)
                .inner_margin(Margin::symmetric(12, 5))
                .stroke(Stroke::new(1.0, pal.warn_fg))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("[i]").color(pal.warn_fg).strong());
                        ui.label(RichText::new(msg).color(pal.warn_fg).size(12.5));
                    });
                });
        }

        if request_reload {
            self.reload_from_disk(ctx);
        }
    }

    fn render_status_bar(&self, ui: &mut egui::Ui, pal: &Palette) {
        let (file_size, encoding, has_crlf, lines_summary) = match &self.backend {
            DocumentBackend::HeadTail(res) => {
                let summary = if res.line_numbers_are_relative {
                    format!("Showing {} tail lines", res.lines.len())
                } else {
                    format!("Showing {} lines", res.lines.len())
                };
                (res.file_size, res.encoding, res.has_crlf, summary)
            }
            DocumentBackend::Full(doc) => {
                let snap = doc.index_snapshot();
                let summary = if snap.complete {
                    format!("{} lines", snap.total_lines)
                } else {
                    let pct = if snap.file_size > 0 {
                        ((snap.indexed_bytes as f64 / snap.file_size as f64) * 100.0)
                            .clamp(0.0, 99.0)
                    } else {
                        0.0
                    };
                    format!("Indexing {pct:.0}% ({} lines…)", snap.total_lines)
                };
                (
                    doc.file_size(),
                    doc.opened_file().encoding,
                    doc.opened_file().has_crlf,
                    summary,
                )
            }
        };

        egui::Frame::NONE
            .fill(pal.bg_elevated)
            .inner_margin(Margin::symmetric(12, 5))
            .stroke(Stroke::new(1.0, pal.border))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.path.display().to_string())
                            .color(pal.fg_muted)
                            .size(11.5),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        status_pill(ui, "READ-ONLY", pal);
                        status_pill(ui, if has_crlf { "CRLF" } else { "LF" }, pal);
                        status_pill(ui, encoding.label(), pal);
                        status_pill(ui, &format_byte_size(file_size), pal);
                        status_pill(ui, &lines_summary, pal);
                    });
                });
            });
    }

    fn render_document_body(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let row_height = 19.0f32;
        let active_match_line = self.search.active_match().map(|(_, m)| m.line_index);
        let query = if self.search_open {
            self.search_query.clone()
        } else {
            String::new()
        };
        let case_sensitive = self.search_case_sensitive;

        let total_rows = match &self.backend {
            DocumentBackend::HeadTail(res) => res.lines.len(),
            DocumentBackend::Full(doc) => doc.index_snapshot().total_lines,
        };

        if total_rows == 0 {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Empty file (0 lines)")
                        .color(pal.fg_muted)
                        .size(14.0),
                );
            });
            return;
        }

        let mut scroll_area = ScrollArea::both().auto_shrink([false, false]);
        if let Some(target_line) = self.pending_scroll_to_line.take() {
            let target_y = (target_line as f32 * row_height).max(0.0);
            scroll_area =
                scroll_area.vertical_scroll_offset(target_y.saturating_sub_f32(row_height * 4.0));
        }

        scroll_area.show_rows(ui, row_height, total_rows, |ui, row_range| {
            let visible_lines: Vec<PreviewLine> = match &mut self.backend {
                DocumentBackend::HeadTail(res) => {
                    let start = row_range.start.min(res.lines.len());
                    let end = row_range.end.min(res.lines.len());
                    res.lines[start..end].to_vec()
                }
                DocumentBackend::Full(doc) => doc.fetch_lines(row_range.start, row_range.len()),
            };

            let max_line_num = visible_lines
                .last()
                .map_or(total_rows as u64, |l| l.line_number);
            let digits = max_line_num.max(1).ilog10() as usize + 1;
            let gutter_width = ((digits.max(4) as f32) * 8.5) + 18.0;

            for (rel_idx, line) in visible_lines.iter().enumerate() {
                let abs_row_idx = row_range.start + rel_idx;
                let is_active_match_row = active_match_line == Some(abs_row_idx);

                let row_bg = if is_active_match_row {
                    pal.active_row_bg
                } else {
                    pal.bg_primary
                };

                egui::Frame::NONE.fill(row_bg).show(ui, |ui| {
                    ui.set_min_height(row_height);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;

                        // Line number gutter
                        egui::Frame::NONE
                            .fill(pal.bg_gutter)
                            .inner_margin(Margin::symmetric(8, 0))
                            .show(ui, |ui| {
                                ui.set_min_width(gutter_width);
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(format!("{}", line.line_number))
                                            .monospace()
                                            .size(12.5)
                                            .color(pal.fg_muted),
                                    );
                                });
                            });

                        ui.add_space(10.0);

                        // Monospace line text (with search highlight spans if a query is active)
                        let job = build_line_layout_job(
                            &line.text,
                            &query,
                            case_sensitive,
                            is_active_match_row,
                            pal,
                        );
                        ui.add(egui::Label::new(job).extend());
                    });
                });
            }
        });
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if let Some(limit) = self.auto_close_after
            && self.started_at.elapsed() >= limit
        {
            self.write_session_dump_if_configured();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if let DocumentBackend::Full(ref mut doc) = self.backend {
            doc.poll_disk_state_if_due();
            if !doc.index_snapshot().complete {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        if self.search_open && !self.search.snapshot().complete {
            if self.pending_scroll_to_line.is_none()
                && let Some((_, m)) = self.search.active_match()
            {
                self.pending_scroll_to_line = Some(m.line_index);
            }
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        let pal = if self.is_dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        self.apply_visuals(&ctx, &pal);
        self.handle_keyboard_shortcuts(&ctx);

        egui::Panel::top("viewer_top_panel")
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.render_top_bar(ui, &ctx, &pal);
                self.render_search_bar(ui, &ctx, &pal);
                self.render_banners(ui, &ctx, &pal);
            });

        egui::Panel::bottom("viewer_bottom_panel")
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.render_status_bar(ui, &pal);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg_primary)
                    .inner_margin(Margin::symmetric(0, 2)),
            )
            .show(ui, |ui| {
                self.render_document_body(ui, &pal);
            });

        self.write_session_dump_if_configured();

        if self.auto_close_after.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}

trait SaturatingSubF32 {
    fn saturating_sub_f32(self, rhs: f32) -> f32;
}

impl SaturatingSubF32 for f32 {
    fn saturating_sub_f32(self, rhs: f32) -> f32 {
        (self - rhs).max(0.0)
    }
}

fn status_pill(ui: &mut egui::Ui, text: &str, pal: &Palette) {
    egui::Frame::NONE
        .fill(pal.bg_gutter)
        .corner_radius(3.0)
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(pal.fg_muted).size(11.0));
        });
}

fn build_line_layout_job(
    line: &str,
    query: &str,
    case_sensitive: bool,
    is_active_row: bool,
    pal: &Palette,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = FontId::new(13.0, FontFamily::Monospace);
    let normal_fmt = TextFormat {
        font_id: font_id.clone(),
        color: pal.fg_primary,
        ..Default::default()
    };

    if query.is_empty() {
        job.append(line, 0.0, normal_fmt);
        return job;
    }

    let ranges = find_highlight_ranges_in_line(line, query, case_sensitive);
    if ranges.is_empty() {
        job.append(line, 0.0, normal_fmt);
        return job;
    }

    let highlight_fmt = TextFormat {
        font_id,
        color: if is_active_row {
            Color32::BLACK
        } else {
            pal.fg_primary
        },
        background: if is_active_row {
            pal.active_match_bg
        } else {
            pal.match_bg
        },
        ..Default::default()
    };

    let mut cursor = 0usize;
    for (start, end) in ranges {
        if start > cursor && line.is_char_boundary(cursor) && line.is_char_boundary(start) {
            job.append(&line[cursor..start], 0.0, normal_fmt.clone());
        }
        if end > start && line.is_char_boundary(start) && line.is_char_boundary(end) {
            job.append(&line[start..end], 0.0, highlight_fmt.clone());
            cursor = end;
        }
    }
    if cursor < line.len() && line.is_char_boundary(cursor) {
        job.append(&line[cursor..], 0.0, normal_fmt);
    }

    job
}

/// Launches the native `eframe` desktop window for `opened`.
pub fn run_viewer_window(
    opened: OpenedTextFile,
    mode: PreviewMode,
    requested_lines: usize,
    custom_title: Option<String>,
    theme: ViewerTheme,
) -> Result<(), String> {
    let window_title = custom_title.unwrap_or_else(|| match mode {
        PreviewMode::Full => format!("{} — Radivine Full Preview", opened.display_name),
        PreviewMode::Head => format!(
            "{} — Radivine Head ({requested_lines} lines)",
            opened.display_name
        ),
        PreviewMode::Tail => format!(
            "{} — Radivine Tail ({requested_lines} lines)",
            opened.display_name
        ),
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(window_title)
            .with_app_id("radivine-viewer")
            .with_inner_size(Vec2::new(1020.0, 700.0))
            .with_min_inner_size(Vec2::new(520.0, 340.0)),
        ..Default::default()
    };

    let mut init_err: Option<String> = None;
    let mut opened_opt = Some(opened);

    let eframe_res = eframe::run_native(
        "radivine-viewer",
        native_options,
        Box::new(|cc| {
            let file = opened_opt.take().expect("opened file taken once");
            match ViewerApp::new(
                file,
                mode,
                requested_lines,
                theme,
                Some(cc.egui_ctx.clone()),
            ) {
                Ok(app) => Ok(Box::new(app)),
                Err(err) => {
                    init_err = Some(err.to_string());
                    Err(Box::new(err))
                }
            }
        }),
    );

    if let Some(err) = init_err {
        return Err(err);
    }
    eframe_res.map_err(|e| e.to_string())
}

/// Unused helper import check suppression for [`DetectedEncoding`].
const _: Option<DetectedEncoding> = None;
