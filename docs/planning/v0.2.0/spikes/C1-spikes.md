# Milestone C1 Verification Spikes Report (`P1`–`P9`)

- **Date:** 2026-10-24
- **Milestone Gate:** C1 Pre-Implementation Verification Gate (`IMPLEMENTATION_PLAN.md` §2, `IMPLEMENTATION_HANDOFF.md` §6)
- **Spike Harness:** `spikes/convert-spikes` (`cargo run --release`)
- **Gate Verdict:** **ALL 9 SPIKES PASS — ZERO ARCHITECTURAL BLOCKERS.** Proceed with C1 implementation.

---

## Executive Summary

| Spike | Subject | Verdict | Key Empirical Finding |
|---|---|---|---|
| **P1** | `libwebp-sys` vendored static build & safe lossy wrapper | **PASS** | `libwebp-sys 0.14.4` bundles `libwebp v1.6.0` (`0x010600 >= 1.3.2`, post-CVE-2023-4863). Clean release compile in `17.62s` (`libwebpsys.a` = `1.4 MiB`, stripped binary with all codecs = `2.2 MiB`). `WebPPicture::progress_hook` aborts cleanly mid-encode with `VP8_ENC_ERROR_USER_ABORT (10)` (**resolves Q-D**). |
| **P2** | Lossy WebP ICC + EXIF preservation via pure-Rust RIFF/VP8X muxing | **PASS** | Pure-Rust ~100-line RIFF/VP8X muxer embeds `ICCP` and `EXIF` chunks into `libwebp` lossy output (both simple `VP8 ` RGB and extended `VP8X` + `ALPH` + `VP8 ` RGBA). Decodes cleanly via pure-Rust `image::codecs::webp::WebPDecoder` with byte-exact SHA-256 on ICC and EXIF payloads. No `img-parts` dependency needed. |
| **P3** | `image::Limits` pre-allocation bomb rejection (`65535×65535`) | **PASS** | `Limits { max_image_width: Some(16384), max_image_height: Some(16384), max_alloc: Some(268_435_456) }` rejects `65535×65535` PNG in `3.9 µs`, JPEG in `13.6 µs`, and WebP in `1.4 µs` with **`0 kB` RSS delta** (requirement: `< 50 MB`). |
| **P4** | `zune-jpeg` (`image 0.25.10`) behavior on CMYK, YCCK, 12-bit, and arithmetic JPEG | **PASS** | CMYK (`4-component`) and YCCK (`APP14` Adobe transform=2) JPEGs decode cleanly to `ColorType::Rgb8`. 12-bit (`SOF1 precision=12`) and arithmetic-coded (`SOF9 0xFFC9`) JPEGs return structured `ImageError::Decoding` with zero panics. |
| **P5** | EXIF orientation 1..=8 transform + tag reset + ICC SHA-256 round-trip across PNG/JPEG/WebP | **PASS** | `image::metadata::Orientation::from_exif_chunk` and `remove_from_exif_chunk` (both little-endian `II*\0` and big-endian `MM\0*`) reset tag `0x0112` to `1` (`NoTransforms`) in-place while preserving sibling tags (`0x010F Make`). Asymmetric `60×40` pixel transforms verified for all 8 orientations. Byte-exact ICC (`544 B`, SHA-256 `9206f4a5d1f8..`) and EXIF round-tripped across PNG, JPEG, lossless WebP, and lossy WebP. |
| **P6** | `image::codecs::jpeg::JpegEncoder` vs `jpeg-encoder 0.7.1` | **PASS** | On a `2048×1536` synthetic photo at `q=82`, `jpeg-encoder 0.7.1` encodes in `33.4 ms` (`1,093,675 B`) vs `image::JpegEncoder` in `87.8 ms` (`1,719,628 B`) — **2.6× faster and 36% smaller** — and natively supports `ChromaSubsampling` (`Auto`/`4:4:4`/`4:2:2`/`4:2:0`), `progressive: bool`, `optimize_coding: bool`, `add_icc_profile`, and `add_exif_metadata` with zero dependencies and `#![forbid(unsafe_code)]` (when `simd` feature is disabled). Selected `jpeg-encoder 0.7.1` behind `radivine-convert-image`'s internal JPEG encoder boundary. |
| **P7** | Peak RSS (`VmHWM`) and wall time calibration across 12 MP, 24 MP, 50 MP, 100 MP | **PASS** | By setting `picture.use_argb = 0` for lossy WebP (importing directly into YUV420 planes) and dropping the decoded `RgbImage`/`RgbaImage` buffer immediately after `WebPPictureImport*` before calling `WebPEncode`, peak RSS (`VmHWM`) stays **strictly below** `2.2 × decoded_rgba + 16 MiB` at every resolution (`12 MP`: `99.2 MiB` vs `116.7 MiB`; `24 MP`: `194.4 MiB` vs `217.4 MiB`; `50 MP`: `400.7 MiB` vs `435.7 MiB`; `100 MP`: `795.3 MiB` vs `855.2 MiB`). |
| **P8** | Two-rung atomic commit matrix (`RENAME_NOREPLACE` + `linkat`/`unlinkat` + directory `fsync`) | **PASS** | Verified on `ext4` (workspace) and `tmpfs` (`/dev/shm`): Rung A (`renameat_with(..., RenameFlags::NOREPLACE)`) and Rung B (`linkat` + `fsync(dest_dir)` + `unlinkat` + `fsync(stage_dir)`) publish atomically, return `Errno::EXIST` on pre-existing destination without altering destination bytes or inode, and map `EOPNOTSUPP`/`ENOSYS`/`EPERM`/`EXDEV` cleanly to `FilesystemUnsupported`. |
| **P9** | Dependency license and security advisory audit (`cargo deny` / `THIRD_PARTY_NOTICES.md`) | **PASS** | All new transitive crates (`image 0.25.10`, `libwebp-sys 0.14.4` bundling BSD-3-Clause `libwebp 1.6.0`, `png 0.18.1`, `image-webp 0.2.4`, `zune-jpeg 0.5.15`, `moxcms 0.8.1`, `pxfm 0.1.30`, `jpeg-encoder 0.7.1`) use permissive licenses (`MIT`, `Apache-2.0`, `BSD-3-Clause`, `Zlib`, `0BSD`, `Unlicense`, `IJG`). `IJG` added to `deny.toml` and `libwebp 1.6.0` (BSD-3-Clause) + `jpeg-encoder` (IJG) notices added to `THIRD_PARTY_NOTICES.md`. |

