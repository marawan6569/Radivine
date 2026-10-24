//! `radivine-webp` — Minimal, audited, encode-only safe wrapper over `libwebp-sys` (`ADR-030`, `ADR-031`).
//!
//! Security invariants:
//! - Only the `libwebp` encoder entry points (`WebPConfigInitInternal`, `WebPValidateConfig`,
//!   `WebPPictureInitInternal`, `WebPPictureImportRGB`, `WebPPictureImportRGBA`, `WebPEncode`,
//!   `WebPPictureFree`) are called.
//! - Zero `libwebp` decoder or demuxer functions are exposed or invoked; all decoding of
//!   untrusted inputs in Radivine happens in pure memory-safe Rust (`image` / `image-webp`).
//! - Every `WebPPicture` allocation is protected by an RAII [`PictureGuard`] that calls
//!   `WebPPictureFree` on normal return, error return, or panic unwind.

use std::ffi::c_void;
use std::mem::MaybeUninit;

use libwebp_sys::{
    WEBP_ENCODER_ABI_VERSION, WEBP_MAX_DIMENSION, WebPConfig, WebPConfigInitInternal, WebPEncode,
    WebPEncodingError, WebPGetEncoderVersion, WebPPicture, WebPPictureFree, WebPPictureImportRGB,
    WebPPictureImportRGBA, WebPPictureInitInternal, WebPPreset, WebPValidateConfig,
};
use thiserror::Error;

/// Maximum canvas dimension supported by the WebP specification (`16,383` px).
pub const MAX_WEBP_DIMENSION: u32 = WEBP_MAX_DIMENSION;

/// Lossy WebP encoding configuration parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebPLossyConfig {
    /// Lossy quality factor in `0..=100`.
    pub quality: u8,
    /// Alpha plane quality factor in `0..=100` (`100` = lossless alpha).
    pub alpha_quality: u8,
    /// Speed/quality trade-off method in `0..=6` (`0` = fastest, `6` = slowest/best).
    pub method: u8,
}

impl Default for WebPLossyConfig {
    fn default() -> Self {
        Self {
            quality: 80,
            alpha_quality: 100,
            method: 4,
        }
    }
}

/// Owned pixel buffer input for [`encode_lossy`].
///
/// Passing an owned `Vec<u8>` allows [`encode_lossy`] to drop the source RGB/RGBA buffer
/// immediately after `WebPPictureImportRGB` / `WebPPictureImportRGBA` converts it into
/// `libwebp`'s YUV420 planes, preventing the decoded pixel buffer and `WebPEncode` working
/// buffers from overlapping in RSS (`Spike P7`).
#[derive(Debug, Clone)]
pub enum WebPEncodeInput {
    Rgb {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
    Rgba {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
}

/// Structured error returned by [`encode_lossy`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WebPEncodeError {
    #[error("invalid WebP dimensions {width}x{height} (must be 1..={MAX_WEBP_DIMENSION})")]
    InvalidDimensions { width: u32, height: u32 },

    #[error("pixel buffer length {actual} does not match expected {expected} bytes")]
    BufferLengthMismatch { expected: usize, actual: usize },

    #[error(
        "invalid WebP encoder configuration (quality={quality}, alpha_quality={alpha_quality}, method={method})"
    )]
    InvalidConfig {
        quality: u8,
        alpha_quality: u8,
        method: u8,
    },

    #[error("WebP encoding cancelled by caller")]
    Cancelled,

    #[error("libwebp encoder out of memory")]
    OutOfMemory,

    #[error("libwebp encoder failed with error code {code}")]
    EncoderFailure { code: i32 },
}

/// Returns the bundled `libwebp` encoder version as `(major, minor, revision)`.
#[must_use]
pub fn encoder_version() -> (u32, u32, u32) {
    // SAFETY: `WebPGetEncoderVersion` is a pure C function taking no arguments and returning a packed int.
    let v = unsafe { WebPGetEncoderVersion() } as u32;
    ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF)
}

/// RAII guard ensuring `WebPPictureFree` is always called once `WebPPictureInitInternal` succeeds.
struct PictureGuard {
    picture: WebPPicture,
}

impl Drop for PictureGuard {
    fn drop(&mut self) {
        // SAFETY: `self.picture` was initialized by `WebPPictureInitInternal` and `WebPPictureFree`
        // safely releases any internal buffers allocated by `WebPPictureImport*` or `WebPEncode`.
        unsafe {
            WebPPictureFree(&mut self.picture);
        }
    }
}

struct HookContext<'a> {
    out: Vec<u8>,
    should_cancel: &'a mut dyn FnMut(u8) -> bool,
}

