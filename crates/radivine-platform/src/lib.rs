//! `radivine-platform` — Platform abstraction and X11 backend (`PLATFORM_INTEGRATION.md`).
//!
//! Isolates all windowing, MIT-SHM pixel presentation, SHAPE input regions, RandR monitor/DPI
//! queries, Invoke-mode pointer/keyboard grabs, and future XDND protocol handling behind the
//! [`Platform`] trait.

use std::fmt;
use std::path::PathBuf;
use std::time::Instant;

use smallvec::SmallVec;

pub mod x11;

/// Level of platform support for a capability (`PLATFORM_INTEGRATION.md` §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Fully supported with native fidelity.
    Yes,
    /// Supported with a documented degradation or limitation.
    Degraded(&'static str),
    /// Not available on this platform/session.
    No(&'static str),
}

impl fmt::Display for Support {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yes => f.write_str("yes"),
            Self::Degraded(reason) => write!(f, "degraded ({reason})"),
            Self::No(reason) => write!(f, "no ({reason})"),
        }
    }
}

/// Capabilities reported by a [`Platform`] backend (`PLATFORM_INTEGRATION.md` §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Mid-drag modifier trigger + overlay acting as the XDND drop target.
    pub drag_session: Support,
    /// Invoke-mode wheel positioned at the pointer.
    pub invoke_at_pointer: Support,
    /// Per-pixel 32-bit ARGB translucency.
    pub translucency: Support,
    /// Input pass-through outside the wheel shapes (`SHAPE` input region).
    pub input_passthrough: Support,
    /// Keyboard navigation during drag mode.
    pub keyboard_in_drag: Support,
    /// Owning the system clipboard with file lists (`Copy` action).
    pub clipboard_files: Support,
}

/// Point in physical screen or overlay pixels (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysPoint {
    /// X coordinate in physical pixels.
    pub x: i32,
    /// Y coordinate in physical pixels.
    pub y: i32,
}

impl PhysPoint {
    /// Creates a new [`PhysPoint`].
    #[inline]
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Axis-aligned rectangle in physical pixels (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RectPx {
    /// Left X coordinate in physical pixels.
    pub x: i32,
    /// Top Y coordinate in physical pixels.
    pub y: i32,
    /// Width in physical pixels.
    pub w: u32,
    /// Height in physical pixels.
    pub h: u32,
}

impl RectPx {
    /// Creates a new [`RectPx`].
    #[inline]
    #[must_use]
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Returns `true` if `pt` lies inside `[x, x + w) × [y, y + h)`.
    #[inline]
    #[must_use]
    pub fn contains(self, pt: PhysPoint) -> bool {
        pt.x >= self.x
            && pt.y >= self.y
            && pt.x < self.x.saturating_add(self.w as i32)
            && pt.y < self.y.saturating_add(self.h as i32)
    }
}

/// Physical-pixel input-region shape primitive (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Debug, PartialEq)]
pub enum ShapePx {
    /// Circular region in overlay-local physical pixels.
    Circle {
        /// Center X in physical pixels.
        cx: f32,
        /// Center Y in physical pixels.
        cy: f32,
        /// Radius in physical pixels.
        r: f32,
    },
    /// Convex polygon in overlay-local physical pixels.
    Polygon(SmallVec<[(f32, f32); 6]>),
}

/// Connected display monitor geometry and refresh rate (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// Backend monitor identifier.
    pub id: u32,
    /// Output name (e.g. `"HDMI-0"`, `"DP-0"`).
    pub name: String,
    /// Monitor bounds in root physical pixels.
    pub rect: RectPx,
    /// Refresh rate in millihertz (e.g. `60_000` for 60 Hz).
    pub refresh_mhz: u32,
    /// Whether this monitor is marked primary.
    pub primary: bool,
}

/// Overlay configuration returned by [`Platform::prepare_overlay`] (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayInfo {
    /// Top-left origin of the overlay window in root physical pixels.
    pub origin: PhysPoint,
    /// Overlay dimensions `(width, height)` in physical pixels.
    pub size: (u32, u32),
    /// Effective UI scale factor (`Xft.dpi / 96.0` or user override).
    pub scale: f32,
    /// Monitor on which the overlay is placed.
    pub monitor: Monitor,
    /// Whether per-pixel alpha translucency is active.
    pub translucent: bool,
}