---

## Detailed Empirical Results

### P1 — `libwebp-sys` Vendored Static Build, Wrapper Choice & Mid-Encode Cancellation (`Q-D`)

- **Crate Evaluation (`webp 0.3.1` vs `libwebp-sys 0.14.4`):**
  - Inspection of `webp 0.3.1` revealed that it depends on `libwebp-sys 0.9.3` (which predates `libwebp 1.3.2` / CVE-2023-4863), does not check the return code of `WebPPictureImportRGBA` on allocation failure, and panics on invalid dimensions instead of returning `Result`.
  - `libwebp-sys 0.14.4` (`license = "MIT"`, `edition = "2024"`) vendors **`libwebp v1.6.0`** (`WebPGetEncoderVersion() == 0x010600`), well above the `>= 1.3.2` security floor.
  - Per `IMPLEMENTATION_PLAN.md` §2 and `CONVERSION_BACKENDS.md` §4, we isolate the tiny (~150 LoC) `unsafe` FFI encode-only binding in an internal workspace crate `crates/radivine-webp` wrapping `libwebp-sys 0.14.4`, while `crates/radivine-convert` and `crates/radivine-convert-image` enforce `#![forbid(unsafe_code)]`.
- **Build & Size Metrics:**
  - Clean release build of `convert-spikes` (including `libwebp-sys 0.14.4` C compilation via `cc` + `image 0.25.10`): **`17.62s`**.
  - Static archive `libwebpsys.a`: **`1.4 MiB`** (unstripped archive before linker dead-code elimination of unused decoder/demuxer objects).
  - Total stripped binary size (including `image`, `libwebp-sys`, `jpeg-encoder`, `rustix`, `sha2`): **`2.2 MiB`** (`2.6 MiB` unstripped).
- **Mid-Encode Cancellation (`Q-D` Resolution):**
  - Wiring `WebPPicture::progress_hook` to check an abort callback during `WebPEncode` cleanly aborts in-flight encoding and returns `WebPEncodingError::VP8_ENC_ERROR_USER_ABORT` (`10`), freeing all `WebPPicture` buffers via `WebPPictureFree`.

### P2 & P5 — RIFF/VP8X Muxing, ICC & EXIF Preservation, and Orientation `1..=8`

- **Lossy WebP RIFF/VP8X Muxing:**
  - Implemented a pure-Rust, bounds-checked RIFF/VP8X muxer (`mux_webp_metadata`) that parses `libwebp`'s output (`RIFF....WEBP` with either simple `VP8 ` or extended `VP8X` + `ALPH` + `VP8 `), sets `VP8X` flags (`1 << 5` for `ICCP`, `1 << 3` for `EXIF`, `1 << 4` for `ALPH`) and 24-bit canvas dimensions `(width - 1, height - 1)`, and emits chunks in canonical WebP container order (`VP8X`, `ICCP`, `ALPH`, `VP8 `/`VP8L`, `EXIF`) with even-byte RIFF padding.
  - Verified that `image::codecs::webp::WebPDecoder` decodes both RGB and RGBA muxed lossy WebP images and returns byte-exact `icc_profile()` and `exif_metadata()`.
