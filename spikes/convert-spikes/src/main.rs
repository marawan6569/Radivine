use std::env;
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Cursor, Write};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use image::codecs::jpeg::{JpegDecoder, JpegEncoder};
use image::codecs::png::{PngDecoder, PngEncoder};
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::metadata::Orientation;
use image::{
    ColorType, DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, Limits,
    RgbImage, RgbaImage,
};
use libwebp_sys::*;
use rustix::fs::{AtFlags, RenameFlags, linkat, renameat_with, unlinkat};
use sha2::{Digest, Sha256};

fn read_vm_kb(key: &str) -> u64 {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let num_str = rest.trim().trim_end_matches("kB").trim();
            if let Ok(v) = num_str.parse::<u64>() {
                return v;
            }
        }
    }
    0
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let out = hasher.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

unsafe extern "C" fn vec_writer(
    data: *const u8,
    data_size: usize,
    picture: *const WebPPicture,
) -> std::ffi::c_int {
    unsafe {
        if data.is_null() || picture.is_null() || (*picture).custom_ptr.is_null() {
            return 0;
        }
        let out = &mut *((*picture).custom_ptr as *mut Vec<u8>);
        let slice = std::slice::from_raw_parts(data, data_size);
        out.extend_from_slice(slice);
        1
    }
}

unsafe extern "C" fn abort_at_50_progress(
    percent: std::ffi::c_int,
    picture: *const WebPPicture,
) -> std::ffi::c_int {
    unsafe {
        if !picture.is_null() && !(*picture).user_data.is_null() {
            let max_pct = &mut *((*picture).user_data as *mut i32);
            *max_pct = (*max_pct).max(percent);
        }
    }
    if percent >= 50 { 0 } else { 1 }
}

#[allow(clippy::too_many_arguments)]
fn encode_webp_lossy_owned(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    has_alpha: bool,
    quality: f32,
    method: i32,
    alpha_quality: i32,
    abort_halfway: bool,
) -> Result<Vec<u8>, i32> {
    unsafe {
        let mut config = std::mem::MaybeUninit::<WebPConfig>::uninit();
        if WebPConfigInitInternal(
            config.as_mut_ptr(),
            WebPPreset::WEBP_PRESET_DEFAULT,
            quality,
            WEBP_ENCODER_ABI_VERSION as i32,
        ) == 0
        {
            return Err(-1);
        }
        let mut config = config.assume_init();
        config.lossless = 0;
        config.quality = quality;
        config.method = method;
        config.alpha_quality = alpha_quality;
        if WebPValidateConfig(&config) == 0 {
            return Err(-2);
        }

        let mut picture = std::mem::MaybeUninit::<WebPPicture>::uninit();
        if WebPPictureInitInternal(picture.as_mut_ptr(), WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return Err(-3);
        }
        let mut picture = picture.assume_init();
        // For lossy encoding, use_argb = 0 imports directly into YUV420 (+ optional alpha plane),
        // reducing WebPPicture memory from 4 B/px + 1.5 B/px to 1.5 B/px (or 2.5 B/px with alpha).
        picture.use_argb = 0;
        picture.width = width as i32;
        picture.height = height as i32;

        let imported = if has_alpha {
            WebPPictureImportRGBA(&mut picture, pixels.as_ptr(), (width * 4) as i32)
        } else {
            WebPPictureImportRGB(&mut picture, pixels.as_ptr(), (width * 3) as i32)
        };
        // Free the source RGB/RGBA buffer before running WebPEncode so decoded pixels and
        // WebPEncode working buffers do not overlap in RSS!
        drop(pixels);

        if imported == 0 {
            WebPPictureFree(&mut picture);
            return Err(picture.error_code as i32);
        }

        let mut out = Vec::with_capacity((width * height / 4) as usize);
        picture.writer = Some(vec_writer);
        picture.custom_ptr = &mut out as *mut Vec<u8> as *mut c_void;

        let mut seen_pct: i32 = 0;
        if abort_halfway {
            picture.progress_hook = Some(abort_at_50_progress);
            picture.user_data = &mut seen_pct as *mut i32 as *mut c_void;
        }

        let ok = WebPEncode(&config, &mut picture);
        let err_code = picture.error_code as i32;
        WebPPictureFree(&mut picture);

        if ok == 0 { Err(err_code) } else { Ok(out) }
    }
}

