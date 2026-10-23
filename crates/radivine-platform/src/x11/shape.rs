//! X11 `SHAPE` input-region rasterisation and application (`PLATFORM_INTEGRATION.md` §3.4).

use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    ClipOrdering, ConnectionExt as _, CreateGCAux, Gcontext, Pixmap, Rectangle, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::{PlatformError, ShapePx};

/// Rasterises a slice of [`ShapePx`] primitives (`Circle` and convex `Polygon`) into horizontal
/// 1-px-high scanline [`Rectangle`] spans clipped to `[0, width) × [0, height)`.
#[must_use]
pub fn rasterize_shapes_to_rectangles(
    shapes: &[ShapePx],
    width: u16,
    height: u16,
) -> Vec<Rectangle> {
    let w_i32 = i32::from(width);
    let h_i32 = i32::from(height);
    if w_i32 <= 0 || h_i32 <= 0 || shapes.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(shapes.len() * 128);

    for shape in shapes {
        match shape {
            ShapePx::Circle { cx, cy, r } => {
                if *r <= 0.0 {
                    continue;
                }
                let y_min = ((cy - r).floor() as i32).clamp(0, h_i32 - 1);
                let y_max = ((cy + r).ceil() as i32).clamp(0, h_i32 - 1);
                let r_sq = r * r;

                for y in y_min..=y_max {
                    let dy = (y as f32) + 0.5 - cy;
                    let rem = r_sq - dy * dy;
                    if rem < 0.0 {
                        continue;
                    }
                    let dx = rem.sqrt();
                    let x0 = ((cx - dx).floor() as i32).clamp(0, w_i32 - 1);
                    let x1 = ((cx + dx).ceil() as i32).clamp(0, w_i32 - 1);
                    if x1 >= x0 {
                        out.push(Rectangle {
                            x: x0 as i16,
                            y: y as i16,
                            width: (x1 - x0 + 1) as u16,
                            height: 1,
                        });
                    }
                }
            }
            ShapePx::Polygon(pts) => {
                if pts.len() < 3 {
                    continue;
                }
                let mut min_y = f32::INFINITY;
                let mut max_y = f32::NEG_INFINITY;
                for &(_px, py) in pts {
                    min_y = min_y.min(py);
                    max_y = max_y.max(py);
                }
                let y_min = (min_y.floor() as i32).clamp(0, h_i32 - 1);
                let y_max = (max_y.ceil() as i32).clamp(0, h_i32 - 1);

                for y in y_min..=y_max {
                    let sy = (y as f32) + 0.5;
                    let mut x_min_span = f32::INFINITY;
                    let mut x_max_span = f32::NEG_INFINITY;

                    for i in 0..pts.len() {
                        let (x0, y0) = pts[i];
                        let (x1, y1) = pts[(i + 1) % pts.len()];
                        if (y0 <= sy && y1 >= sy) || (y1 <= sy && y0 >= sy) {
                            let dy = y1 - y0;
                            if dy.abs() <= 1e-5 {
                                x_min_span = x_min_span.min(x0.min(x1));
                                x_max_span = x_max_span.max(x0.max(x1));
                            } else {
                                let t = ((sy - y0) / dy).clamp(0.0, 1.0);
                                let ix = x0 + t * (x1 - x0);
                                x_min_span = x_min_span.min(ix);
                                x_max_span = x_max_span.max(ix);
                            }
                        }
                    }

                    if x_max_span >= x_min_span {
                        let x0 = (x_min_span.floor() as i32).clamp(0, w_i32 - 1);
                        let x1 = (x_max_span.ceil() as i32).clamp(0, w_i32 - 1);
                        if x1 >= x0 {
                            out.push(Rectangle {
                                x: x0 as i16,
                                y: y as i16,
                                width: (x1 - x0 + 1) as u16,
                                height: 1,
                            });
                        }
                    }
                }
            }
        }
    }

    out
}