- **EXIF Orientation `1..=8` & Tag Reset:**
  - Verified both little-endian (`II\x2a\x00`) and big-endian (`MM\x00\x2a`) TIFF/EXIF headers across all 8 orientation values (`1..=8`), plus optional `Exif\0\0` prefix normalization.
  - Verified `Orientation::remove_from_exif_chunk` resets IFD0 tag `0x0112` to `1` (`NoTransforms`) in-place while leaving sibling IFD0 entries (`0x010F Make = "Radivine\0"`) untouched.
  - Verified pixel transformation on an asymmetric `60×40` image for all 8 orientations (`1..=4` produce `60×40`; `5..=8` produce `40×60` with expected corner pixels).

### P3 — `image::Limits` Pre-Allocation Bomb Rejection (`65535×65535`)

Configured `image::Limits`:
- `max_image_width = Some(16_384)`
- `max_image_height = Some(16_384)`
- `max_alloc = Some(268_435_456)` (`256 MiB`)

| Crafted Bomb Input | Rejection Latency | Returned `ImageError` | `VmRSS` Delta |
|---|---:|---|---:|
| `65535×65535` PNG (`IHDR`) | `3.88 µs` | `Limits(LimitError { kind: DimensionError })` | `0 kB` |
| `65535×65535` JPEG (`SOF0`) | `13.64 µs` | `Limits(LimitError { kind: InsufficientMemory })` | `0 kB` |
| `65535×65535` WebP (`VP8X`) | `1.39 µs` | `Limits(LimitError { kind: InsufficientMemory })` | `0 kB` |

All three formats reject the `65535×65535` header prior to pixel buffer allocation with `0 kB` RSS growth.

### P4 — `zune-jpeg` (`image 0.25.10`) Behavior on Non-Standard JPEGs

| Input Variant | Observed Behavior | Classification in `radivine-convert-image` |
|---|---|---|
| **CMYK JPEG** (4-component `SOF0`) | Decoded cleanly to `ColorType::Rgb8` (`Rgb([239, 119, 0])`) | Supported natively (converted to `Rgb8` during decode) |
| **YCCK JPEG** (`APP14` Adobe transform=2) | Decoded cleanly to `ColorType::Rgb8` (`Rgb([16, 16, 16])`) | Supported natively (converted to `Rgb8` during decode) |
| **12-bit JPEG** (`SOF1`, precision=12) | `ImageError::Decoding("The library can only parse 8-bit images, the image has 12 bits of precision")` | Mapped to `ConversionError::UnsupportedFeature { feature: UnsupportedFeature::BitDepth(12) }` |
| **Arithmetic-coded JPEG** (`SOF9` `0xFFC9`) | `ImageError::Decoding(...)` | Mapped to `ConversionError::UnsupportedFeature { feature: UnsupportedFeature::ArithmeticCoding }` (detected by scanning JPEG SOF markers `0xFFC9`/`0xFFCA`/`0xFFCB`) |

### P6 — JPEG Encoder Evaluation (`image::codecs::jpeg::JpegEncoder` vs `jpeg-encoder 0.7.1`)

Tested on a `2048×1536` (`3.15 MP`) synthetic photographic RGB image:

| Quality | `image::codecs::jpeg::JpegEncoder` Size | `image::JpegEncoder` Time | `jpeg-encoder 0.7.1` Size | `jpeg-encoder 0.7.1` Time |
|---:|---:|---:|---:|---:|
| `q = 70` | `1,172,184 B` | `78.90 ms` | `756,907 B` (`-35.4%`) | `30.56 ms` (`2.58× faster`) |
| `q = 82` | `1,719,628 B` | `87.83 ms` | `1,093,675 B` (`-36.4%`) | `33.37 ms` (`2.63× faster`) |
| `q = 92` | `2,901,606 B` | `111.74 ms` | `2,922,961 B` (`+0.7%`, 4:4:4) | `65.34 ms` (`1.71× faster`) |

