# Radivine v0.2.0 — Image Conversion Backends

Status: **Proposed — pending owner review** · Prepared: 2026-10-24
Related: [CONVERSION_ARCHITECTURE](CONVERSION_ARCHITECTURE.md) · [CONVERSION_PRODUCT_SPEC](CONVERSION_PRODUCT_SPEC.md) · [SECURITY_AND_FILE_SAFETY](SECURITY_AND_FILE_SAFETY.md)

Evidence legend:
- **[SRC]**: verified by reading the crate source in the local Cargo registry (versions as resolved in `Cargo.lock`: `image 0.25.10`, `image-webp 0.1.3`, `png 0.17/0.18`, `zune-jpeg 0.5.15`, `moxcms 0.8`, `rustix 1.1.5`, `eframe 0.37.0`).
- **[WEB]**: from public documentation, changelogs, or advisories (URLs listed in §9).
- **[VERIFY]**: unverified. Must be confirmed by the C1 verification spikes before anything depends on it.

---

## 1. Requirements recap

Minimum: PNG, JPEG, and WebP decode **and** encode. The engine needs lossy and lossless WebP, a JPEG quality setting, transparency handling, EXIF orientation, ICC preservation, resizing, bounded memory, a small dependency footprint, Debian distribution, future Windows/macOS portability, and licences compatible with `MIT OR Apache-2.0`.

## 2. Candidates

### 2.1 Rust `image` crate (0.25.10, already in `Cargo.lock` via `eframe`)

| Capability | Finding | Evidence |
|------------|---------|----------|
| Formats | Default features: avif, bmp, dds, exr, ff, gif, hdr, ico, jpeg, png, pnm, qoi, tga, tiff, webp. Radivine would enable only `png`, `jpeg`, `webp` (`default-features = false`). | [SRC] `Cargo.toml` |
| PNG | Decode + encode, 8/16-bit, palette/tRNS. `PngDecoder::is_apng()` detects animation. | [SRC] `codecs/png.rs:160` |
| JPEG decode | `zune-jpeg` 0.5 (pure Rust). Progressive decode supported. **CMYK/YCCK/Adobe-inverted correctness [VERIFY]** | [SRC] CHANGES 0.25.0/0.25.9; [WEB] |
| JPEG encode | `JpegEncoder::new_with_quality(w, q: u8)`. Pure Rust. **Baseline only (progressive encode not found) [VERIFY]**. Size efficiency is below mozjpeg [WEB]. | [SRC] `codecs/jpeg/encoder.rs` |
| WebP decode | `image-webp` (pure Rust): lossy, lossless, alpha, animation. `WebPDecoder::has_animation()` | [SRC] `codecs/webp/decoder.rs:31` |
| **WebP encode** | **Lossless only.** The only constructor is `WebPEncoder::new_lossless`, and the doc comment says "Right now only lossless encoding is supported". The libwebp feature was **removed** in 0.25.0. There is no quality parameter. | [SRC] `codecs/webp/encoder.rs`, CHANGES 0.25.0 |
| ICC | `ImageDecoder::icc_profile()` for PNG/JPEG/WebP/TIFF/GIF/AVIF. `ImageEncoder::set_icc_profile()` for JPEG, PNG, WebP (lossless), TIFF. | [SRC] `jpeg/encoder.rs:752`, `png.rs:780`, `webp/encoder.rs:96` |
| EXIF | `exif_metadata()` for JPEG/PNG/WebP. `set_exif_metadata()` for JPEG, PNG, WebP (lossless encoder). | [SRC] `jpeg/encoder.rs:757`, `png.rs:785`, `webp/encoder.rs:101` |
| Orientation | `ImageDecoder::orientation()`, `DynamicImage::apply_orientation()`, `metadata::Orientation::{from_exif_chunk, remove_from_exif_chunk}` (resets the tag in place) | [SRC] `metadata.rs:84,96` |
| Limits | `image::Limits { max_image_width, max_image_height, max_alloc }`. Default: width/height `None`, `max_alloc = 512 MiB`. Applied via `ImageReader::limits()` → `set_limits` *before* allocation. | [SRC] `io/limits.rs:35-66` |
| Resize | `imageops::resize` with Nearest/Triangle/CatmullRom/Gaussian/Lanczos3. Scalar code. | [SRC] |
| Memory | Full decode into memory. Incremental decoding was removed in 0.25.0. No tiling. | [SRC] CHANGES 0.25.0 |
| Colour management | `moxcms` (pure Rust, BSD-3-Clause OR Apache-2.0) is already a dependency of image 0.25.10 (CICP awareness). Full ICC→sRGB transforms via moxcms are possible but not needed in v0.2.0. | [SRC] `Cargo.lock` |
| Licence | image, image-webp, png, fdeflate: MIT OR Apache-2.0. zune-jpeg: MIT OR Apache-2.0 OR Zlib. | [SRC] |
| Security | Historical RustSec advisories (2019/2020) affect only old versions. Nothing open against 0.25.x found. **Run `cargo audit` in CI [VERIFY]**. Memory-safe decoders: CVE-2023-4863 (libwebp) did not affect image-webp. | [WEB] |