/// Borrowed reference to a premultiplied RGBA8 framebuffer (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Copy, Debug)]
pub struct FrameRef<'a> {
    /// Premultiplied RGBA8 pixel bytes (`height * stride`).
    pub data: &'a [u8],
    /// Framebuffer width in physical pixels.
    pub width: u32,
    /// Framebuffer height in physical pixels.
    pub height: u32,
    /// Row stride in bytes (`width * 4`).
    pub stride: usize,
}

/// Drag source application metadata (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceInfo {
    /// Process executable name (e.g. `Some("nemo".into())`).
    pub app: Option<String>,
    /// Process ID, if known.
    pub pid: Option<u32>,
}

/// Logical key translated by the platform in Invoke mode (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Tab.
    Tab,
    /// Shift+Tab.
    ShiftTab,
    /// Enter / Return.
    Enter,
    /// Spacebar.
    Space,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Direct 1-based digit (`1`..=`9`).
    Digit(u8),
}

/// Clipboard operation kind (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipOp {
    /// Copy operation.
    Copy,
}

/// Clipboard payload passed to [`Platform::set_clipboard`] (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardContent {
    /// File URIs (`file:///...`).
    pub uris: Vec<String>,
    /// Local filesystem paths.
    pub local_paths: Vec<PathBuf>,
    /// Operation (`ClipOp::Copy`).
    pub operation: ClipOp,
}

/// Error retrieving an XDND selection payload (`PLATFORM_INTEGRATION.md` §3.8, §7).
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum PayloadError {
    /// Source refused the selection conversion (`property == None`).
    #[error("selection conversion refused by source")]
    Refused,
    /// Selection transfer timed out (`> 2 s`).
    #[error("selection retrieval timed out")]
    Timeout,
    /// Payload exceeded the 16 MiB safety cap.
    #[error("selection payload exceeded 16 MiB limit")]
    TooLarge,
    /// Protocol or property format error.
    #[error("selection protocol error: {0}")]
    Protocol(String),
}

/// Error returned by [`Platform`] operations (`PLATFORM_INTEGRATION.md` §7).
#[derive(thiserror::Error, Debug)]
pub enum PlatformError {
    /// Could not connect to the display server.
    #[error("failed to connect to display server: {0}")]
    Connect(String),
    /// X11 protocol or connection error.
    #[error("X11 protocol error: {0}")]
    X11(String),
    /// Operating system / shared memory error.
    #[error("OS error: {0}")]
    Os(String),
    /// Unsupported session type (e.g. pure Wayland without X11).
    #[error("unsupported display session: {0}")]
    Unsupported(String),
}

/// Event emitted by [`Platform::dispatch_pending`] and [`Platform::on_deadline`] (`PLATFORM_INTEGRATION.md` §7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    /// Another client acquired `XdndSelection` while a mouse button is held.
    DragArmed {
        /// Drag source metadata.
        source: SourceInfo,
    },
    /// Active drag ended or disarmed.
    DragDisarmed,
    /// Fresh trigger modifier press detected while armed.
    Trigger {
        /// Root physical pointer coordinates at trigger time.
        at: PhysPoint,
    },
    /// Trigger modifier released.
    TriggerReleased,
    /// `XdndEnter` received on the overlay window.
    DndEnter {
        /// Offered MIME type names.
        types: Vec<String>,
    },
    /// `XdndPosition` received at root physical coordinates `at`.
    DndPosition {
        /// Root physical pointer coordinates.
        at: PhysPoint,
    },
    /// `XdndLeave` received (`drag_ended == true` if button was released or source vanished).
    DndLeave {
        /// Whether the underlying drag session has ended.
        drag_ended: bool,
    },
    /// `XdndDrop` received on the overlay window.
    DndDrop,
    /// Selection payload retrieval completed.
    Payload {
        /// Requested MIME type.
        mime: String,
        /// Retrieved bytes or [`PayloadError`].
        result: Result<Vec<u8>, PayloadError>,
    },
    /// Pointer moved in Invoke mode (`at` in root physical pixels).
    PointerMoved {
        /// Root physical pointer coordinates.
        at: PhysPoint,
    },
    /// Mouse button pressed or released in Invoke mode (`at` in root physical pixels).
    Button {
        /// X11 button number (`1` = primary, `2` = middle, `3` = secondary).
        button: u8,
        /// `true` on press, `false` on release.
        pressed: bool,
        /// Root physical pointer coordinates.
        at: PhysPoint,
    },
    /// Keyboard navigation key pressed or released in Invoke mode.
    Key {
        /// Translated navigation key.
        key: Key,
        /// `true` on press, `false` on release.
        pressed: bool,
    },
    /// Monitor layout or resolution changed (`RandR`).
    MonitorsChanged,
    /// Desktop settings (`Xft.dpi` / `RESOURCE_MANAGER`) changed.
    SettingsChanged,
    /// Display server connection was lost.
    ConnectionLost,
}

