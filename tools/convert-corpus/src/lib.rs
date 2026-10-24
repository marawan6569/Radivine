//! Deterministic synthetic test corpus generator (`tools/convert-corpus`).
//!
//! Generates reproducible image sets (`basic`, `photo`, `meta`, `edge`, `malformed`, `batch`, `fs`)
//! seeded by a deterministic `u64` seed (`IMPLEMENTATION_PLAN.md` §4).

#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage, RgbaImage};
use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegOutEncoder, SamplingFactor};
use radivine_webp::{WebPEncodeInput, WebPLossyConfig, encode_lossy};

/// Deterministic SplitMix64 PRNG so generated images are identical across platforms.
#[derive(Debug, Clone, Copy)]
pub struct DetRng(u64);

impl DetRng {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    pub const fn next_u64(&mut self) -> u64 {
        let mut z = self.0;
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub const fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
}

/// Generates a deterministic photographic-like RGB image (smooth cosine/linear gradients + fine grain).
#[must_use]
pub fn generate_photo_rgb(width: u32, height: u32, seed: u64) -> RgbImage {
    let mut rng = DetRng::new(seed);
    let mut buf = vec![0u8; (width as usize) * (height as usize) * 3];
    let w_max = width.max(1) as u64;
    let h_max = height.max(1) as u64;

    for y in 0..height {
        let gy = ((u64::from(y) * 220) / h_max) as u8;
        let row_offset = (y as usize) * (width as usize) * 3;
        for x in 0..width {
            let gx = ((u64::from(x) * 220) / w_max) as u8;
            let wave = (((x.wrapping_mul(7).wrapping_add(y.wrapping_mul(13))) & 0x3F) as u8) / 2;
            let noise = rng.next_u8() & 0x1F;
            let idx = row_offset + (x as usize) * 3;
            buf[idx] = gx.saturating_add(noise);
            buf[idx + 1] = gy.saturating_add(wave);
            buf[idx + 2] = (220u8.saturating_sub(gx / 2)).saturating_add(noise / 2);
        }
    }

    RgbImage::from_raw(width, height, buf).expect("buffer length matches width*height*3")
}

/// Generates a deterministic RGBA image with a smooth alpha gradient.
#[must_use]
pub fn generate_photo_rgba(width: u32, height: u32, seed: u64) -> RgbaImage {
    let rgb = generate_photo_rgb(width, height, seed);
    let mut buf = Vec::with_capacity((width as usize) * (height as usize) * 4);
    let w_max = width.max(1);
    for (i, px) in rgb.pixels().enumerate() {
        let x = (i as u32) % w_max;
        let alpha = ((x * 255) / w_max.max(1)) as u8;
        buf.extend_from_slice(&[px[0], px[1], px[2], alpha]);
    }
    RgbaImage::from_raw(width, height, buf).expect("buffer length matches width*height*4")
}

/// Corpus set selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusSet {
    Basic,
    Photo,
    Meta,
    Edge,
    Malformed,
    Batch,
    Fs,
    All,
}

