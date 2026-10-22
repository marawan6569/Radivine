//! MIT-SHM `memfd` pixel buffer and damage-clipped RGBA8→ZPixmap swizzling (`PLATFORM_INTEGRATION.md` §3.5).

use std::os::fd::OwnedFd;
use std::ptr::NonNull;

use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, Gcontext, ImageFormat, ImageOrder, Window};
use x11rb::rust_connection::RustConnection;

use crate::{FrameRef, PlatformError, RectPx};

/// Byte-index mapping from premultiplied `[R, G, B, A]` source bytes to a 32-bit X11 `ZPixmap` pixel.
///
/// Derived dynamically from the X server's `image_byte_order` (`LSBFirst` or `MSBFirst`) and
/// the chosen visual's `red_mask`, `green_mask`, and `blue_mask` (`PLATFORM_INTEGRATION.md` §3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelShuffle {
    /// Destination byte offset (`0..=3`) within each 4-byte pixel for Red.
    pub r_idx: usize,
    /// Destination byte offset (`0..=3`) within each 4-byte pixel for Green.
    pub g_idx: usize,
    /// Destination byte offset (`0..=3`) within each 4-byte pixel for Blue.
    pub b_idx: usize,
    /// Destination byte offset (`0..=3`) within each 4-byte pixel for Alpha.
    pub a_idx: usize,
}

impl ChannelShuffle {
    /// Derives the 4-byte channel permutation from the server's `image_byte_order` and visual masks.
    #[must_use]
    pub fn from_visual(
        byte_order: ImageOrder,
        red_mask: u32,
        green_mask: u32,
        blue_mask: u32,
    ) -> Self {
        let mask_to_byte = |mask: u32| -> usize {
            let shift_bytes = (mask.trailing_zeros() / 8).min(3) as usize;
            if byte_order == ImageOrder::LSB_FIRST {
                shift_bytes
            } else {
                3 - shift_bytes
            }
        };

        let r_idx = mask_to_byte(red_mask);
        let g_idx = mask_to_byte(green_mask);
        let b_idx = mask_to_byte(blue_mask);
        // The remaining byte in `[0, 1, 2, 3]` holds the alpha channel.
        let used = (1u8 << r_idx) | (1u8 << g_idx) | (1u8 << b_idx);
        let a_idx = (0..4).find(|&i| (used & (1u8 << i)) == 0).unwrap_or(3);

        Self {
            r_idx,
            g_idx,
            b_idx,
            a_idx,
        }
    }
}

/// Clips `rect` to `[0, width) × [0, height)`. Returns `None` if the clipped rectangle is empty.
#[inline]
#[must_use]
pub fn clip_damage_rect(rect: RectPx, width: u32, height: u32) -> Option<RectPx> {
    let x0 = rect.x.max(0) as u32;
    let y0 = rect.y.max(0) as u32;
    let x1 = (rect.x.saturating_add(rect.w as i32)).max(0) as u32;
    let y1 = (rect.y.saturating_add(rect.h as i32)).max(0) as u32;

    let cx0 = x0.min(width);
    let cy0 = y0.min(height);
    let cx1 = x1.min(width);
    let cy1 = y1.min(height);

    if cx1 > cx0 && cy1 > cy0 {
        Some(RectPx::new(cx0 as i32, cy0 as i32, cx1 - cx0, cy1 - cy0))
    } else {
        None
    }
}

/// Copies and swizzles **only** the rows and columns inside `clipped` from `src_rgba` into `dst_zpixmap`.
///
/// For the common `LSBFirst` ARGB visual (`r_idx=2, g_idx=1, b_idx=0, a_idx=3`), uses a fast tight loop
/// that swaps `R` and `B` per pixel (`[R, G, B, A] -> [B, G, R, A]`).
pub fn swizzle_damage_rect(
    src_rgba: &[u8],
    src_stride: usize,
    dst_zpixmap: &mut [u8],
    dst_stride: usize,
    clipped: RectPx,
    shuffle: ChannelShuffle,
) {
    let x0 = clipped.x as usize;
    let y0 = clipped.y as usize;
    let w = clipped.w as usize;
    let h = clipped.h as usize;
    let row_bytes = w * 4;

    let is_bgra =
        shuffle.b_idx == 0 && shuffle.g_idx == 1 && shuffle.r_idx == 2 && shuffle.a_idx == 3;

    for row in 0..h {
        let sy = y0 + row;
        let s_off = sy * src_stride + x0 * 4;
        let d_off = sy * dst_stride + x0 * 4;
        let src_row = &src_rgba[s_off..s_off + row_bytes];
        let dst_row = &mut dst_zpixmap[d_off..d_off + row_bytes];

        if is_bgra {
            for (d, s) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
        } else {
            for (d, s) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
                d[shuffle.r_idx] = s[0];
                d[shuffle.g_idx] = s[1];
                d[shuffle.b_idx] = s[2];
                d[shuffle.a_idx] = s[3];
            }
        }
    }
}