/// Pure-Rust RIFF/VP8X muxer for lossy WebP output.
fn mux_webp_metadata(
    webp_bytes: &[u8],
    width: u32,
    height: u32,
    icc: Option<&[u8]>,
    exif: Option<&[u8]>,
) -> Result<Vec<u8>, &'static str> {
    let icc = icc.filter(|b| !b.is_empty());
    let exif = exif.filter(|b| !b.is_empty());
    if icc.is_none() && exif.is_none() {
        return Ok(webp_bytes.to_vec());
    }
    if webp_bytes.len() < 12 || &webp_bytes[0..4] != b"RIFF" || &webp_bytes[8..12] != b"WEBP" {
        return Err("invalid RIFF WEBP header");
    }
    if width == 0 || height == 0 || width > 16383 || height > 16383 {
        return Err("invalid WebP canvas dimensions");
    }

    let mut has_alpha = false;
    let mut alph_chunk: Option<&[u8]> = None;
    let mut image_chunk: Option<([u8; 4], &[u8])> = None;

    let mut pos = 12usize;
    while pos + 8 <= webp_bytes.len() {
        let fourcc: [u8; 4] = webp_bytes[pos..pos + 4].try_into().unwrap();
        let len = u32::from_le_bytes(webp_bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let data_start = pos + 8;
        let data_end = data_start.checked_add(len).ok_or("chunk overflow")?;
        if data_end > webp_bytes.len() {
            return Err("truncated RIFF chunk");
        }
        let payload = &webp_bytes[data_start..data_end];
        match &fourcc {
            b"VP8X" if payload.len() >= 10 && (payload[0] & (1 << 4)) != 0 => {
                has_alpha = true;
            }
            b"ALPH" => {
                has_alpha = true;
                alph_chunk = Some(payload);
            }
            b"VP8 " | b"VP8L" => {
                image_chunk = Some((fourcc, payload));
            }
            _ => {}
        }
        let padded = len + (len & 1);
        pos = data_start.saturating_add(padded);
    }

    let (img_fourcc, img_payload) = image_chunk.ok_or("missing VP8/VP8L bitstream chunk")?;

    let write_chunk = |buf: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]| {
        buf.extend_from_slice(fourcc);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            buf.push(0);
        }
    };

    let mut flags: u8 = 0;
    if exif.is_some() {
        flags |= 1 << 3;
    }
    if has_alpha {
        flags |= 1 << 4;
    }
    if icc.is_some() {
        flags |= 1 << 5;
    }

    let mut body = Vec::with_capacity(webp_bytes.len() + 256);
    body.extend_from_slice(b"WEBP");

    let mut vp8x = [0u8; 10];
    vp8x[0] = flags;
    let w_minus_1 = (width - 1).to_le_bytes();
    let h_minus_1 = (height - 1).to_le_bytes();
    vp8x[4..7].copy_from_slice(&w_minus_1[0..3]);
    vp8x[7..10].copy_from_slice(&h_minus_1[0..3]);
    write_chunk(&mut body, b"VP8X", &vp8x);

    if let Some(icc_bytes) = icc {
        write_chunk(&mut body, b"ICCP", icc_bytes);
    }
    if let Some(alph_bytes) = alph_chunk {
        write_chunk(&mut body, b"ALPH", alph_bytes);
    }
    write_chunk(&mut body, &img_fourcc, img_payload);
    if let Some(exif_bytes) = exif {
        write_chunk(&mut body, b"EXIF", exif_bytes);
    }

    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn build_test_exif(orientation: u16, big_endian: bool) -> Vec<u8> {
    let mut buf = Vec::new();
    let u16_bytes = |v: u16| -> [u8; 2] {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32_bytes = |v: u32| -> [u8; 4] {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };

    if big_endian {
        buf.extend_from_slice(b"MM\x00\x2a");
    } else {
        buf.extend_from_slice(b"II\x2a\x00");
    }
    buf.extend_from_slice(&u32_bytes(8));
    buf.extend_from_slice(&u16_bytes(2));

    buf.extend_from_slice(&u16_bytes(0x010F));
    buf.extend_from_slice(&u16_bytes(2));
    buf.extend_from_slice(&u32_bytes(9));
    buf.extend_from_slice(&u32_bytes(38));

    buf.extend_from_slice(&u16_bytes(0x0112));
    buf.extend_from_slice(&u16_bytes(3));
    buf.extend_from_slice(&u32_bytes(1));
    buf.extend_from_slice(&u16_bytes(orientation));
    buf.extend_from_slice(&u16_bytes(0));

    buf.extend_from_slice(&u32_bytes(0));
    buf.extend_from_slice(b"Radivine\0");
    buf
}

fn make_synthetic_photo_rgb(width: u32, height: u32) -> RgbImage {
    let mut img = RgbImage::new(width, height);
    let raw = img.as_mut();
    let mut state: u32 = 0x9E37_79B9;
    let mut idx = 0usize;
    for y in 0..height {
        let base_r = ((y * 255) / height.max(1)) as u8;
        let base_b = 255 - base_r;
        for x in 0..width {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = (state & 0x1F) as u8;
            let base_g = (((x ^ y) & 0xFF) as u8).wrapping_add(noise);
            raw[idx] = base_r.wrapping_add(noise);
            raw[idx + 1] = base_g;
            raw[idx + 2] = base_b.wrapping_sub(noise >> 1);
            idx += 3;
        }
    }
    img
}

/// Take a valid encoded 8x8 JPEG and patch its SOF marker/precision/dimensions to test header rejection.
fn make_patched_jpeg(sof_marker: u8, precision: u8, width: u16, height: u16) -> Vec<u8> {
    let tiny = make_synthetic_photo_rgb(8, 8);
    let mut jpg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpg, 80)
        .write_image(tiny.as_raw(), 8, 8, ExtendedColorType::Rgb8)
        .unwrap();
    // Locate SOF0 (0xFF 0xC0, length 0x00 0x11)
    for i in 0..jpg.len().saturating_sub(10) {
        if jpg[i] == 0xFF && jpg[i + 1] == 0xC0 && jpg[i + 2] == 0x00 && jpg[i + 3] == 0x11 {
            jpg[i + 1] = sof_marker;
            jpg[i + 4] = precision;
            let hb = height.to_be_bytes();
            let wb = width.to_be_bytes();
            jpg[i + 5] = hb[0];
            jpg[i + 6] = hb[1];
            jpg[i + 7] = wb[0];
            jpg[i + 8] = wb[1];
            break;
        }
    }
    jpg
}