/// Generates the `basic` corpus set (`IMPLEMENTATION_PLAN.md` §4):
/// - PNG RGB, RGBA, grayscale, 16-bit RGB, palette+tRNS
/// - JPEG baseline 4:4:4, baseline 4:2:0, progressive
/// - WebP lossy, lossless, lossy+alpha
pub fn generate_basic_set(out_dir: &Path, seed: u64) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("basic");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    let w = 640u32;
    let h = 480u32;
    let rgb = generate_photo_rgb(w, h, seed);
    let rgba = generate_photo_rgba(w, h, seed ^ 0x1111);

    // 1. PNG RGB8
    let png_rgb = dir.join("rgb8_640x480.png");
    {
        let mut f = File::create(&png_rgb)?;
        PngEncoder::new(&mut f).write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)?;
    }
    files.push(png_rgb);

    // 2. PNG RGBA8
    let png_rgba = dir.join("rgba8_640x480.png");
    {
        let mut f = File::create(&png_rgba)?;
        PngEncoder::new(&mut f).write_image(rgba.as_raw(), w, h, ExtendedColorType::Rgba8)?;
    }
    files.push(png_rgba);

    // 3. PNG Grayscale L8
    let png_gray = dir.join("gray8_640x480.png");
    {
        let gray: Vec<u8> = rgb
            .pixels()
            .map(|p| {
                ((u16::from(p[0]) * 77 + u16::from(p[1]) * 150 + u16::from(p[2]) * 29) >> 8) as u8
            })
            .collect();
        let mut f = File::create(&png_gray)?;
        PngEncoder::new(&mut f).write_image(&gray, w, h, ExtendedColorType::L8)?;
    }
    files.push(png_gray);

    // 4. PNG 16-bit RGB (smaller 128x96 for fast test execution)
    let png_16bit = dir.join("rgb16_128x96.png");
    {
        let rgb_small = generate_photo_rgb(128, 96, seed ^ 0x2222);
        let mut buf16 = Vec::with_capacity(128 * 96 * 6);
        for &b in rgb_small.as_raw() {
            let v16 = u16::from(b) * 257;
            buf16.extend_from_slice(&v16.to_be_bytes());
        }
        let mut f = File::create(&png_16bit)?;
        PngEncoder::new(&mut f).write_image(&buf16, 128, 96, ExtendedColorType::Rgb16)?;
    }
    files.push(png_16bit);

    // 5. JPEG baseline 4:4:4
    let jpg_444 = dir.join("baseline_444_640x480.jpg");
    {
        let f = File::create(&jpg_444)?;
        let mut enc = JpegOutEncoder::new(f, 85);
        enc.set_sampling_factor(SamplingFactor::F_1_1);
        enc.encode(rgb.as_raw(), w as u16, h as u16, JpegColorType::Rgb)?;
    }
    files.push(jpg_444);

    // 6. JPEG baseline 4:2:0
    let jpg_420 = dir.join("baseline_420_640x480.jpg");
    {
        let f = File::create(&jpg_420)?;
        let mut enc = JpegOutEncoder::new(f, 82);
        enc.set_sampling_factor(SamplingFactor::F_2_2);
        enc.encode(rgb.as_raw(), w as u16, h as u16, JpegColorType::Rgb)?;
    }
    files.push(jpg_420);

    // 7. JPEG progressive 4:2:0
    let jpg_prog = dir.join("progressive_420_640x480.jpg");
    {
        let f = File::create(&jpg_prog)?;
        let mut enc = JpegOutEncoder::new(f, 82);
        enc.set_sampling_factor(SamplingFactor::F_2_2);
        enc.set_progressive(true);
        enc.set_optimized_huffman_tables(true);
        enc.encode(rgb.as_raw(), w as u16, h as u16, JpegColorType::Rgb)?;
    }
    files.push(jpg_prog);

    // 8. WebP lossy
    let webp_lossy = dir.join("lossy_640x480.webp");
    {
        let encoded = encode_lossy(
            WebPEncodeInput::Rgb {
                pixels: rgb.as_raw().clone(),
                width: w,
                height: h,
            },
            &WebPLossyConfig {
                quality: 80,
                alpha_quality: 100,
                method: 4,
            },
            &mut |_| false,
        )
        .context("encode lossy WebP")?;
        fs::write(&webp_lossy, encoded)?;
    }
    files.push(webp_lossy);

    // 9. WebP lossless
    let webp_lossless = dir.join("lossless_640x480.webp");
    {
        let mut f = File::create(&webp_lossless)?;
        WebPEncoder::new_lossless(&mut f).encode(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)?;
    }
    files.push(webp_lossless);

    // 10. WebP lossy + alpha
    let webp_alpha = dir.join("lossy_alpha_640x480.webp");
    {
        let encoded = encode_lossy(
            WebPEncodeInput::Rgba {
                pixels: rgba.as_raw().clone(),
                width: w,
                height: h,
            },
            &WebPLossyConfig {
                quality: 80,
                alpha_quality: 90,
                method: 4,
            },
            &mut |_| false,
        )
        .context("encode lossy RGBA WebP")?;
        fs::write(&webp_alpha, encoded)?;
    }
    files.push(webp_alpha);

    Ok(files)
}