struct MmapBuffer {
    _fd: OwnedFd,
    ptr: NonNull<u8>,
    len: usize,
    seg: shm::Seg,
}

#[allow(unsafe_code)]
impl MmapBuffer {
    fn create(conn: &RustConnection, width: u32, height: u32) -> Result<Self, PlatformError> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .ok_or_else(|| PlatformError::Os("SHM buffer size overflow".to_string()))?;
        let len = len.max(4);

        let fd = memfd_create(c"radivine-shm", MemfdFlags::CLOEXEC)
            .map_err(|e| PlatformError::Os(format!("memfd_create failed: {e}")))?;
        ftruncate(&fd, len as u64)
            .map_err(|e| PlatformError::Os(format!("ftruncate failed: {e}")))?;

        // SAFETY: `fd` is a newly created anonymous memfd sized to `len` bytes via `ftruncate`.
        // Mapping it `MAP_SHARED` with `READ | WRITE` produces a valid region of `len` bytes
        // owned exclusively by this `MmapBuffer` until `Drop` detaches the X11 SHM segment and unmaps it.
        let raw_ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )
            .map_err(|e| PlatformError::Os(format!("mmap failed: {e}")))?
        };
        let ptr = NonNull::new(raw_ptr.cast::<u8>())
            .ok_or_else(|| PlatformError::Os("mmap returned null pointer".to_string()))?;

        let seg: shm::Seg = conn
            .generate_id()
            .map_err(|e| PlatformError::X11(e.to_string()))?;
        let dup_fd = rustix::io::dup(&fd)
            .map_err(|e| PlatformError::Os(format!("dup(memfd) failed: {e}")))?;
        conn.shm_attach_fd(seg, dup_fd, true)
            .map_err(|e| PlatformError::X11(e.to_string()))?
            .check()
            .map_err(|e| PlatformError::X11(format!("shm_attach_fd rejected: {e}")))?;

        Ok(Self {
            _fd: fd,
            ptr,
            len,
            seg,
        })
    }

    #[inline]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `self.ptr` points to `self.len` valid mapped bytes and `&mut self` guarantees exclusive access.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    fn detach_and_drop(self, conn: &RustConnection) {
        let _ = conn.shm_detach(self.seg);
        drop(self);
    }
}