fn run_p1() {
    println!("=== P1: libwebp-sys 0.14.4 Vendored Build & Safe Encode Wrapper ===");
    let ver = unsafe { WebPGetEncoderVersion() };
    let maj = (ver >> 16) & 0xFF;
    let min = (ver >> 8) & 0xFF;
    let rev = ver & 0xFF;
    println!("WebPGetEncoderVersion() = 0x{ver:06x} -> v{maj}.{min}.{rev}");
    assert!(
        (maj, min, rev) >= (1, 3, 2),
        "libwebp version must be >= 1.3.2"
    );

    let img = make_synthetic_photo_rgb(512, 384);
    let t0 = Instant::now();
    let encoded_rgb =
        encode_webp_lossy_owned(img.clone().into_raw(), 512, 384, false, 80.0, 4, 100, false)
            .expect("encode RGB");
    let elapsed_rgb = t0.elapsed();
    let dec = WebPDecoder::new(Cursor::new(&encoded_rgb)).expect("decode lossy RGB WebP");
    assert_eq!(dec.dimensions(), (512, 384));
    assert!(!dec.has_animation());

    let mut rgba = RgbaImage::new(256, 256);
    for (x, y, p) in rgba.enumerate_pixels_mut() {
        *p = image::Rgba([x as u8, y as u8, 128, ((x + y) / 2) as u8]);
    }
    let encoded_rgba = encode_webp_lossy_owned(rgba.into_raw(), 256, 256, true, 80.0, 4, 85, false)
        .expect("encode RGBA");
    let dec_a = WebPDecoder::new(Cursor::new(&encoded_rgba)).expect("decode lossy RGBA WebP");
    assert_eq!(dec_a.dimensions(), (256, 256));
    assert_eq!(dec_a.color_type(), ColorType::Rgba8);

    let err = encode_webp_lossy_owned(img.into_raw(), 512, 384, false, 80.0, 4, 100, true)
        .expect_err("progress_hook abort must fail");
    println!(
        "Lossy RGB 512x384 encoded={} B in {:?}; RGBA 256x256 encoded={} B; progress_hook abort error_code={} (VP8_ENC_ERROR_USER_ABORT=10)",
        encoded_rgb.len(),
        elapsed_rgb,
        encoded_rgba.len(),
        err
    );
    assert_eq!(
        err,
        WebPEncodingError::VP8_ENC_ERROR_USER_ABORT as i32,
        "must return VP8_ENC_ERROR_USER_ABORT"
    );
}