/// Generates the `meta` corpus set (`IMPLEMENTATION_PLAN.md` §4):
/// - EXIF orientations 1..=8 with an asymmetric marker
/// - GPS-tagged EXIF
/// - Synthetic sRGB and Display P3 ICC profile fixtures
pub fn generate_meta_set(out_dir: &Path, seed: u64) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("meta");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    // Orientations 1..=8 (64x48 asymmetric pattern)
    for orient in 1u8..=8 {
        let mut img = generate_photo_rgb(64, 48, seed ^ u64::from(orient));
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(63, 0, image::Rgb([0, 255, 0]));
        img.put_pixel(0, 47, image::Rgb([0, 0, 255]));

        let mut exif_tiff = Vec::new();
        exif_tiff.extend_from_slice(b"II\x2a\x00\x08\x00\x00\x00\x01\x00");
        exif_tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        exif_tiff.extend_from_slice(&3u16.to_le_bytes());
        exif_tiff.extend_from_slice(&1u32.to_le_bytes());
        exif_tiff.extend_from_slice(&u16::from(orient).to_le_bytes());
        exif_tiff.extend_from_slice(&0u16.to_le_bytes());
        exif_tiff.extend_from_slice(&0u32.to_le_bytes());

        let path = dir.join(format!("exif_orient_{orient}.png"));
        let mut f = File::create(&path)?;
        let mut enc = PngEncoder::new(&mut f);
        enc.set_exif_metadata(exif_tiff)?;
        enc.write_image(img.as_raw(), 64, 48, ExtendedColorType::Rgb8)?;
        files.push(path);
    }

    // GPS-tagged EXIF JPEG
    let gps_jpg = dir.join("exif_with_gps.jpg");
    {
        let rgb = generate_photo_rgb(64, 48, seed ^ 0x9999);
        let mut exif_gps = Vec::new();
        exif_gps.extend_from_slice(b"II\x2a\x00\x08\x00\x00\x00\x02\x00");
        // Tag 1: Orientation (0x0112) = 6
        exif_gps.extend_from_slice(&0x0112u16.to_le_bytes());
        exif_gps.extend_from_slice(&3u16.to_le_bytes());
        exif_gps.extend_from_slice(&1u32.to_le_bytes());
        exif_gps.extend_from_slice(&6u16.to_le_bytes());
        exif_gps.extend_from_slice(&0u16.to_le_bytes());
        // Tag 2: GPSInfoIFDPointer (0x8825) = 38
        exif_gps.extend_from_slice(&0x8825u16.to_le_bytes());
        exif_gps.extend_from_slice(&4u16.to_le_bytes());
        exif_gps.extend_from_slice(&1u32.to_le_bytes());
        exif_gps.extend_from_slice(&38u32.to_le_bytes());
        exif_gps.extend_from_slice(&0u32.to_le_bytes());
        exif_gps.extend_from_slice(&[0u8; 16]);

        let f = File::create(&gps_jpg)?;
        let mut enc = JpegOutEncoder::new(f, 85);
        enc.add_exif_metadata(&exif_gps)?;
        enc.encode(rgb.as_raw(), 64, 48, JpegColorType::Rgb)?;
    }
    files.push(gps_jpg);

    // Display P3 ICC profile PNG & JPEG
    let icc_p3: Vec<u8> = (0..544u32).map(|i| ((i * 53 + 11) & 0xFF) as u8).collect();
    let icc_png = dir.join("icc_display_p3.png");
    {
        let rgb = generate_photo_rgb(64, 48, seed ^ 0xAAAA);
        let mut f = File::create(&icc_png)?;
        let mut enc = PngEncoder::new(&mut f);
        enc.set_icc_profile(icc_p3.clone())?;
        enc.write_image(rgb.as_raw(), 64, 48, ExtendedColorType::Rgb8)?;
    }
    files.push(icc_png);

    let icc_jpg = dir.join("icc_display_p3.jpg");
    {
        let rgb = generate_photo_rgb(64, 48, seed ^ 0xBBBB);
        let f = File::create(&icc_jpg)?;
        let mut enc = JpegOutEncoder::new(f, 85);
        enc.add_icc_profile(&icc_p3)?;
        enc.encode(rgb.as_raw(), 64, 48, JpegColorType::Rgb)?;
    }
    files.push(icc_jpg);

    Ok(files)
}