/// EWMH window type used for the X11 overlay (`ARCHITECTURE.md` §7.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayWindowType {
    /// `_NET_WM_WINDOW_TYPE_DND` (default; verified 169 µs map with zero Muffin animations in M0 S-5).
    #[default]
    Dnd,
    /// `_NET_WM_WINDOW_TYPE_POPUP_MENU`.
    PopupMenu,
    /// `_NET_WM_WINDOW_TYPE_NOTIFICATION`.
    Notification,
    /// `_NET_WM_WINDOW_TYPE_UTILITY`.
    Utility,
}

/// Trigger detection strategy for X11 drag mode (`ARCHITECTURE.md` §7.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TriggerDetectionMode {
    /// XI2 `RawKeyPress` / `RawKeyRelease` while armed (default).
    #[default]
    Raw,
    /// 8 ms `QueryPointer` polling while armed.
    Poll,
}

/// Modifier key used to summon the radial wheel during an active drag (`ARCHITECTURE.md` §7.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TriggerModifier {
    /// `Shift` (`Shift_L` / `Shift_R`, default).
    #[default]
    Shift,
    /// `Ctrl` (`Control_L` / `Control_R`).
    Ctrl,
    /// `Alt` (`Mod1`).
    Alt,
    /// `Super` (`Mod4`).
    Super,
    /// Drag-mode trigger disabled.
    Disabled,
}

/// Configuration for the X11 platform backend (`ARCHITECTURE.md` §7.4).
#[derive(Clone, Debug)]
pub struct X11Config {
    /// Trigger detection mode (`Raw` or `Poll`).
    pub trigger_detection: TriggerDetectionMode,
    /// Modifier key that triggers the wheel while a drag is armed (`Shift` default).
    pub trigger_modifier: TriggerModifier,
    /// EWMH window type for the overlay (`Dnd` default).
    pub window_type: OverlayWindowType,
    /// Whether to issue a 0-px relative `WarpPointer` nudge on map/unmap (`true` default).
    pub pointer_nudge: bool,
}

impl Default for X11Config {
    fn default() -> Self {
        Self {
            trigger_detection: TriggerDetectionMode::Raw,
            trigger_modifier: TriggerModifier::Shift,
            window_type: OverlayWindowType::Dnd,
            pointer_nudge: true,
        }
    }
}

/// Platform backend interface (`PLATFORM_INTEGRATION.md` §7).
pub trait Platform {
    /// Backend identifier (e.g. `"x11"`).
    fn name(&self) -> &'static str;

    /// Returns the probed [`Capabilities`] of this backend.
    fn capabilities(&self) -> &Capabilities;

    /// Returns the pollable file descriptor registered with `calloop`.
    #[cfg(unix)]
    fn poll_fd(&self) -> std::os::fd::RawFd;

    /// Polls and translates all pending display server events into `out`.
    fn dispatch_pending(&mut self, out: &mut Vec<PlatformEvent>) -> Result<(), PlatformError>;

    /// Returns the list of active monitors.
    fn monitors(&self) -> &[Monitor];

    /// Returns the auto-detected UI scale factor (`Xft.dpi / 96.0`).
    fn scale(&self) -> f32;

    /// Returns desktop reduced-motion preference if known.
    fn reduced_motion(&self) -> Option<bool>;

    /// Queries the current global pointer position in root physical pixels.
    fn pointer_position(&mut self) -> Result<PhysPoint, PlatformError>;

    /// Returns the underlying platform window ID of the overlay (e.g. X11 `Window` XID) for diagnostics/tests.
    fn overlay_window_id(&self) -> u32;