fn run_p2_and_p5() {
    println!("\n=== P2 & P5: RIFF/VP8X Muxing, ICC & EXIF Preservation, Orientation 1..=8 ===");
    let dummy_icc: Vec<u8> = (0..544u32)
        .map(|i| ((i.wrapping_mul(37).wrapping_add(11)) & 0xFF) as u8)
        .collect();
    let icc_sha = sha256_hex(&dummy_icc);

    for orient_val in 1u16..=8 {
        for &big_endian in &[false, true] {
            let mut exif = build_test_exif(orient_val, big_endian);
            let parsed = Orientation::from_exif_chunk(&exif).expect("parse orientation");
            assert_eq!(parsed.to_exif(), orient_val as u8);
            let removed =
                Orientation::remove_from_exif_chunk(&mut exif).expect("reset orientation");
            assert_eq!(removed.to_exif(), orient_val as u8);
            let after = Orientation::from_exif_chunk(&exif).expect("re-parse orientation");
            assert_eq!(after, Orientation::NoTransforms);
            assert!(exif.ends_with(b"Radivine\0"), "other EXIF tags preserved");
        }
    }
    println!("EXIF Orientation 1..=8 parse + reset to 1 (LE & BE) preserving Make tag: PASS");

    for orient_val in 1u8..=8 {
        let mut src_img = RgbImage::new(60, 40);
        src_img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        src_img.put_pixel(59, 0, image::Rgb([0, 255, 0]));
        src_img.put_pixel(0, 39, image::Rgb([0, 0, 255]));
        let mut dyn_img = DynamicImage::ImageRgb8(src_img);
        let orient = Orientation::from_exif(orient_val).unwrap();
        dyn_img.apply_orientation(orient);
        let expected_dims = match orient_val {
            1..=4 => (60, 40),
            5..=8 => (40, 60),
            _ => unreachable!(),
        };
        assert_eq!(
            (dyn_img.width(), dyn_img.height()),
            expected_dims,
            "orientation {orient_val} dimensions"
        );
    }
    println!("Asymmetric 60x40 pixel orientation transform for 1..=8: PASS");

    let rgb = make_synthetic_photo_rgb(128, 96);
    let exif_le = build_test_exif(1, false);
    let exif_sha = sha256_hex(&exif_le);

    let mut png_bytes = Vec::new();
    {
        let mut enc = PngEncoder::new(&mut png_bytes);
        enc.set_icc_profile(dummy_icc.clone()).unwrap();
        enc.set_exif_metadata(exif_le.clone()).unwrap();
        enc.write_image(rgb.as_raw(), 128, 96, ExtendedColorType::Rgb8)
            .unwrap();
    }
    {
        let mut dec = PngDecoder::new(Cursor::new(&png_bytes)).unwrap();
        assert_eq!(sha256_hex(&dec.icc_profile().unwrap().unwrap()), icc_sha);
        assert_eq!(sha256_hex(&dec.exif_metadata().unwrap().unwrap()), exif_sha);
    }

    let mut jpg_bytes = Vec::new();
    {
        let mut enc = JpegEncoder::new_with_quality(&mut jpg_bytes, 85);
        enc.set_icc_profile(dummy_icc.clone()).unwrap();
        enc.set_exif_metadata(exif_le.clone()).unwrap();
        enc.write_image(rgb.as_raw(), 128, 96, ExtendedColorType::Rgb8)
            .unwrap();
    }
    {
        let mut dec = JpegDecoder::new(Cursor::new(&jpg_bytes)).unwrap();
        assert_eq!(sha256_hex(&dec.icc_profile().unwrap().unwrap()), icc_sha);
        assert_eq!(sha256_hex(&dec.exif_metadata().unwrap().unwrap()), exif_sha);
    }

    let mut webp_ll_bytes = Vec::new();
    {
        let mut enc = WebPEncoder::new_lossless(&mut webp_ll_bytes);
        enc.set_icc_profile(dummy_icc.clone()).unwrap();
        enc.set_exif_metadata(exif_le.clone()).unwrap();
        enc.write_image(rgb.as_raw(), 128, 96, ExtendedColorType::Rgb8)
            .unwrap();
    }
    {
        let mut dec = WebPDecoder::new(Cursor::new(&webp_ll_bytes)).unwrap();
        assert_eq!(sha256_hex(&dec.icc_profile().unwrap().unwrap()), icc_sha);
        assert_eq!(sha256_hex(&dec.exif_metadata().unwrap().unwrap()), exif_sha);
    }

    let raw_lossy_rgb =
        encode_webp_lossy_owned(rgb.into_raw(), 128, 96, false, 80.0, 4, 100, false).unwrap();
    let muxed_rgb =
        mux_webp_metadata(&raw_lossy_rgb, 128, 96, Some(&dummy_icc), Some(&exif_le)).unwrap();
    {
        let mut dec = WebPDecoder::new(Cursor::new(&muxed_rgb)).unwrap();
        assert_eq!(dec.dimensions(), (128, 96));
        assert_eq!(sha256_hex(&dec.icc_profile().unwrap().unwrap()), icc_sha);
        assert_eq!(sha256_hex(&dec.exif_metadata().unwrap().unwrap()), exif_sha);
        let mut pix = vec![0u8; dec.total_bytes() as usize];
        dec.read_image(&mut pix).unwrap();
    }

    let mut rgba = RgbaImage::new(128, 96);
    for (x, y, p) in rgba.enumerate_pixels_mut() {
        *p = image::Rgba([x as u8, y as u8, 200, if x < 64 { 128 } else { 255 }]);
    }
    let raw_lossy_rgba =
        encode_webp_lossy_owned(rgba.into_raw(), 128, 96, true, 80.0, 4, 90, false).unwrap();
    let muxed_rgba =
        mux_webp_metadata(&raw_lossy_rgba, 128, 96, Some(&dummy_icc), Some(&exif_le)).unwrap();
    {
        let mut dec = WebPDecoder::new(Cursor::new(&muxed_rgba)).unwrap();
        assert_eq!(dec.dimensions(), (128, 96));
        assert_eq!(dec.color_type(), ColorType::Rgba8);
        assert_eq!(sha256_hex(&dec.icc_profile().unwrap().unwrap()), icc_sha);
        assert_eq!(sha256_hex(&dec.exif_metadata().unwrap().unwrap()), exif_sha);
        let mut pix = vec![0u8; dec.total_bytes() as usize];
        dec.read_image(&mut pix).unwrap();
    }
    println!(
        "ICC (544 B, sha256={}..) & EXIF round-trip across PNG, JPEG, WebP-Lossless, WebP-Lossy (RGB & RGBA): PASS",
        &icc_sha[..12]
    );
}