unsafe extern "C" fn memory_writer_hook(
    data: *const u8,
    data_size: usize,
    picture: *const WebPPicture,
) -> std::ffi::c_int {
    if data.is_null() || picture.is_null() {
        return 0;
    }
    // SAFETY: `picture` points to our stack-pinned `PictureGuard::picture`, and `custom_ptr`
    // points to our stack-pinned `HookContext` for the duration of `WebPEncode`.
    unsafe {
        let ctx_ptr = (*picture).custom_ptr as *mut HookContext<'_>;
        if ctx_ptr.is_null() {
            return 0;
        }
        let ctx = &mut *ctx_ptr;
        let slice = std::slice::from_raw_parts(data, data_size);
        ctx.out.extend_from_slice(slice);
    }
    1
}

unsafe extern "C" fn progress_cancel_hook(
    percent: std::ffi::c_int,
    picture: *const WebPPicture,
) -> std::ffi::c_int {
    if picture.is_null() {
        return 0;
    }
    // SAFETY: `picture` points to our stack-pinned `PictureGuard::picture`, and `custom_ptr`
    // points to our stack-pinned `HookContext` for the duration of `WebPEncode`.
    unsafe {
        let ctx_ptr = (*picture).custom_ptr as *mut HookContext<'_>;
        if ctx_ptr.is_null() {
            return 0;
        }
        let ctx = &mut *ctx_ptr;
        let pct = percent.clamp(0, 100) as u8;
        if (ctx.should_cancel)(pct) { 0 } else { 1 }
    }
}