#[allow(unsafe_code)]
impl Drop for MmapBuffer {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` and `self.len` were returned by `mmap` in `MmapBuffer::create`.
        unsafe {
            let _ = munmap(self.ptr.as_ptr().cast(), self.len);
        }
    }
}

/// Manages the overlay's MIT-SHM segment (with automatic core `PutImage` fallback if SHM is unavailable).
pub struct ShmPresenter {
    shm_supported: bool,
    depth: u8,
    shuffle: ChannelShuffle,
    width: u32,
    height: u32,
    mmap_buf: Option<MmapBuffer>,
    fallback_buf: Vec<u8>,
    pending_damage: Vec<RectPx>,
}

impl ShmPresenter {
    /// Creates a new [`ShmPresenter`] for the given visual depth and [`ChannelShuffle`].
    #[must_use]
    pub fn new(shm_supported: bool, depth: u8, shuffle: ChannelShuffle) -> Self {
        Self {
            shm_supported,
            depth,
            shuffle,
            width: 0,
            height: 0,
            mmap_buf: None,
            fallback_buf: Vec::new(),
            pending_damage: Vec::new(),
        }
    }

    /// Ensures the backing buffer is allocated for `(width, height)`.
    pub fn ensure_size(
        &mut self,
        conn: &RustConnection,
        width: u32,
        height: u32,
    ) -> Result<(), PlatformError> {
        let w = width.max(1);
        let h = height.max(1);
        if self.width == w
            && self.height == h
            && (self.mmap_buf.is_some() || !self.fallback_buf.is_empty())
        {
            return Ok(());
        }

        self.release(conn);
        self.width = w;
        self.height = h;

        if self.shm_supported {
            match MmapBuffer::create(conn, w, h) {
                Ok(buf) => {
                    self.mmap_buf = Some(buf);
                    return Ok(());
                }
                Err(err) => {
                    tracing::warn!(
                        "MIT-SHM allocation failed ({err}); falling back to core PutImage"
                    );
                    self.shm_supported = false;
                }
            }
        }

        let len = (w as usize) * (h as usize) * 4;
        self.fallback_buf.resize(len, 0);
        Ok(())
    }

    /// Swizzles only the damaged rectangles of `frame` into the SHM buffer and, if `is_mapped` is
    /// `true`, uploads them immediately to `win`. If `!is_mapped`, queues the clipped rectangles so
    /// [`Self::flush_pending`] can emit `ShmPutImage` in the same X11 batch right after `MapWindow`
    /// (since Xorg Composite only allocates the redirected window pixmap upon `MapWindow`).
    pub fn present(
        &mut self,
        conn: &RustConnection,
        win: Window,
        gc: Gcontext,
        frame: FrameRef<'_>,
        damage: &[RectPx],
        is_mapped: bool,
    ) -> Result<(), PlatformError> {
        if damage.is_empty() || frame.width == 0 || frame.height == 0 {
            return Ok(());
        }
        self.ensure_size(conn, frame.width, frame.height)?;

        let w = self.width;
        let h = self.height;
        let dst_stride = (w as usize) * 4;
        let shuffle = self.shuffle;

        let mut clipped_rects = Vec::with_capacity(damage.len());
        if let Some(ref mut shm_buf) = self.mmap_buf {
            let dst = shm_buf.as_mut_slice();
            for rect in damage {
                if let Some(clipped) = clip_damage_rect(*rect, w, h) {
                    swizzle_damage_rect(
                        frame.data,
                        frame.stride,
                        dst,
                        dst_stride,
                        clipped,
                        shuffle,
                    );
                    clipped_rects.push(clipped);
                }
            }
        } else {
            for rect in damage {
                if let Some(clipped) = clip_damage_rect(*rect, w, h) {
                    swizzle_damage_rect(
                        frame.data,
                        frame.stride,
                        &mut self.fallback_buf,
                        dst_stride,
                        clipped,
                        shuffle,
                    );
                    clipped_rects.push(clipped);
                }
            }
        }

        if is_mapped {
            self.upload_rects(conn, win, gc, &clipped_rects)?;
        } else {
            self.pending_damage.extend(clipped_rects);
        }
        Ok(())
    }

    /// Uploads any damage rectangles that were prepared while the window was unmapped.
    pub fn flush_pending(
        &mut self,
        conn: &RustConnection,
        win: Window,
        gc: Gcontext,
    ) -> Result<(), PlatformError> {
        if self.pending_damage.is_empty() {
            return Ok(());
        }
        let rects = std::mem::take(&mut self.pending_damage);
        self.upload_rects(conn, win, gc, &rects)
    }

    fn upload_rects(
        &self,
        conn: &RustConnection,
        win: Window,
        gc: Gcontext,
        rects: &[RectPx],
    ) -> Result<(), PlatformError> {
        let w = self.width;
        let h = self.height;
        let dst_stride = (w as usize) * 4;

        if let Some(ref shm_buf) = self.mmap_buf {
            let seg = shm_buf.seg;
            for &clipped in rects {
                conn.shm_put_image(
                    win,
                    gc,
                    w as u16,
                    h as u16,
                    clipped.x as u16,
                    clipped.y as u16,
                    clipped.w as u16,
                    clipped.h as u16,
                    clipped.x as i16,
                    clipped.y as i16,
                    self.depth,
                    ImageFormat::Z_PIXMAP.into(),
                    false,
                    seg,
                    0,
                )
                .map_err(|e| PlatformError::X11(e.to_string()))?;
            }
            return Ok(());
        }

        // Core PutImage fallback, chunked by row bands to stay well under maximum_request_length
        let max_req_bytes = conn.maximum_request_bytes().saturating_sub(64).max(16_384);
        for &clipped in rects {
            let rw = clipped.w as usize;
            let rh = clipped.h as usize;
            let row_bytes = rw * 4;
            let rows_per_chunk = (max_req_bytes / row_bytes.max(1)).max(1).min(rh);

            let mut row_offset = 0usize;
            while row_offset < rh {
                let band_h = (rh - row_offset).min(rows_per_chunk);
                let mut band_data = Vec::with_capacity(row_bytes * band_h);
                for r in 0..band_h {
                    let y = (clipped.y as usize) + row_offset + r;
                    let x = clipped.x as usize;
                    let start = y * dst_stride + x * 4;
                    band_data.extend_from_slice(&self.fallback_buf[start..start + row_bytes]);
                }
                conn.put_image(
                    ImageFormat::Z_PIXMAP,
                    win,
                    gc,
                    rw as u16,
                    band_h as u16,
                    clipped.x as i16,
                    (clipped.y + row_offset as i32) as i16,
                    0,
                    self.depth,
                    &band_data,
                )
                .map_err(|e| PlatformError::X11(e.to_string()))?;
                row_offset += band_h;
            }
        }

        Ok(())
    }

    /// Detaches and frees any allocated SHM or fallback pixel buffer (`PLATFORM_INTEGRATION.md` §3.4).
    pub fn release(&mut self, conn: &RustConnection) {
        if let Some(buf) = self.mmap_buf.take() {
            buf.detach_and_drop(conn);
        }
        self.fallback_buf.clear();
        self.fallback_buf.shrink_to_fit();
        self.pending_damage.clear();
        self.width = 0;
        self.height = 0;
    }
}