**Limitation (explicit):** the `image` crate alone **cannot produce lossy WebP**. Quick Convert `JPEG → WebP` at a quality setting needs an additional component. Lossless WebP of a photograph is typically several times larger than the source JPEG, which would surprise users.

### 2.2 libwebp bindings

| Crate | Capability | Concerns | Evidence |
|-------|-----------|----------|----------|
| `webp` (jaredforth) 0.3.x | `Encoder::from_rgba/from_rgb(...).encode(q)` lossy, `encode_lossless()`, `encode_advanced(&WebPConfig)` | Sparse maintenance. 0.3.1 fixed **RUSTSEC-2024-0443** (soundness). No mux, so no ICC/EXIF embedding. | [WEB] |
| `libwebp-sys` | Raw bindgen FFI. Compiles **vendored libwebp C source via `cc`** and links statically by default. A system-library option may exist. | Needs a C compiler. The bundled libwebp version must be ≥ 1.3.2. | [WEB]; system option **[VERIFY]** |
| `webpx` | Newer safe wrapper: quality, presets, ICC/EXIF/XMP, cancellation, limits | Maturity and maintainer track record unknown | [WEB] **[VERIFY]** |
| `zenwebp` (pure-Rust lossy encoder) | — | **AGPL-3.0 / commercial: incompatible with MIT OR Apache-2.0. Rejected.** | [WEB] |

libwebp itself: BSD-3-Clause (notice must ship). Ubuntu 24.04 ships `libwebp7` 1.3.2, which includes the CVE-2023-4863 fix [WEB].

### 2.3 libvips

| Aspect | Finding |
|--------|---------|
| Capabilities | Demand-driven, streaming, threaded pipeline with very low memory for large images. `thumbnail` with shrink-on-load. ICC via lcms2. `autorot`. `webpsave` (Q, lossless, alpha_q, effort). `jpegsave` (Q, interlace, optimize_coding). `keep=` metadata control (added in 8.15) [WEB] |
| Ubuntu 24.04 | `libvips42t64` **8.15.1**, with a **large dependency tree** (libheif, libjxl, poppler-glib, librsvg, cairo/pango, fftw, matio, cfitsio, openexr, ImageMagick?) [WEB]. Measure install size [VERIFY] |
| Licence | LGPL-2.1+. Dynamic linking or out-of-process use is fine with notices. Static linking/vendoring adds relinking obligations. |
| Rust bindings | `libvips` (olxgroup-oss): sporadic maintenance. `rs-vips`: targets libvips **8.18**, newer than noble's 8.15, so it carries API-mismatch risk and needs `libvips-dev` at build time [WEB] |
| CLI | `vips copy in.jpg out.webp[Q=80,keep=icc]`, `vipsthumbnail` (package `libvips-tools`). Easy to sandbox as a subprocess. |
| Fit | Excellent as an **optional, out-of-process, large-image backend**. Poor as the essential built-in backend: heavy dependency closure, large parsing attack surface (dozens of C loaders), and binding instability. |