/// Encodes an RGB8 or RGBA8 buffer into a lossy WebP container (`RIFF....WEBP`).
///
/// Calls `should_cancel(percent)` periodically during encoding; if `should_cancel` returns `true`,
/// encoding aborts immediately with [`WebPEncodeError::Cancelled`].
pub fn encode_lossy(
    input: WebPEncodeInput,
    cfg: &WebPLossyConfig,
    should_cancel: &mut dyn FnMut(u8) -> bool,
) -> Result<Vec<u8>, WebPEncodeError> {
    let (pixels, width, height, channels) = match input {
        WebPEncodeInput::Rgb {
            pixels,
            width,
            height,
        } => (pixels, width, height, 3usize),
        WebPEncodeInput::Rgba {
            pixels,
            width,
            height,
        } => (pixels, width, height, 4usize),
    };

    if width == 0 || height == 0 || width > MAX_WEBP_DIMENSION || height > MAX_WEBP_DIMENSION {
        return Err(WebPEncodeError::InvalidDimensions { width, height });
    }

    let stride_bytes = (width as usize)
        .checked_mul(channels)
        .ok_or(WebPEncodeError::InvalidDimensions { width, height })?;
    let expected_len = stride_bytes
        .checked_mul(height as usize)
        .ok_or(WebPEncodeError::InvalidDimensions { width, height })?;
    if pixels.len() != expected_len {
        return Err(WebPEncodeError::BufferLengthMismatch {
            expected: expected_len,
            actual: pixels.len(),
        });
    }

    if cfg.quality > 100 || cfg.alpha_quality > 100 || cfg.method > 6 {
        return Err(WebPEncodeError::InvalidConfig {
            quality: cfg.quality,
            alpha_quality: cfg.alpha_quality,
            method: cfg.method,
        });
    }

    let stride_i32 = i32::try_from(stride_bytes)
        .map_err(|_| WebPEncodeError::InvalidDimensions { width, height })?;

    // SAFETY: `WebPConfigInitInternal` initializes `raw_cfg` when ABI version matches.
    let mut webp_cfg = unsafe {
        let mut raw_cfg = MaybeUninit::<WebPConfig>::uninit();
        if WebPConfigInitInternal(
            raw_cfg.as_mut_ptr(),
            WebPPreset::WEBP_PRESET_DEFAULT,
            f32::from(cfg.quality),
            WEBP_ENCODER_ABI_VERSION as i32,
        ) == 0
        {
            return Err(WebPEncodeError::InvalidConfig {
                quality: cfg.quality,
                alpha_quality: cfg.alpha_quality,
                method: cfg.method,
            });
        }
        raw_cfg.assume_init()
    };

    webp_cfg.lossless = 0;
    webp_cfg.quality = f32::from(cfg.quality);
    webp_cfg.alpha_quality = i32::from(cfg.alpha_quality);
    webp_cfg.method = i32::from(cfg.method);
    webp_cfg.thread_level = 1;

    // SAFETY: `webp_cfg` is fully initialized.
    if unsafe { WebPValidateConfig(&webp_cfg) } == 0 {
        return Err(WebPEncodeError::InvalidConfig {
            quality: cfg.quality,
            alpha_quality: cfg.alpha_quality,
            method: cfg.method,
        });
    }

    // SAFETY: `WebPPictureInitInternal` initializes `raw_pic`, which is immediately wrapped in `PictureGuard`.
    let mut guard = unsafe {
        let mut raw_pic = MaybeUninit::<WebPPicture>::uninit();
        if WebPPictureInitInternal(raw_pic.as_mut_ptr(), WEBP_ENCODER_ABI_VERSION as i32) == 0 {
            return Err(WebPEncodeError::EncoderFailure { code: -1 });
        }
        PictureGuard {
            picture: raw_pic.assume_init(),
        }
    };

    // Keep `use_argb = 0` so `WebPPictureImport*` converts directly into YUV420 (+ alpha) planes.
    guard.picture.use_argb = 0;
    guard.picture.width = width as i32;
    guard.picture.height = height as i32;

    // SAFETY: `pixels` has been verified to have exact length `stride_bytes * height`.
    let imported = unsafe {
        if channels == 4 {
            WebPPictureImportRGBA(&mut guard.picture, pixels.as_ptr(), stride_i32)
        } else {
            WebPPictureImportRGB(&mut guard.picture, pixels.as_ptr(), stride_i32)
        }
    };

    // Drop the owned RGB/RGBA buffer before `WebPEncode` allocates bitstream buffers.
    drop(pixels);

    if imported == 0 {
        let code = guard.picture.error_code as i32;
        return Err(map_webp_error(code));
    }

    let mut ctx = HookContext {
        out: Vec::with_capacity((width as usize * height as usize) / 4),
        should_cancel,
    };

    guard.picture.writer = Some(memory_writer_hook);
    guard.picture.progress_hook = Some(progress_cancel_hook);
    guard.picture.custom_ptr = (&mut ctx as *mut HookContext<'_>).cast::<c_void>();

    // SAFETY: `webp_cfg`, `guard.picture`, and `ctx` are valid and outlive `WebPEncode`.
    let ok = unsafe { WebPEncode(&webp_cfg, &mut guard.picture) };
    guard.picture.custom_ptr = std::ptr::null_mut();

    if ok == 0 {
        let code = guard.picture.error_code as i32;
        return Err(map_webp_error(code));
    }

    Ok(ctx.out)
}

fn map_webp_error(code: i32) -> WebPEncodeError {
    if code == WebPEncodingError::VP8_ENC_ERROR_USER_ABORT as i32 {
        WebPEncodeError::Cancelled
    } else if code == WebPEncodingError::VP8_ENC_ERROR_OUT_OF_MEMORY as i32
        || code == WebPEncodingError::VP8_ENC_ERROR_BITSTREAM_OUT_OF_MEMORY as i32
    {
        WebPEncodeError::OutOfMemory
    } else {
        WebPEncodeError::EncoderFailure { code }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_libwebp_version_is_at_least_1_3_2() {
        let ver = encoder_version();
        assert!(ver >= (1, 3, 2), "bundled libwebp {ver:?} must be >= 1.3.2");
    }

    #[test]
    fn encode_rgb_and_rgba_and_cancel_mid_encode() {
        let width = 64u32;
        let height = 48u32;
        let rgb = vec![128u8; (width * height * 3) as usize];
        let out = encode_lossy(
            WebPEncodeInput::Rgb {
                pixels: rgb.clone(),
                width,
                height,
            },
            &WebPLossyConfig::default(),
            &mut |_| false,
        )
        .unwrap();
        assert!(out.starts_with(b"RIFF"));
        assert_eq!(&out[8..12], b"WEBP");

        // Cancellation via progress_hook
        let err = encode_lossy(
            WebPEncodeInput::Rgb {
                pixels: rgb,
                width,
                height,
            },
            &WebPLossyConfig::default(),
            &mut |_| true,
        )
        .unwrap_err();
        assert_eq!(err, WebPEncodeError::Cancelled);
    }

    #[test]
    fn invalid_dimensions_and_buffer_lengths_return_errors_without_panicking() {
        let res_zero = encode_lossy(
            WebPEncodeInput::Rgb {
                pixels: Vec::new(),
                width: 0,
                height: 10,
            },
            &WebPLossyConfig::default(),
            &mut |_| false,
        );
        assert!(matches!(
            res_zero,
            Err(WebPEncodeError::InvalidDimensions { .. })
        ));

        let res_over = encode_lossy(
            WebPEncodeInput::Rgb {
                pixels: vec![0; 3],
                width: 16_384,
                height: 1,
            },
            &WebPLossyConfig::default(),
            &mut |_| false,
        );
        assert!(matches!(
            res_over,
            Err(WebPEncodeError::InvalidDimensions { .. })
        ));

        let res_len = encode_lossy(
            WebPEncodeInput::Rgba {
                pixels: vec![0; 10],
                width: 4,
                height: 4,
            },
            &WebPLossyConfig::default(),
            &mut |_| false,
        );
        assert!(matches!(
            res_len,
            Err(WebPEncodeError::BufferLengthMismatch { .. })
        ));
    }
}