fn run_p3() {
    println!("\n=== P3: image::Limits Pre-Allocation Bomb Rejection (65535x65535) ===");
    let mut limits = Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(268_435_456); // 256 MiB

    // 1. Crafted 65535x65535 PNG header
    let mut png_bomb = Vec::new();
    png_bomb.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    png_bomb.extend_from_slice(&13u32.to_be_bytes());
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(b"IHDR");
    ihdr.extend_from_slice(&65535u32.to_be_bytes());
    ihdr.extend_from_slice(&65535u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in &ihdr {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if (crc & 1) != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    png_bomb.extend_from_slice(&ihdr);
    png_bomb.extend_from_slice(&(!crc).to_be_bytes());
    png_bomb.extend_from_slice(&0u32.to_be_bytes());
    png_bomb.extend_from_slice(b"IEND\xae\x42\x60\x82");

    let rss_before = read_vm_kb("VmRSS:");
    let t0 = Instant::now();
    let mut reader = ImageReader::with_format(Cursor::new(&png_bomb), image::ImageFormat::Png);
    reader.limits(limits.clone());
    let png_res = reader.decode();
    let png_elapsed = t0.elapsed();
    let rss_after_png = read_vm_kb("VmRSS:");
    assert!(png_res.is_err(), "65535x65535 PNG must be rejected");

    // 2. Complete valid JPEG with SOF0 patched to 65535x65535
    let jpg_bomb = make_patched_jpeg(0xC0, 8, 65535, 65535);
    let t1 = Instant::now();
    let mut reader = ImageReader::with_format(Cursor::new(&jpg_bomb), image::ImageFormat::Jpeg);
    reader.limits(limits.clone());
    let jpg_res = reader.decode();
    let jpg_elapsed = t1.elapsed();
    let rss_after_jpg = read_vm_kb("VmRSS:");
    assert!(jpg_res.is_err(), "65535x65535 JPEG must be rejected");

    // 3. Crafted 65535x65535 WebP header (VP8X with canvas 65535x65535)
    let mut webp_bomb = Vec::new();
    webp_bomb.extend_from_slice(b"RIFF");
    webp_bomb.extend_from_slice(&30u32.to_le_bytes());
    webp_bomb.extend_from_slice(b"WEBP");
    webp_bomb.extend_from_slice(b"VP8X");
    webp_bomb.extend_from_slice(&10u32.to_le_bytes());
    webp_bomb.extend_from_slice(&[0, 0, 0, 0]);
    webp_bomb.extend_from_slice(&65534u32.to_le_bytes()[0..3]);
    webp_bomb.extend_from_slice(&65534u32.to_le_bytes()[0..3]);
    webp_bomb.extend_from_slice(b"VP8 ");
    webp_bomb.extend_from_slice(&0u32.to_le_bytes());

    let t2 = Instant::now();
    let mut reader = ImageReader::with_format(Cursor::new(&webp_bomb), image::ImageFormat::WebP);
    reader.limits(limits);
    let webp_res = reader.decode();
    let webp_elapsed = t2.elapsed();
    let rss_after_webp = read_vm_kb("VmRSS:");
    assert!(webp_res.is_err(), "65535x65535 WebP must be rejected");

    println!(
        "PNG 65535x65535 rejected in {:?}: {:?} (RSS delta={} kB)",
        png_elapsed,
        png_res.unwrap_err(),
        rss_after_png.saturating_sub(rss_before)
    );
    println!(
        "JPEG 65535x65535 rejected in {:?}: {:?} (RSS delta={} kB)",
        jpg_elapsed,
        jpg_res.unwrap_err(),
        rss_after_jpg.saturating_sub(rss_after_png)
    );
    println!(
        "WebP 65535x65535 rejected in {:?}: {:?} (RSS delta={} kB)",
        webp_elapsed,
        webp_res.unwrap_err(),
        rss_after_webp.saturating_sub(rss_after_jpg)
    );
}

fn run_p4() {
    println!("\n=== P4: zune-jpeg Behavior on CMYK, YCCK, 12-bit, and Arithmetic JPEG ===");
    let arith_jpg = make_patched_jpeg(0xC9, 8, 8, 8);
    let arith_res = JpegDecoder::new(Cursor::new(&arith_jpg)).and_then(DynamicImage::from_decoder);
    println!("Arithmetic JPEG (SOF9 0xFFC9): {:?}", arith_res.err());

    let bit12_jpg = make_patched_jpeg(0xC1, 12, 8, 8);
    let bit12_res = JpegDecoder::new(Cursor::new(&bit12_jpg)).and_then(DynamicImage::from_decoder);
    println!("12-bit JPEG (SOF1 precision=12): {:?}", bit12_res.err());

    // Build a valid 1x1 4-component CMYK JPEG using jpeg-encoder (ColorType::Cmyk)
    let mut cmyk_jpg = Vec::new();
    jpeg_encoder::Encoder::new(&mut cmyk_jpg, 85)
        .encode(&[0u8, 128, 255, 16], 1, 1, jpeg_encoder::ColorType::Cmyk)
        .unwrap();
    let cmyk_dec = JpegDecoder::new(Cursor::new(&cmyk_jpg)).unwrap();
    let cmyk_color = cmyk_dec.color_type();
    let cmyk_img = DynamicImage::from_decoder(cmyk_dec).unwrap();
    println!(
        "CMYK JPEG (jpeg_encoder::ColorType::Cmyk): decoded cleanly! color_type={:?}, pixel={:?}",
        cmyk_color,
        cmyk_img.to_rgb8().get_pixel(0, 0)
    );

    // Build a valid 1x1 4-component YCCK JPEG using jpeg-encoder (ColorType::Ycck)
    let mut ycck_jpg = Vec::new();
    jpeg_encoder::Encoder::new(&mut ycck_jpg, 85)
        .encode(&[128u8, 128, 128, 32], 1, 1, jpeg_encoder::ColorType::Ycck)
        .unwrap();
    let ycck_dec = JpegDecoder::new(Cursor::new(&ycck_jpg)).unwrap();
    let ycck_color = ycck_dec.color_type();
    let ycck_img = DynamicImage::from_decoder(ycck_dec).unwrap();
    println!(
        "YCCK JPEG (jpeg_encoder::ColorType::Ycck): decoded cleanly! color_type={:?}, pixel={:?}",
        ycck_color,
        ycck_img.to_rgb8().get_pixel(0, 0)
    );
}

fn run_p6() {
    println!(
        "\n=== P6: JPEG Encoder Evaluation (image::codecs::jpeg::JpegEncoder vs jpeg-encoder) ==="
    );
    let img = make_synthetic_photo_rgb(2048, 1536);
    for &q in &[70u8, 82, 92] {
        let t0 = Instant::now();
        let mut out_img = Vec::with_capacity(512 * 1024);
        let enc = JpegEncoder::new_with_quality(&mut out_img, q);
        enc.write_image(img.as_raw(), 2048, 1536, ExtendedColorType::Rgb8)
            .unwrap();
        let dt_img = t0.elapsed();

        let t1 = Instant::now();
        let mut out_je = Vec::with_capacity(512 * 1024);
        let je = jpeg_encoder::Encoder::new(&mut out_je, q);
        je.encode(img.as_raw(), 2048, 1536, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        let dt_je = t1.elapsed();

        println!(
            "q={q}: image::JpegEncoder size={} B time={:?} | jpeg-encoder size={} B time={:?}",
            out_img.len(),
            dt_img,
            out_je.len(),
            dt_je
        );
    }
}

fn run_p7_parent() {
    println!("\n=== P7: Peak RSS & Wall Time Calibration (12 MP, 24 MP, 50 MP, 100 MP) ===");
    let exe = env::current_exe().expect("current_exe");
    let tmp = tempfile::tempdir_in("/dev/shm").unwrap_or_else(|_| tempfile::tempdir().unwrap());
    for &(mp_label, w, h) in &[
        ("12MP", 4000u32, 3000u32),
        ("24MP", 6000u32, 4000u32),
        ("50MP", 8192u32, 6104u32),
        ("100MP", 10000u32, 10000u32),
    ] {
        // Generate source files on disk once per resolution so child processes measure pure conversion RSS
        let rgb = make_synthetic_photo_rgb(w, h);
        let jpg_path = tmp.path().join(format!("{mp_label}.jpg"));
        let png_path = tmp.path().join(format!("{mp_label}.png"));
        {
            let f = BufWriter::new(File::create(&jpg_path).unwrap());
            JpegEncoder::new_with_quality(f, 85)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)
                .unwrap();
        }
        {
            let f = BufWriter::new(File::create(&png_path).unwrap());
            PngEncoder::new(f)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)
                .unwrap();
        }
        drop(rgb);

        for (pair, src_path) in &[("jpg_to_webp", &jpg_path), ("png_to_jpg", &png_path)] {
            let out_path = tmp.path().join(format!("{mp_label}_{pair}.out"));
            let out = Command::new(&exe)
                .args([
                    "--p7-child",
                    mp_label,
                    &w.to_string(),
                    &h.to_string(),
                    pair,
                    src_path.to_str().unwrap(),
                    out_path.to_str().unwrap(),
                ])
                .output()
                .expect("spawn child");
            assert!(
                out.status.success(),
                "child failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            print!("{}", String::from_utf8_lossy(&out.stdout));
            let _ = fs::remove_file(&out_path);
        }
        let _ = fs::remove_file(&jpg_path);
        let _ = fs::remove_file(&png_path);
    }
}

fn run_p7_child(mp_label: &str, w: u32, h: u32, pair: &str, src_path: &str, out_path: &str) {
    let rss_before_kb = read_vm_kb("VmRSS:");
    let t0 = Instant::now();
    let reader = ImageReader::open(src_path)
        .unwrap()
        .with_guessed_format()
        .unwrap();
    let decoded = reader.decode().unwrap();
    let rgb = decoded.into_rgb8();
    match pair {
        "jpg_to_webp" => {
            let out_bytes =
                encode_webp_lossy_owned(rgb.into_raw(), w, h, false, 80.0, 4, 100, false).unwrap();
            fs::write(out_path, &out_bytes).unwrap();
        }
        "png_to_jpg" => {
            let f = BufWriter::new(File::create(out_path).unwrap());
            JpegEncoder::new_with_quality(f, 82)
                .write_image(rgb.as_raw(), w, h, ExtendedColorType::Rgb8)
                .unwrap();
        }
        _ => unreachable!(),
    }
    let elapsed = t0.elapsed();
    let hwm_kb = read_vm_kb("VmHWM:");
    let rss_after_kb = read_vm_kb("VmRSS:");
    let src_kb = fs::metadata(src_path).unwrap().len() / 1024;
    let out_kb = fs::metadata(out_path).unwrap().len() / 1024;
    let decoded_rgba_mib = (w as f64 * h as f64 * 4.0) / (1024.0 * 1024.0);
    let formula_mib = 2.2 * decoded_rgba_mib + 16.0;
    let hwm_mib = hwm_kb as f64 / 1024.0;
    println!(
        "{mp_label} ({w}x{h}) {pair}: wall={elapsed:?}, src={src_kb} KB, out={out_kb} KB, VmRSS_before={rss_before_kb} KB, VmRSS_after={rss_after_kb} KB, VmHWM={hwm_mib:.1} MiB, formula(2.2*RGBA+16)={formula_mib:.1} MiB ({})",
        if hwm_mib <= formula_mib {
            "WITHIN FORMULA"
        } else {
            "ABOVE FORMULA"
        }
    );
}

fn test_atomic_commit_on_dir(base_dir: &Path, label: &str) {
    let stage_dir_path = base_dir.join(".rvstage-spike");
    fs::create_dir_all(&stage_dir_path).unwrap();
    let stage_dir_fd = File::open(&stage_dir_path).unwrap();
    let dest_dir_fd = File::open(base_dir).unwrap();

    let part_a = stage_dir_path.join("item-0-aaaaaaaa.part");
    let dest_a = base_dir.join("out_a.webp");
    let _ = fs::remove_file(&dest_a);
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&part_a)
            .unwrap();
        f.write_all(b"rung-a-payload").unwrap();
        f.sync_all().unwrap();
    }
    renameat_with(
        &stage_dir_fd,
        "item-0-aaaaaaaa.part",
        &dest_dir_fd,
        "out_a.webp",
        RenameFlags::NOREPLACE,
    )
    .expect("Rung A renameat_with NOREPLACE");
    stage_dir_fd.sync_all().unwrap();
    dest_dir_fd.sync_all().unwrap();
    assert_eq!(fs::read(&dest_a).unwrap(), b"rung-a-payload");

    let part_coll = stage_dir_path.join("item-1-bbbbbbbb.part");
    fs::write(&part_coll, b"new-payload-must-not-overwrite").unwrap();
    let coll_err = renameat_with(
        &stage_dir_fd,
        "item-1-bbbbbbbb.part",
        &dest_dir_fd,
        "out_a.webp",
        RenameFlags::NOREPLACE,
    )
    .expect_err("collision must return EEXIST");
    assert_eq!(coll_err, rustix::io::Errno::EXIST);
    assert_eq!(fs::read(&dest_a).unwrap(), b"rung-a-payload");
    fs::remove_file(&part_coll).unwrap();
    fs::remove_file(&dest_a).unwrap();

    let part_b = stage_dir_path.join("item-2-cccccccc.part");
    let dest_b = base_dir.join("out_b.webp");
    let _ = fs::remove_file(&dest_b);
    {
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&part_b)
            .unwrap();
        f.write_all(b"rung-b-payload").unwrap();
        f.sync_all().unwrap();
    }
    linkat(
        &stage_dir_fd,
        "item-2-cccccccc.part",
        &dest_dir_fd,
        "out_b.webp",
        AtFlags::empty(),
    )
    .expect("Rung B linkat");
    dest_dir_fd.sync_all().unwrap();
    unlinkat(&stage_dir_fd, "item-2-cccccccc.part", AtFlags::empty()).expect("Rung B unlinkat");
    stage_dir_fd.sync_all().unwrap();
    assert_eq!(fs::read(&dest_b).unwrap(), b"rung-b-payload");

    let part_b_coll = stage_dir_path.join("item-3-dddddddd.part");
    fs::write(&part_b_coll, b"collision-b").unwrap();
    let link_err = linkat(
        &stage_dir_fd,
        "item-3-dddddddd.part",
        &dest_dir_fd,
        "out_b.webp",
        AtFlags::empty(),
    )
    .expect_err("linkat collision must return EEXIST");
    assert_eq!(link_err, rustix::io::Errno::EXIST);
    assert_eq!(fs::read(&dest_b).unwrap(), b"rung-b-payload");

    fs::remove_file(&part_b_coll).unwrap();
    fs::remove_file(&dest_b).unwrap();
    drop(stage_dir_fd);
    drop(dest_dir_fd);
    fs::remove_dir(&stage_dir_path).unwrap();
    println!(
        "{label}: Rung A (RENAME_NOREPLACE + EEXIST) & Rung B (linkat + unlinkat + EEXIST) + dir fsyncs: PASS"
    );
}

fn run_p8() {
    println!("\n=== P8: Two-Rung Atomic Commit Matrix (ext4, tmpfs, Fault Injection) ===");
    let ext4_tmp = tempfile::tempdir_in(".").expect("tempdir in workspace (ext4)");
    test_atomic_commit_on_dir(ext4_tmp.path(), "ext4 (workspace)");

    if Path::new("/dev/shm").is_dir() {
        let tmpfs_tmp = tempfile::tempdir_in("/dev/shm").expect("tempdir in /dev/shm (tmpfs)");
        test_atomic_commit_on_dir(tmpfs_tmp.path(), "tmpfs (/dev/shm)");
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 8 && args[1] == "--p7-child" {
        let mp_label = &args[2];
        let w: u32 = args[3].parse().unwrap();
        let h: u32 = args[4].parse().unwrap();
        let pair = &args[5];
        let src_path = &args[6];
        let out_path = &args[7];
        run_p7_child(mp_label, w, h, pair, src_path, out_path);
        return;
    }

    let _ = BufReader::new(Cursor::new(&[]));
    run_p1();
    run_p2_and_p5();
    run_p3();
    run_p4();
    run_p6();
    run_p7_parent();
    run_p8();
}