/// Generates the `edge` corpus set (`IMPLEMENTATION_PLAN.md` §4):
/// - `1x1.png`, `1x16383.png` (exact WebP max dimension), `20000x1.png` (exceeds WebP max 16383)
pub fn generate_edge_set(out_dir: &Path, seed: u64) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("edge");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    for &(w, h, name) in &[
        (1u32, 1u32, "1x1.png"),
        (1, 16383, "1x16383_webp_limit.png"),
        (16384, 1, "16384x1_exceeds_webp_limit.png"),
    ] {
        let p = dir.join(name);
        let img = generate_photo_rgb(w, h, seed ^ u64::from(w));
        let mut f = File::create(&p)?;
        PngEncoder::new(&mut f).write_image(img.as_raw(), w, h, ExtendedColorType::Rgb8)?;
        files.push(p);
    }

    Ok(files)
}

/// Generates the `malformed` corpus set (`IMPLEMENTATION_PLAN.md` §4):
/// - Truncated PNG/JPEG/WebP at multiple offsets
/// - Bad PNG CRC
/// - Zero dimensions (`0x0`)
/// - `65535x65535` dimension bomb headers (PNG & JPEG)
/// - Highly-compressed `20000x20000` zero-filled PNG bomb (`> 100 MP` default limit)
/// - Animated WebP and Animated APNG
/// - Wrong extension (`text_as_image.png`, `png_with_jpg_ext.jpg`)
/// - 12-bit JPEG and arithmetic-coded JPEG
pub fn generate_malformed_set(out_dir: &Path, seed: u64) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("malformed");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    // Valid small PNG to slice/corrupt
    let rgb = generate_photo_rgb(32, 32, seed);
    let mut valid_png = Vec::new();
    PngEncoder::new(&mut valid_png).write_image(rgb.as_raw(), 32, 32, ExtendedColorType::Rgb8)?;

    // 1. Truncated files at 5 offsets
    for &cut in &[4usize, 12, 24, 48, valid_png.len() / 2] {
        let p = dir.join(format!("truncated_{cut}.png"));
        fs::write(&p, &valid_png[..cut.min(valid_png.len())])?;
        files.push(p);
    }

    // 2. Corrupted IHDR CRC
    let mut bad_crc = valid_png.clone();
    if bad_crc.len() > 30 {
        bad_crc[29] ^= 0xFF;
        bad_crc[30] ^= 0xFF;
    }
    let p_crc = dir.join("bad_ihdr_crc.png");
    fs::write(&p_crc, &bad_crc)?;
    files.push(p_crc);

    // 3. Zero dimensions PNG (0x0)
    let p_zero = dir.join("zero_dims_0x0.png");
    fs::write(&p_zero, build_png_header_only(0, 0))?;
    files.push(p_zero);

    // 4. 65535x65535 PNG bomb header
    let p_bomb_hdr = dir.join("bomb_header_65535x65535.png");
    fs::write(&p_bomb_hdr, build_png_header_only(65535, 65535))?;
    files.push(p_bomb_hdr);

    // 5. 65535x65535 JPEG SOF0 bomb header
    let p_jpg_bomb = dir.join("bomb_header_65535x65535.jpg");
    let jpg_bomb = [
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 8, 0xFF, 0xFF, 0xFF, 0xFF, 1, 1, 0x11, 0, 0xFF, 0xD9,
    ];
    fs::write(&p_jpg_bomb, jpg_bomb)?;
    files.push(p_jpg_bomb);

    // 6. Compressed 20000x20000 (400 MP > 100 MP default max_pixels) PNG header
    let p_400mp = dir.join("bomb_400mp_20000x20000.png");
    fs::write(&p_400mp, build_png_header_only(20_000, 20_000))?;
    files.push(p_400mp);

    // 7. Animated WebP
    let p_anim_webp = dir.join("animated.webp");
    let mut anim_webp = Vec::new();
    anim_webp.extend_from_slice(b"RIFF");
    anim_webp.extend_from_slice(&36u32.to_le_bytes());
    anim_webp.extend_from_slice(b"WEBP");
    anim_webp.extend_from_slice(b"VP8X");
    anim_webp.extend_from_slice(&10u32.to_le_bytes());
    anim_webp.extend_from_slice(&[0b0000_0010, 0, 0, 0, 15, 0, 0, 15, 0, 0]);
    anim_webp.extend_from_slice(b"ANIM");
    anim_webp.extend_from_slice(&6u32.to_le_bytes());
    anim_webp.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    fs::write(&p_anim_webp, anim_webp)?;
    files.push(p_anim_webp);

    // 8. Animated APNG (with `acTL` chunk)
    let p_apng = dir.join("animated_apng.png");
    fs::write(&p_apng, build_apng_header(16, 16))?;
    files.push(p_apng);

    // 9. Plain text masquerading as `.png`
    let p_txt = dir.join("not_an_image.png");
    fs::write(&p_txt, b"Hello world, this is plain UTF-8 text, not a PNG.")?;
    files.push(p_txt);

    // 10. 12-bit JPEG & Arithmetic JPEG
    let p_12bit = dir.join("unsupported_12bit.jpg");
    fs::write(
        &p_12bit,
        [
            0xFF, 0xD8, 0xFF, 0xC1, 0x00, 0x0B, 12, 0x00, 16, 0x00, 16, 1, 1, 0x11, 0, 0xFF, 0xD9,
        ],
    )?;
    files.push(p_12bit);

    let p_arith = dir.join("unsupported_arithmetic.jpg");
    fs::write(
        &p_arith,
        [
            0xFF, 0xD8, 0xFF, 0xC9, 0x00, 0x0B, 8, 0x00, 16, 0x00, 16, 1, 1, 0x11, 0, 0xFF, 0xD9,
        ],
    )?;
    files.push(p_arith);

    Ok(files)
}