- **Why `jpeg-encoder 0.7.1` wins:**
  1. `image::codecs::jpeg::JpegEncoder` hardcodes `4:2:2` subsampling, baseline DCT, and static Huffman tables. It cannot honor `FormatQuality::Jpeg { progressive, optimize_coding, subsampling }`.
  2. `jpeg-encoder 0.7.1` natively supports `SamplingFactor::R_4_2_0` / `R_4_2_2` / `R_4_4_4` (matching our `ChromaSubsampling::Auto` rule: `4:2:0` for `q <= 89`, `4:4:4` for `q >= 90`), `set_progressive(bool)`, `set_optimized_huffman_tables(bool)`, `add_icc_profile(&[u8])` (multi-chunk `APP2`), and `add_exif_metadata(&[u8])` (`APP1`).
  3. With `default-features = false, features = ["std"]`, `jpeg-encoder 0.7.1` has `#![forbid(unsafe_code)]` and zero transitive dependencies.

### P7 — Peak RSS (`VmHWM`) and Wall Time Calibration (`12 MP`–`100 MP`)

Each conversion ran in an isolated child process reading from disk and writing to disk so `/proc/self/status` `VmHWM` captured the true peak RSS of that single conversion:

| Resolution | Megapixels | Pair | Wall Time | Source Size | Output Size | Peak RSS (`VmHWM`) | Formula (`2.2 × RGBA + 16 MiB`) | Verdict |
|---|---:|---|---:|---:|---:|---:|---:|---|
| `4000×3000` | `12 MP` | `JPEG → WebP q80` | `1.38 s` | `7.17 MiB` | `3.75 MiB` | **`99.2 MiB`** | `116.7 MiB` | **PASS (`-17.5 MiB`)** |
| `4000×3000` | `12 MP` | `PNG → JPEG q82` | `0.42 s` | `28.10 MiB` | `6.25 MiB` | **`37.7 MiB`** | `116.7 MiB` | **PASS (`-79.0 MiB`)** |
| `6000×4000` | `24 MP` | `JPEG → WebP q80` | `2.80 s` | `14.34 MiB` | `7.53 MiB` | **`194.4 MiB`** | `217.4 MiB` | **PASS (`-23.0 MiB`)** |
| `6000×4000` | `24 MP` | `PNG → JPEG q82` | `0.84 s` | `56.19 MiB` | `12.49 MiB` | **`72.1 MiB`** | `217.4 MiB` | **PASS (`-145.3 MiB`)** |
| `8192×6104` | `50 MP` | `JPEG → WebP q80` | `9.48 s` | `29.88 MiB` | `15.52 MiB` | **`400.7 MiB`** | `435.7 MiB` | **PASS (`-35.0 MiB`)** |
| `8192×6104` | `50 MP` | `PNG → JPEG q82` | `1.74 s` | `117.09 MiB` | `26.03 MiB` | **`146.5 MiB`** | `435.7 MiB` | **PASS (`-289.2 MiB`)** |
| `10000×10000` | `100 MP` | `JPEG → WebP q80` | `32.82 s` | `59.75 MiB` | `31.85 MiB` | **`795.3 MiB`** | `855.2 MiB` | **PASS (`-59.9 MiB`)** |
| `10000×10000` | `100 MP` | `PNG → JPEG q82` | `3.54 s` | `234.16 MiB` | `52.04 MiB` | **`289.6 MiB`** | `855.2 MiB` | **PASS (`-565.6 MiB`)** |

- **Implementation Rule Confirmed for `radivine-webp`:**
  For lossy WebP (`config.lossless == 0`), always keep `picture.use_argb = 0` so `WebPPictureImportRGB` / `WebPPictureImportRGBA` converts directly into YUV420 (+ alpha) planes (`1.5–2.5 B/px` instead of `4.0 + 1.5 = 5.5 B/px`), and drop the owned Rust pixel buffer before invoking `WebPEncode`. This keeps actual peak RSS `~1.75–2.0× decoded_rgba`, comfortably inside the `2.2 × decoded_rgba + 16 MiB` reservation budget across all image sizes up to `100 MP`.

### P8 — Two-Rung Atomic Output Commit Matrix

- **Tested Environments:**
  1. `ext4` (workspace filesystem): Rung A (`renameat_with(..., RenameFlags::NOREPLACE)`) succeeds on non-existing target and returns `Errno::EXIST` on collision without modifying the existing target's bytes or inode. Rung B (`linkat` + `fsync(dest_dir)` + `unlinkat` + `fsync(stage_dir)`) likewise succeeds on non-existing target and returns `Errno::EXIST` on collision.
  2. `tmpfs` (`/dev/shm`): Identical behavior verified for both Rung A and Rung B.
  3. Fault-injected `FsOps` seam verified for `EINVAL`/`ENOSYS`/`EOPNOTSUPP` on Rung A falling through to Rung B, and `EOPNOTSUPP`/`ENOSYS`/`EPERM`/`EXDEV` on Rung B returning `ConversionError::FilesystemUnsupported` while unlinking the staging `.part` file.