/// Applies `shapes` to `win`'s `ShapeInput` region (`PLATFORM_INTEGRATION.md` §3.4):
/// - `None`: resets `ShapeInput` to the full window (`x11rb::NONE`).
/// - `Some(&[])`: sets an empty `ShapeInput` rectangle list so all pointer events pass through.
/// - `Some(non_empty)`: rasterises `non_empty` into a 1-bit `Pixmap` and applies `shape_mask(SO::SET, SK::INPUT)`.
pub fn apply_input_region(
    conn: &RustConnection,
    win: Window,
    width: u16,
    height: u16,
    shapes: Option<&[ShapePx]>,
) -> Result<(), PlatformError> {
    apply_shape_region(conn, win, width, height, shapes, SK::INPUT)
}

/// Applies `shapes` to `win`'s `ShapeBounding` region (`PLATFORM_INTEGRATION.md` §3.11 no-compositor fallback):
/// - `None`: resets `ShapeBounding` to the full window (`x11rb::NONE`).
/// - `Some(&[])`: sets an empty `ShapeBounding` rectangle list.
/// - `Some(non_empty)`: rasterises `non_empty` into a 1-bit `Pixmap` and applies `shape_mask(SO::SET, SK::BOUNDING)`.
pub fn apply_bounding_region(
    conn: &RustConnection,
    win: Window,
    width: u16,
    height: u16,
    shapes: Option<&[ShapePx]>,
) -> Result<(), PlatformError> {
    apply_shape_region(conn, win, width, height, shapes, SK::BOUNDING)
}

fn apply_shape_region(
    conn: &RustConnection,
    win: Window,
    width: u16,
    height: u16,
    shapes: Option<&[ShapePx]>,
    kind: SK,
) -> Result<(), PlatformError> {
    let Some(shape_slice) = shapes else {
        conn.shape_mask(SO::SET, kind, win, 0, 0, x11rb::NONE)
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        return Ok(());
    };

    if shape_slice.is_empty() {
        return clear_shape_region_empty(conn, win, kind);
    }

    let w = width.max(1);
    let h = height.max(1);
    let rects = rasterize_shapes_to_rectangles(shape_slice, w, h);
    if rects.is_empty() {
        return clear_shape_region_empty(conn, win, kind);
    }

    let pixmap: Pixmap = conn
        .generate_id()
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    conn.create_pixmap(1, pixmap, win, w, h)
        .map_err(|e| PlatformError::X11(e.to_string()))?;

    let gc_clear: Gcontext = conn
        .generate_id()
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    let gc_set: Gcontext = conn
        .generate_id()
        .map_err(|e| PlatformError::X11(e.to_string()))?;

    conn.create_gc(gc_clear, pixmap, &CreateGCAux::new().foreground(0))
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    conn.create_gc(gc_set, pixmap, &CreateGCAux::new().foreground(1))
        .map_err(|e| PlatformError::X11(e.to_string()))?;

    conn.poly_fill_rectangle(
        pixmap,
        gc_clear,
        &[Rectangle {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }],
    )
    .map_err(|e| PlatformError::X11(e.to_string()))?;

    for chunk in rects.chunks(1000) {
        conn.poly_fill_rectangle(pixmap, gc_set, chunk)
            .map_err(|e| PlatformError::X11(e.to_string()))?;
    }

    conn.shape_mask(SO::SET, kind, win, 0, 0, pixmap)
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    let _ = conn.free_gc(gc_clear);
    let _ = conn.free_gc(gc_set);
    let _ = conn.free_pixmap(pixmap);
    Ok(())
}

/// Sets `win`'s `ShapeInput` region to an empty rectangle list so all events pass through.
pub fn clear_input_region_empty(conn: &RustConnection, win: Window) -> Result<(), PlatformError> {
    clear_shape_region_empty(conn, win, SK::INPUT)
}

/// Resets `win`'s `ShapeBounding` region to the full window (`x11rb::NONE`).
pub fn reset_bounding_region(conn: &RustConnection, win: Window) -> Result<(), PlatformError> {
    conn.shape_mask(SO::SET, SK::BOUNDING, win, 0, 0, x11rb::NONE)
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    Ok(())
}

fn clear_shape_region_empty(
    conn: &RustConnection,
    win: Window,
    kind: SK,
) -> Result<(), PlatformError> {
    conn.shape_rectangles(SO::SET, kind, ClipOrdering::UNSORTED, win, 0, 0, &[])
        .map_err(|e| PlatformError::X11(e.to_string()))?;
    Ok(())
}