    /// Configures the overlay window to cover the monitor containing `at` (remains unmapped).
    fn prepare_overlay(&mut self, at: PhysPoint) -> Result<OverlayInfo, PlatformError>;

    /// Raises and maps the overlay window.
    fn map_overlay(&mut self) -> Result<(), PlatformError>;

    /// Uploads the damaged rectangles of `frame` to the overlay window.
    fn present(&mut self, frame: FrameRef<'_>, damage: &[RectPx]) -> Result<(), PlatformError>;

    /// Sets the overlay's input region (`None` = whole overlay captures input; `Some(&[])` = pass-through all).
    fn set_input_region(&mut self, shapes: Option<&[ShapePx]>) -> Result<(), PlatformError>;

    /// Sets the overlay's bounding region (`None` = whole overlay; `Some(shapes)` = 1-bit `ShapeBounding`
    /// clipping mask for the no-compositor fallback per `PLATFORM_INTEGRATION.md` §3.11).
    fn set_bounding_region(&mut self, _shapes: Option<&[ShapePx]>) -> Result<(), PlatformError> {
        Ok(())
    }

    /// Clears the input region and unmaps the overlay window.
    fn hide_overlay(&mut self);

    /// Detaches and frees shared-memory pixel buffers when idle.
    fn release_buffers(&mut self);

    /// Sends an `XdndStatus` reply (`accept = true` or `false`) to the active drag source.
    fn dnd_reply(&mut self, accept: bool);

    /// Requests the drag payload for `mime` (e.g. `"text/uri-list"`).
    fn request_payload(&mut self, mime: &str);

    /// Sends `XdndFinished` to complete a drop.
    fn dnd_finish(&mut self, success: bool);

    /// Acquires pointer and keyboard grabs on the mapped overlay for Invoke mode.
    fn grab_input(&mut self) -> Result<(), PlatformError>;

    /// Releases any active Invoke-mode pointer and keyboard grabs.
    fn ungrab_input(&mut self);

    /// Claims `CLIPBOARD` ownership with `content`.
    fn set_clipboard(&mut self, content: ClipboardContent) -> Result<(), PlatformError>;

    /// Flushes outgoing protocol requests to the display server.
    fn flush(&mut self) -> Result<(), PlatformError>;

    /// Returns the earliest backend-internal timer deadline, if any.
    fn next_deadline(&self) -> Option<Instant>;

    /// Fires any backend-internal timers due at `now`.
    fn on_deadline(&mut self, now: Instant, out: &mut Vec<PlatformEvent>);
}

/// Connects to the default platform backend using default [`X11Config`] (`PLATFORM_INTEGRATION.md` §7).
pub fn connect_default() -> Result<Box<dyn Platform>, PlatformError> {
    connect_with_config(X11Config::default())
}

/// Connects to the platform backend with the provided [`X11Config`].
pub fn connect_with_config(cfg: X11Config) -> Result<Box<dyn Platform>, PlatformError> {
    let plat = x11::X11Platform::connect(cfg)?;
    Ok(Box::new(plat))
}

/// Verifies whether the current `DISPLAY` is a dedicated isolated virtual X11 display (`Xvfb`)
/// rather than the user's host desktop (`:0`, `:1`).
///
/// Automated X11 tests MUST call this before connecting or running `xdotool` so they never
/// interfere with the user's active desktop session (`TESTING_STRATEGY.md` §4).
#[must_use]
pub fn is_isolated_test_display() -> bool {
    let Ok(display) = std::env::var("DISPLAY") else {
        return false;
    };
    let trimmed = display.trim();
    let Some(after_colon) = trimmed.strip_prefix(':') else {
        return false;
    };
    let num_str = after_colon.split('.').next().unwrap_or("");
    let Ok(display_num) = num_str.parse::<u32>() else {
        return false;
    };
    // Never touch host physical displays (:0, :1)
    if display_num <= 1 {
        return false;
    }

    // Verify via /tmp/.X<N>-lock -> /proc/<pid>/comm that the server is Xvfb
    let lock_path = format!("/tmp/.X{display_num}-lock");
    if let Ok(lock_contents) = std::fs::read_to_string(&lock_path)
        && let Ok(pid) = lock_contents.trim().parse::<u32>()
        && let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        && comm.trim().eq_ignore_ascii_case("Xvfb")
    {
        return true;
    }

    false
}