### 2.4 Other options considered

| Option | Verdict |
|--------|---------|
| `fast_image_resize` (MIT/Apache, runtime-dispatched SIMD; author reports ~10–40× faster Lanczos than `image`) [WEB] | **Deferred.** Resize is not on the Quick Convert default path (default = Original). Start with `image::imageops` behind an internal `Resizer` seam and adopt it in C5 only if the resize budget is missed. |
| `jpeg-encoder` (pure Rust, MIT/Apache; progressive, better Huffman) [WEB] | **Evaluate in the C1 spike** against `image`'s `JpegEncoder` (size at equal quality, speed, ICC/EXIF segment support). Swap behind the backend boundary if clearly better. |
| `mozjpeg` / `turbojpeg` | Rejected for v0.2.0: cmake + nasm + C build, larger supply chain. Possible future "high-efficiency JPEG" backend. |
| `zune-image` | Rejected: no WebP encoder; the `image` crate is more mature. |
| ImageMagick CLI | Rejected: very large attack surface, ImageTragick history, policy.xml dependence. |
| `cwebp`/`dwebp` CLI (`webp` package) | Not needed if libwebp is linked. Could be an emergency fallback external backend. |
| `kamadak-exif` (BSD-2) / `little_exif` (MIT/Apache) | Not needed: `image` 0.25.10 provides EXIF read/write and orientation reset. |
| `img-parts` (MIT/Apache) | **Candidate** for embedding ICC/EXIF into libwebp's *lossy* output (RIFF chunk editing, VP8X). Alternative: a ~200-line in-house RIFF/VP8X muxer with fuzz tests. Decide in the C1 spike. |
| `lcms2` (MIT, C) / `moxcms` (pure Rust) | Not needed in v0.2.0 (ICC is passed through, not transformed). `moxcms` is the preferred future path for "Convert to sRGB". |

## 3. Comparison matrix

| | `image` only | **`image` + libwebp (encode)** | libvips (bindings) | vips CLI (external) |
|---|---|---|---|---|
| PNG/JPEG/WebP decode | ✓ pure Rust | ✓ pure Rust | ✓ C | ✓ C (separate process) |
| PNG/JPEG encode | ✓ | ✓ | ✓ | ✓ |
| Lossy WebP | ✗ | ✓ q 0–100, method 0–6, alpha_q | ✓ | ✓ |
| Lossless WebP | ✓ | ✓ (image-webp or libwebp) | ✓ | ✓ |
| JPEG quality | ✓ u8 | ✓ u8 | ✓ | ✓ |
| Progressive JPEG | [VERIFY] ✗ | [VERIFY] ✗ (✓ with jpeg-encoder) | ✓ | ✓ |
| Alpha | ✓ | ✓ | ✓ | ✓ |
| ICC read/write | ✓ / ✓ (JPEG, PNG, WebP-lossless) | ✓ / ✓ (lossy WebP via mux [VERIFY]) | ✓ (+transform) | ✓ |
| EXIF + orientation | ✓ | ✓ | ✓ | ✓ |
| Resize | ✓ scalar | ✓ scalar (SIMD later) | ✓ fast | ✓ fast |
| Large images (>100 MP) | Full in-RAM | Full in-RAM | **Streaming** | **Streaming** |
| Native deps | none | vendored libwebp (C, cc) | libvips + ~30 libs | libvips-tools package |
| Untrusted-input parsing in C | no | **no** (libwebp only encodes our pixels) | yes | yes (isolated process) |
| Licence | MIT/Apache | + BSD-3 notice | LGPL-2.1 | LGPL (process boundary) |
| Debian | crate | crate | `libvips42t64` | `libvips-tools` (Suggests) |
| Windows/macOS | ✓ | ✓ | harder | needs install |

