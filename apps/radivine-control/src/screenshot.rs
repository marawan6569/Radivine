//! Pure-Rust software rasterizer for `egui::ClippedPrimitive` triangle meshes (`epaint::Mesh`).
//!
//! Renders the actual `ControlCenterApp` `egui` frames directly to PNG files without opening an
//! X11 window on the host display (`Section 13` & `Section 17`).

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{
    self, Color32, Context, ImageData, Pos2, RawInput, Rect, TextureId, Vec2, epaint::Primitive,
};
use tiny_skia::Pixmap;

struct SoftTexture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

impl SoftTexture {
    fn sample(&self, u: f32, v: f32) -> Color32 {
        if self.width == 0 || self.height == 0 {
            return Color32::WHITE;
        }
        let x =
            ((u.clamp(0.0, 1.0) * (self.width as f32 - 1.0)).round() as usize).min(self.width - 1);
        let y = ((v.clamp(0.0, 1.0) * (self.height as f32 - 1.0)).round() as usize)
            .min(self.height - 1);
        self.pixels[y * self.width + x]
    }
}

/// Software rasterizer state maintaining `egui` managed textures across frames.
#[derive(Default)]
pub struct EguiSoftRasterizer {
    textures: HashMap<TextureId, SoftTexture>,
}

impl EguiSoftRasterizer {
    /// Creates a new software rasterizer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `ui_fn` for two settled frames at `size_points` and saves the rendered PNG to `out_path`.
    pub fn render_to_png<F>(
        &mut self,
        ctx: &Context,
        size_points: Vec2,
        pixels_per_point: f32,
        out_path: &Path,
        mut ui_fn: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(&mut egui::Ui),
    {
        let screen_rect = Rect::from_min_size(Pos2::ZERO, size_points);
        let mut last_output = None;

        for frame_idx in 0..2 {
            let raw_input = RawInput {
                screen_rect: Some(screen_rect),
                time: Some(f64::from(frame_idx) * 0.016),
                ..Default::default()
            };
            ctx.set_pixels_per_point(pixels_per_point);
            let output = ctx.run_ui(raw_input, |ui| {
                ui_fn(ui);
            });
            self.apply_textures_delta(&output.textures_delta);
            last_output = Some(output);
        }

        let output = last_output.expect("two frames executed");
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);

        let width_px = (size_points.x * pixels_per_point).round() as u32;
        let height_px = (size_points.y * pixels_per_point).round() as u32;
        let mut pixmap = Pixmap::new(width_px, height_px)
            .ok_or_else(|| anyhow::anyhow!("invalid screenshot dimensions"))?;

        for clipped in &primitives {
            if let Primitive::Mesh(ref mesh) = clipped.primitive {
                self.rasterize_mesh(
                    &mut pixmap,
                    clipped.clip_rect,
                    mesh,
                    output.pixels_per_point,
                );
            }
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        pixmap.save_png(out_path)?;
        Ok(())
    }

    fn apply_textures_delta(&mut self, delta: &egui::TexturesDelta) {
        for (tex_id, img_deltas) in &delta.set {
            for img_delta in img_deltas {
                let ImageData::Color(color_img) = &img_delta.image;
                let [w, h] = color_img.size;
                if let Some([ox, oy]) = img_delta.pos {
                    if let Some(existing) = self.textures.get_mut(tex_id) {
                        for row in 0..h {
                            let dst_y = oy + row;
                            if dst_y >= existing.height {
                                continue;
                            }
                            for col in 0..w {
                                let dst_x = ox + col;
                                if dst_x >= existing.width {
                                    continue;
                                }
                                existing.pixels[dst_y * existing.width + dst_x] =
                                    color_img.pixels[row * w + col];
                            }
                        }
                    }
                } else {
                    self.textures.insert(
                        *tex_id,
                        SoftTexture {
                            width: w,
                            height: h,
                            pixels: color_img.pixels.clone(),
                        },
                    );
                }
            }
        }
        for tex_id in &delta.free {
            self.textures.remove(tex_id);
        }
    }

    fn rasterize_mesh(
        &self,
        pixmap: &mut Pixmap,
        clip_rect: Rect,
        mesh: &egui::epaint::Mesh,
        ppp: f32,
    ) {
        let fb_w = pixmap.width() as i32;
        let fb_h = pixmap.height() as i32;
        let clip_min_x = (clip_rect.min.x * ppp).floor().max(0.0) as i32;
        let clip_min_y = (clip_rect.min.y * ppp).floor().max(0.0) as i32;
        let clip_max_x = (clip_rect.max.x * ppp).ceil().min(fb_w as f32) as i32;
        let clip_max_y = (clip_rect.max.y * ppp).ceil().min(fb_h as f32) as i32;

        if clip_min_x >= clip_max_x || clip_min_y >= clip_max_y {
            return;
        }

        let tex = self.textures.get(&mesh.texture_id);
        let data = pixmap.data_mut();

        for tri in mesh.indices.chunks_exact(3) {
            let (Some(v0), Some(v1), Some(v2)) = (
                mesh.vertices.get(tri[0] as usize),
                mesh.vertices.get(tri[1] as usize),
                mesh.vertices.get(tri[2] as usize),
            ) else {
                continue;
            };

            let x0 = v0.pos.x * ppp;
            let y0 = v0.pos.y * ppp;
            let x1 = v1.pos.x * ppp;
            let y1 = v1.pos.y * ppp;
            let x2 = v2.pos.x * ppp;
            let y2 = v2.pos.y * ppp;

            let min_x = x0.min(x1).min(x2).floor().max(clip_min_x as f32) as i32;
            let max_x = x0.max(x1).max(x2).ceil().min(clip_max_x as f32) as i32;
            let min_y = y0.min(y1).min(y2).floor().max(clip_min_y as f32) as i32;
            let max_y = y0.max(y1).max(y2).ceil().min(clip_max_y as f32) as i32;

            if min_x >= max_x || min_y >= max_y {
                continue;
            }

            let denom = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
            if denom.abs() < 1e-5 {
                continue;
            }
            let inv_denom = 1.0 / denom;

            let c0 = v0.color.to_array();
            let c1 = v1.color.to_array();
            let c2 = v2.color.to_array();

            for py in min_y..max_y {
                let sample_y = py as f32 + 0.5;
                for px in min_x..max_x {
                    let sample_x = px as f32 + 0.5;

                    let w0 =
                        ((y1 - y2) * (sample_x - x2) + (x2 - x1) * (sample_y - y2)) * inv_denom;
                    let w1 =
                        ((y2 - y0) * (sample_x - x2) + (x0 - x2) * (sample_y - y2)) * inv_denom;
                    let w2 = 1.0 - w0 - w1;

                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }

                    let vr = (f32::from(c0[0]) * w0 + f32::from(c1[0]) * w1 + f32::from(c2[0]) * w2)
                        .clamp(0.0, 255.0) as u32;
                    let vg = (f32::from(c0[1]) * w0 + f32::from(c1[1]) * w1 + f32::from(c2[1]) * w2)
                        .clamp(0.0, 255.0) as u32;
                    let vb = (f32::from(c0[2]) * w0 + f32::from(c1[2]) * w1 + f32::from(c2[2]) * w2)
                        .clamp(0.0, 255.0) as u32;
                    let va = (f32::from(c0[3]) * w0 + f32::from(c1[3]) * w1 + f32::from(c2[3]) * w2)
                        .clamp(0.0, 255.0) as u32;

                    let (sr, sg, sb, sa) = if let Some(t) = tex {
                        let u = v0.uv.x * w0 + v1.uv.x * w1 + v2.uv.x * w2;
                        let v = v0.uv.y * w0 + v1.uv.y * w1 + v2.uv.y * w2;
                        let tc = t.sample(u, v).to_array();
                        (
                            (vr * u32::from(tc[0]) + 127) / 255,
                            (vg * u32::from(tc[1]) + 127) / 255,
                            (vb * u32::from(tc[2]) + 127) / 255,
                            (va * u32::from(tc[3]) + 127) / 255,
                        )
                    } else {
                        (vr, vg, vb, va)
                    };

                    if sa == 0 {
                        continue;
                    }

                    let idx = ((py * fb_w + px) as usize) * 4;
                    if sa >= 255 {
                        data[idx] = sr.min(255) as u8;
                        data[idx + 1] = sg.min(255) as u8;
                        data[idx + 2] = sb.min(255) as u8;
                        data[idx + 3] = 255;
                    } else {
                        let inv_a = 255 - sa;
                        let dr = u32::from(data[idx]);
                        let dg = u32::from(data[idx + 1]);
                        let db = u32::from(data[idx + 2]);
                        let da = u32::from(data[idx + 3]);

                        data[idx] = (sr + (dr * inv_a + 127) / 255).min(255) as u8;
                        data[idx + 1] = (sg + (dg * inv_a + 127) / 255).min(255) as u8;
                        data[idx + 2] = (sb + (db * inv_a + 127) / 255).min(255) as u8;
                        data[idx + 3] = (sa + (da * inv_a + 127) / 255).min(255) as u8;
                    }
                }
            }
        }
    }
}