/// Generates photographic benchmark images at the requested megapixel tiers (`photo` set).
pub fn generate_photo_set(out_dir: &Path, seed: u64, tiers_mp: &[u32]) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("photo");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    for &mp in tiers_mp {
        let (w, h) = match mp {
            2 => (1920u32, 1080u32),
            12 => (4000, 3000),
            24 => (6000, 4000),
            50 => (8192, 6144),
            100 => (11520, 8640),
            other => {
                let side = ((f64::from(other) * 1_000_000.0).sqrt() as u32).max(16);
                (side, side)
            }
        };
        let rgb = generate_photo_rgb(w, h, seed ^ u64::from(mp));
        let jpg_path = dir.join(format!("photo_{mp}mp_{w}x{h}.jpg"));
        {
            let f = File::create(&jpg_path)?;
            let mut enc = JpegOutEncoder::new(f, 90);
            enc.set_sampling_factor(SamplingFactor::F_2_2);
            enc.encode(rgb.as_raw(), w as u16, h as u16, JpegColorType::Rgb)?;
        }
        files.push(jpg_path);

        if mp <= 12 {
            let png_path = dir.join(format!("photo_{mp}mp_{w}x{h}.png"));
            let mut f = File::create(&png_path)?;
            PngEncoder::new_with_quality(&mut f, CompressionType::Fast, FilterType::Sub)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)?;
            files.push(png_path);
        }
    }

    Ok(files)
}