## 4. Recommendation [PROPOSED — ADR-C04]

**Built-in backend `builtin.image` (crate `radivine-convert-image`):**

1. **Decode**: always with the pure-Rust `image` crate (`png`, `jpeg` via zune-jpeg, `webp` via image-webp), with explicit `Limits` (width, height, `max_alloc`) set before decode. *All parsing of untrusted input stays in memory-safe Rust.*
2. **Encode PNG**: `image::codecs::png::PngEncoder` (compression Fast/Default/Best, ICC, EXIF).
3. **Encode JPEG**: `image::codecs::jpeg::JpegEncoder` (quality, ICC, EXIF). Optional swap to `jpeg-encoder` after the C1 spike if it is clearly better (progressive, size).
4. **Encode WebP lossless**: `image::codecs::webp::WebPEncoder::new_lossless` (pure Rust, ICC/EXIF supported).
5. **Encode WebP lossy**: libwebp through `libwebp-sys` behind a **minimal safe wrapper**. Two options:
   - The wrapper lives in a tiny dedicated crate, `radivine-webp-sys-safe` (the only place with `unsafe`, reviewed, fuzzed).
   - Or use `webp ≥ 0.3.1` / `webpx`, if the spike shows their API and maintenance are acceptable.
   
   Either way, `radivine-convert-image` keeps `#![forbid(unsafe_code)]`. Metadata (ICC/EXIF) for lossy WebP is added by RIFF muxing (`img-parts` or in-house) [VERIFY].
6. **Orientation**: `apply_orientation` (auto-orient default). When EXIF is preserved, `Orientation::remove_from_exif_chunk` normalizes the tag to 1.
7. **Resize**: `image::imageops::resize` with Lanczos3 (downscale), behind an internal `Resizer` seam.
8. **Animated sources** (APNG, animated WebP): rejected with `UnsupportedFeature(Animation)`.

**Why not libvips as the essential backend:** heavy dependency closure, large C parsing surface for untrusted files, binding/API-version mismatch risk on noble (8.15 vs bindings targeting 8.18), and LGPL obligations if static. It remains the natural **optional** high-performance backend for very large images, and the architecture supports it (`BackendKind::ExternalProgram`, routing by `max_pixels`).

## 5. libwebp linkage (owner decision D2)

| Option | Pros | Cons |
|--------|------|------|
| **A. Vendored static (recommended)** (`libwebp-sys` default build) | Self-contained `.deb` (no new `Depends`). Identical behaviour on Debian, CI, and future Windows/macOS. Build needs only `cc`. **Exposure is limited because Radivine only *encodes* with libwebp** (CVE-2023-4863 was a decoder bug). | Radivine must track libwebp security releases and bump the crate. ~0.5 MB binary growth [VERIFY]. Debian policy discourages embedded copies (applies to the official archive, not to this third-party package). |
| B. System shared `libwebp7` | Ubuntu security updates; smaller binary; Debian-idiomatic | Needs `libwebp-dev` + pkg-config at build. `Depends: libwebp7 (>= 1.3.2)` must be generated (the current `build-deb.sh` hardcodes `Depends`). Whether `libwebp-sys` supports system linking is **[VERIFY]**. Divergent builds per platform. |

Recommendation: **A** for v0.2.0, with:
- the libwebp version pinned and recorded in `THIRD_PARTY_NOTICES.md`;
- `cargo deny`/`cargo audit` in CI;
- a release checklist item: "check libwebp security advisories".

Revisit B if Radivine ever decodes with libwebp.

## 6. Quality semantics (do not compare across formats)

| Format / backend | Parameter | Meaning |
|------------------|-----------|---------|
| JPEG (`image` encoder) | `quality` 1–100 | libjpeg-style quantization table scaling. q=82 is a typical "web" setting. Chroma subsampling is chosen by the encoder [VERIFY which]. |
| WebP lossy (libwebp) | `quality` 0–100 (float) | libwebp's own rate-distortion control. **Not equivalent to JPEG q.** WebP q80 typically yields smaller files than JPEG q80 at similar perceived quality [WEB], but the mapping is content-dependent. |
| WebP lossy | `method` 0–6 | Speed/size trade-off (higher = slower, smaller). Default 4. |
| WebP lossy | `alpha_quality` 0–100 | Compression of the alpha plane (100 = lossless alpha). Default 100. |
| WebP lossless | `effort` 0–100 | Compression effort only. The output is pixel-exact. |
| PNG | compression Fast/Default/Best | Lossless. Size/speed only. |

The UI presents **intents** (Small / Balanced / High / Lossless) and shows the resolved per-format number in Advanced mode, labelled per format ("JPEG quality 82", "WebP quality 80"). It never shows a single cross-format "quality" slider.

## 7. Optional backends and discovery (ADR-C16)

- Framework in v0.2.0: `BackendKind::ExternalProgram(ProbeSpec)`, `probe_all` (fixed search paths `/usr/bin`, `/usr/local/bin`, no `$PATH` by default, `--version` with a 2 s timeout, minimum version), `Availability::MissingDependency { requirement, install_hint }`, and `TargetStatus::RequiresOptional`. The UI surfaces these states (CONVERSION_UX §7).
- **Recommended for v0.2.0 (owner decision D3): ship no optional backend.** The framework is tested with a fake descriptor (a test-only program stub). First real optional backend candidate: `external.vips` (`libvips-tools`, `Suggests:` in Debian) for images above the built-in pixel limit and for future AVIF/HEIF/TIFF targets, run out-of-process with `VIPS_BLOCK_UNTRUSTED=1` [VERIFY variable/version] and resource limits.
- Alternative for D3: implement `external.vips` in C5 as a stretch item (adds a test matrix and security review).

## 8. Migration and extension path

1. v0.2.0: `builtin.image` (image + libwebp encode).
2. v0.2.x: optional `external.vips` (large images, extra formats). `fast_image_resize` if needed. ICC → sRGB conversion via `moxcms` (`IccPolicy::ConvertToSrgb`).
3. Later: GIF/TIFF/AVIF via `image` features or vips; `radivine-convert-audio` (e.g. `symphonia` decode + encoder crates, or an `ffmpeg` external backend), each as a separate backend crate with its own descriptor. Adding a format = add a `FormatId` variant + descriptor + capabilities. No engine changes.

## 9. Dependency and licence risk assessment

| Dependency | Licence | Linkage | Obligation | Risk |
|------------|---------|---------|-----------|------|
| image, image-webp, png, fdeflate | MIT OR Apache-2.0 | static (Rust) | Notices | Low |
| zune-jpeg, zune-core | MIT OR Apache-2.0 OR Zlib | static | Notices | Low |
| moxcms | BSD-3-Clause OR Apache-2.0 | static (already present) | Notices | Low |
| libwebp (C, via libwebp-sys) | BSD-3-Clause | static (vendored) | **Ship BSD-3 notice** in `THIRD_PARTY_NOTICES.md` and `debian/copyright` | Medium: track CVEs |
| libwebp-sys / safe wrapper | MIT (or MIT/Apache) [VERIFY] | static | Notices | Medium (maintenance) |
| img-parts (if chosen) | MIT OR Apache-2.0 | static | Notices | Low–medium (slow maintenance) |
| rusqlite | MIT | static | Notices | Low |
| SQLite (bundled via libsqlite3-sys) | Public domain (libsqlite3-sys: MIT) | static | None (courtesy notice) | Low (database is a trusted, private file) |
| jpeg-encoder (if chosen) | MIT OR Apache-2.0 (+IJG acknowledgment [VERIFY]) | static | Notices | Low |
| libvips (future, optional) | LGPL-2.1+ | **separate process** (CLI) | Notice in docs. No linking. | Medium (attack surface, mitigated by process isolation) |