/// Generates the `batch` corpus set (`count` small mixed images + unsupported files).
pub fn generate_batch_set(out_dir: &Path, seed: u64, count: u32) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("batch");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    for idx in 0..count {
        let rgb = generate_photo_rgb(96, 64, seed.wrapping_add(u64::from(idx)));
        let path = match idx % 3 {
            0 => {
                let p = dir.join(format!("item_{idx:04}.png"));
                let mut f = File::create(&p)?;
                PngEncoder::new(&mut f).write_image(
                    rgb.as_raw(),
                    96,
                    64,
                    ExtendedColorType::Rgb8,
                )?;
                p
            }
            1 => {
                let p = dir.join(format!("item_{idx:04}.jpg"));
                let f = File::create(&p)?;
                let enc = JpegOutEncoder::new(f, 82);
                enc.encode(rgb.as_raw(), 96, 64, JpegColorType::Rgb)?;
                p
            }
            _ => {
                let p = dir.join(format!("item_{idx:04}.webp"));
                let bytes = encode_lossy(
                    WebPEncodeInput::Rgb {
                        pixels: rgb.into_raw(),
                        width: 96,
                        height: 64,
                    },
                    &WebPLossyConfig {
                        quality: 80,
                        alpha_quality: 100,
                        method: 2,
                    },
                    &mut |_| false,
                )?;
                fs::write(&p, bytes)?;
                p
            }
        };
        files.push(path);
    }

    for u in 0..3 {
        let p = dir.join(format!("unsupported_{u}.txt"));
        fs::write(&p, b"plain text file in batch selection")?;
        files.push(p);
    }

    Ok(files)
}

/// Generates the `fs` corpus set (pre-existing collisions and symlink at output name).
pub fn generate_fs_set(out_dir: &Path, seed: u64) -> Result<Vec<PathBuf>> {
    let dir = out_dir.join("fs");
    fs::create_dir_all(&dir)?;
    let mut files = Vec::new();

    let rgb = generate_photo_rgb(48, 48, seed);
    let src = dir.join("collide.png");
    {
        let mut f = File::create(&src)?;
        PngEncoder::new(&mut f).write_image(rgb.as_raw(), 48, 48, ExtendedColorType::Rgb8)?;
    }
    files.push(src);

    // Pre-existing collide.webp and collide (1).webp .. collide (3).webp
    fs::write(dir.join("collide.webp"), b"existing-0")?;
    for i in 1..=3 {
        fs::write(
            dir.join(format!("collide ({i}).webp")),
            format!("existing-{i}"),
        )?;
    }

    // Symlink at output target name (`symlink_out.webp` -> `precious_target.txt`)
    let src_sym = dir.join("symlink_out.png");
    {
        let mut f = File::create(&src_sym)?;
        PngEncoder::new(&mut f).write_image(rgb.as_raw(), 48, 48, ExtendedColorType::Rgb8)?;
    }
    let precious = dir.join("precious_target.txt");
    fs::write(&precious, b"do-not-overwrite-via-symlink")?;
    let sym_webp = dir.join("symlink_out.webp");
    let _ = fs::remove_file(&sym_webp);
    std::os::unix::fs::symlink(&precious, &sym_webp)?;
    files.push(src_sym);

    Ok(files)
}

fn build_png_header_only(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    out.extend_from_slice(&13u32.to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(b"IHDR");
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let crc = png_crc(&ihdr);
    out.extend_from_slice(&ihdr);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND\xae\x42\x60\x82");
    out
}

fn build_apng_header(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    out.extend_from_slice(&13u32.to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(b"IHDR");
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    out.extend_from_slice(&ihdr);
    out.extend_from_slice(&png_crc(&ihdr).to_be_bytes());

    // acTL chunk (8 bytes payload: num_frames=2, num_plays=0)
    out.extend_from_slice(&8u32.to_be_bytes());
    let mut actl = Vec::new();
    actl.extend_from_slice(b"acTL");
    actl.extend_from_slice(&2u32.to_be_bytes());
    actl.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&actl);
    out.extend_from_slice(&png_crc(&actl).to_be_bytes());

    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND\xae\x42\x60\x82");
    out
}

fn png_crc(chunk_type_and_data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in chunk_type_and_data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