`deny.toml` already allows MIT, Apache-2.0, BSD-2/3, ISC, Zlib, MPL-2.0, Unicode, CC0. No new licence family is introduced except "public domain" (SQLite), which needs a `clarify` or exception entry if `cargo deny` flags `libsqlite3-sys` [VERIFY].

## 10. Proof-of-concept items (C1 verification gate)

| ID | Question | Pass criterion |
|----|----------|----------------|
| P1 | libwebp-sys: bundled libwebp version; clean build on noble + CI; build-time and size delta | ≥ 1.3.2 (prefer latest); builds with `cc` only |
| P2 | Lossy WebP + ICC + EXIF muxing (img-parts or in-house) round-trips | `exiftool`/`webpinfo` show ICCP/EXIF; Chrome/Firefox/GIMP/eog decode |
| P3 | `image::Limits` rejects 65535×65535 headers (PNG/JPEG/WebP) **before** allocation | RSS stays < 50 MB during rejection |
| P4 | zune-jpeg on CMYK/YCCK/Adobe-inverted, 12-bit, arithmetic-coded JPEGs | Correct colours or a clean `UnsupportedFeature` error. Never a panic. |
| P5 | ICC byte-exact round trip PNG→JPEG, JPEG→WebP (lossless and lossy) | Profile bytes identical |
| P6 | `image` JpegEncoder vs `jpeg-encoder`: size/speed at q 70/82/92, ICC/EXIF support | Decision recorded |
| P7 | Peak RSS and wall time: 12/24/50/100 MP JPEG→WebP q80 and PNG→JPEG | Calibrates the governor factor and budgets |
| P8 | `renameat_with(NOREPLACE)` (Rung A) and `linkat` + `unlinkat` (Rung B) on ext4, btrfs, tmpfs, vfat/exFAT USB, ntfs3, sshfs/gvfs FUSE, NFS (if available) | errno table recorded; Rung A → Rung B → `ConversionError::FilesystemUnsupported` (no copy fallback) validated. |
| P9 | `cargo deny check` + `cargo audit` on the planned dependency set | Clean or justified exceptions |
| P10 | systemd static user unit started from a process inside `radivine.service` survives `systemctl --user restart radivine.service` | Worker PID unchanged |

## 11. Sources

- image crate changelog and source: https://github.com/image-rs/image/blob/main/CHANGES.md · https://docs.rs/image/0.25.10
- image-webp (lossless-only encoder): https://github.com/image-rs/image-webp
- RUSTSEC-2024-0443 (webp crate): https://rustsec.org/advisories/RUSTSEC-2024-0443.html
- libwebp-sys: https://crates.io/crates/libwebp-sys · webp: https://crates.io/crates/webp · webpx: https://crates.io/crates/webpx
- zenwebp licence (AGPL): https://github.com/imazen/zenwebp
- CVE-2023-4863 / Ubuntu: https://ubuntu.com/security/CVE-2023-4863 · https://ubuntu.com/security/notices/USN-6369-1
- libvips API and licence: https://www.libvips.org/API/current/ · https://github.com/libvips/libvips/blob/master/LICENSE · noble package: https://packages.ubuntu.com/noble/libvips42t64
- rs-vips: https://crates.io/crates/rs-vips · libvips crate: https://crates.io/crates/libvips
- fast_image_resize: https://github.com/Cykooz/fast_image_resize
- img-parts: https://github.com/paolobarbolini/img-parts · little_exif: https://crates.io/crates/little_exif · kamadak-exif: https://github.com/kamadak/exif-rs
- renameat2 / O_TMPFILE: https://man7.org/linux/man-pages/man2/rename.2.html · https://man7.org/linux/man-pages/man2/open.2.html
- MoveFileExW: https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw
- ImageTragick: https://imagetragick.com
