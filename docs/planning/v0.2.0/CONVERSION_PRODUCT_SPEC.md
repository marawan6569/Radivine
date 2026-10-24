# Radivine v0.2.0 — Conversion Product Specification

Status: **Proposed — pending owner review** · Prepared: 2026-10-24
Related: [CONVERSION_UX](CONVERSION_UX.md) · [CONVERSION_ARCHITECTURE](CONVERSION_ARCHITECTURE.md) · [CONVERSION_BACKENDS](CONVERSION_BACKENDS.md)

---

## 1. Scope

### 1.1 In scope for v0.2.0

| Area | Scope |
|------|-------|
| Formats | PNG, JPEG, WebP as both source and target |
| Workflows | **Quick Convert** (wheel → target → start immediately with saved defaults) and **Advanced Convert** (wheel → "Advanced…" → settings window → explicit Start) [APPROVED 4.5] |
| Batch | Any number of selected files up to the context cap (10,000 items, `MAX_CONTEXT_ITEMS`), each with an individual result [APPROVED 4.6] |
| Options | Quality intent and per-format quality, resize (original / width / height / fit within, no upscale by default), transparency background, metadata policy, destination, filename suffix |
| Presets | Built-in presets plus user presets (`convert.toml`) |
| Jobs | Durable background jobs with progress, cancel, retry, history (JOB_SYSTEM_DESIGN) |
| UI | Wheel actions, Advanced Convert window, Control Center **Jobs** page and **Conversion** settings page, desktop notifications |

### 1.2 Out of scope (v0.2.0)

Audio, video, and document conversion; GIF/TIFF/AVIF/HEIC/JXL; animation conversion; cropping, rotation, and filters beyond auto-orient and resize; overwrite or replace-original modes; an output size estimate; a native folder-picker dialog (CONVERSION_UX §5.4); cloud or network sources; an optional external backend (owner decision D3).

## 2. Conversion matrix

| From \ To | PNG | JPEG | WebP |
|-----------|-----|------|------|
| **PNG** | Advanced only (resize/recompress) | ✓ Quick (alpha → flatten) | ✓ Quick (lossy or lossless) |
| **JPEG** | ✓ Quick (lossless container, larger file) | Advanced only (resize/recompress) | ✓ Quick |
| **WebP** (still) | ✓ Quick | ✓ Quick (alpha → flatten) | Advanced only (resize/recompress) |
| **Animated WebP / APNG** | ✗ "Animated images aren't supported yet" | ✗ | ✗ |

Same-format conversion is **hidden from Quick Convert** (`SameFormatPolicy::Skip`: items whose detected content format already matches `target` are skipped with `AlreadyTargetFormat`). It is available in Advanced Convert (`SameFormatPolicy::Convert`, writing a new numbered file `stem (1).ext`) when a meaningful operation is selected: resize, a different quality, or metadata stripping. Advanced validation warns: "Output format is the same as the source. Choose a size or quality change, or this will only re-encode the image."

## 3. Quick Convert

1. The user drops onto a target (e.g. `Convert ▸ Image ▸ WebP`).
2. The wheel closes immediately (existing commit behaviour).
3. A job is submitted with `PresetRef::QuickDefault`. The wheel daemon never reads `convert.toml`; `radivine-worker` is the sole authoritative runtime resolver of `convert.toml [quick].<target>` (default preset `balanced`) at *Preparing* time and snapshots the resolved `ConversionOptions` (with `SameFormatPolicy::Skip`) into `jobs.resolved`. Consequently, saving Quick Convert defaults in the Control Center takes effect on the very next Quick Convert without a daemon reload, while already-submitted or retried jobs always use their snapshotted `jobs.resolved` options.
4. Outputs are written beside each source (default) with the target extension, never overwriting.
5. A completion notification is shown (configurable).

## 4. Advanced Convert

Opened from `Convert ▸ Image ▸ Advanced…`. A separate window (`radivine-control convert`) shows:
- the source summary (count, formats, total size, largest dimensions, skipped/unsupported files with reasons);
- the target format, preset, quality (intent + per-format number), resize, background colour (only when needed), metadata policy, destination, and filename suffix;
- a destination preview and validation messages.

The user starts the job explicitly. The window never decodes full images; it reads headers only (CONVERSION_UX §5).

## 5. Quality

### 5.1 Intents and initial mapping (provisional, calibrated in C1)

| Intent | JPEG | WebP | PNG |
|--------|------|------|-----|
| **Small** | q 70, 4:2:0 | lossy q 70, method 4 | compression Best |
| **Balanced** (default) | q 82, 4:2:0 | lossy q 80, method 4 | compression Default |
| **High** | q 92, 4:4:4 if supported [VERIFY] | lossy q 90, method 5 | compression Default |
| **Lossless** | ✗ invalid ("JPEG has no lossless mode") | lossless, effort 60 | compression Default |

- Values are **per format and not comparable** across formats (CONVERSION_BACKENDS §6). Advanced mode shows the resolved number with the format name.
- The C1 calibration uses the benchmark corpus to confirm that each intent produces sensible sizes. It records file-size ratios and, if available, SSIMULACRA2/butteraugli scores (tooling optional, results informational).
- WebP alpha: `alpha_quality = 100` (lossless alpha plane) by default.

### 5.2 Explicit overrides

Advanced mode can set an explicit number (JPEG 1–100; WebP 0–100 + lossless toggle + method 0–6; PNG compression). Presets may store per-format overrides.

## 6. Transparency

| Source | Target | Behaviour |
|--------|--------|-----------|
| Has alpha (PNG RGBA / palette+tRNS, WebP alpha) | PNG, WebP | Alpha preserved |
| Has alpha | JPEG | **Flatten** onto the background colour (default `#FFFFFF`, configurable globally and per job). Blending happens in sRGB space: `out = α·c + (1−α)·bg`. |
| Alpha channel present but fully opaque | JPEG | Alpha dropped, no warning |

The item warning `AlphaFlattened{background}` is recorded only when at least one pixel had α < 255. Advanced mode shows the background picker only when the target lacks alpha and at least one inspected source reports alpha. A hex text field with a colour swatch plus presets (White, Black, Light grey) is shown.

## 7. Resizing

| Mode | Semantics |
|------|-----------|
| Original (default) | No resampling |
| Width W | Height = round(H·W/w). Aspect preserved. |
| Height H | Width derived |
| Fit within W×H ("maximum dimensions") | Scale = min(W/w, H/h). Aspect preserved. |

- **No upscaling by default** (`allow_upscale = false`). An image already smaller than the requested size is kept at its original size and gets the warning "Kept original size (already smaller)".
- Filter: Lanczos3 for downscaling (Triangle when the scale factor ≥ 0.5 [VERIFY quality/speed trade-off in C1]).
- Exact W×H with distortion or cropping is **not offered** in v0.2.0.
- Limits: 1 ≤ W,H ≤ 16383 for WebP targets, 65500 for JPEG, and the global `max_dimension` for PNG.

## 8. Metadata policy

| Metadata | Default | Options | Guarantees |
|----------|---------|---------|------------|
| **Orientation** (EXIF 0x0112) | **Auto-orient**: pixels rotated/flipped; output carries no orientation or orientation = 1 | Keep tag (pixels untouched, tag copied only if EXIF is preserved) | Viewers that ignore EXIF still display correctly |
| **ICC profile** | **Preserve**: source profile embedded byte-for-byte in the output | Strip | PNG/JPEG/WebP-lossless via `image` [SRC]. Lossy WebP via RIFF mux [VERIFY P2]. If the chosen backend cannot embed it, the result carries `IccNotPreserved` (shown in the Jobs details). **No silent promise.** |
| **EXIF** (camera, GPS, serials, timestamps) | **Strip** (privacy-first) | Preserve: the raw EXIF block is copied with orientation normalized to 1 (`Orientation::remove_from_exif_chunk`) | GPS is never "partially" stripped. It is all-or-nothing in v0.2.0. Selective EXIF editing is future work. |
| XMP / IPTC | Dropped | — | Documented limitation |
| File timestamps | Output mtime = conversion time | `preserve_mtime = true` copies the source mtime/atime | Source timestamps are never modified |
| Permissions | Output mode = source mode & 0o666 (exec bits dropped), further restricted by the umask | — | A private (0600) source never yields a world-readable output |
| Ownership/xattrs/ACLs | Not copied | — | — |

Owner decision D5 confirms the privacy default (Strip EXIF).

## 9. Configuration schema

### 9.1 `convert.toml` (new, `$XDG_CONFIG_HOME/radivine/convert.toml`)

```toml
version = 1

[defaults]
location = "beside-source"      # beside-source | folder
folder = ""                     # absolute path; used when location = "folder"
suffix = ""                     # appended to the stem, e.g. "-web" → photo-web.webp
jpeg_extension = "jpg"          # jpg | jpeg
background = "#FFFFFF"          # flatten colour for targets without alpha
auto_orient = true
icc = "preserve"                # preserve | strip
exif = "strip"                  # strip | preserve
preserve_mtime = false

[quick]                         # preset used by Quick Convert per target
png = "balanced"
jpeg = "balanced"
webp = "balanced"

[limits]
max_pixels = 100_000_000        # 100 MP. Hard cap 500 MP.
max_dimension = 32768
max_file_mb = 512               # source file size cap

[backends]
prefer = []                     # e.g. ["external.vips"] (future)
disable = []

[[preset]]                      # user presets; built-ins (small, balanced, high, lossless, web-1920) are implicit and read-only
id = "web-1920"
label = "Web (1920 px)"
quality = "balanced"            # small | balanced | high | lossless
resize = { mode = "fit", width = 1920, height = 1920, upscale = false }
# optional explicit overrides:
# jpeg_quality = 85
# webp_quality = 82
# webp_lossless = false
```

### 9.2 `config.toml [jobs]` (new section, generic worker settings)

```toml
[jobs]
max_parallel = 0                # 0 = auto: clamp(available_parallelism / 2, 1, 4)
memory_budget_mb = 0            # 0 = auto: clamp(25% of MemTotal, 256, 1024)
idle_exit_s = 60                # worker exits after this long with no work and no subscribers
history_days = 30               # prune terminal jobs older than this
history_max_jobs = 500
notify = "always"               # always | failures | never
```

### 9.3 Validation, defaults, migration

- Every key is optional. A missing file means defaults. **Invalid config never prevents startup** (ADR-018 philosophy): invalid values reset to defaults with logged warnings (`ConvertSettings::from_toml_str`). The Control Center uses `validate_strict` before saving and writes with the existing `toml_edit` atomic persistence (preserves comments and unknown keys, detects external modification).
- `version = 1`. Future migrations are pure functions `v1 → v2` applied in memory. The file is rewritten only when the user saves through the Control Center. Unknown keys produce warnings and are preserved.
- **Backwards compatibility:** v0.1.1 `config.toml` and `actions.toml` work unchanged. The v0.1.1 daemon ignores `[jobs]` (no `deny_unknown_fields`). Downgrading to v0.1.1 leaves `convert.toml` and `jobs.sqlite3` unused and harmless.
- Preset ids: `[a-z0-9-]{1,32}`. Built-in ids are reserved. A user preset with a built-in id is rejected with a warning.

### 9.4 Authoritative Resource & Safety Limits Table

This table is the single source of truth for all resource and safety bounds across `convert.toml [limits]`, `config.toml [jobs]`, `ConversionLimits`, `ResourceGovernor`, and `image::Limits` ([`SECURITY_AND_FILE_SAFETY.md`](SECURITY_AND_FILE_SAFETY.md) `T3`, [`JOB_SYSTEM_DESIGN.md`](JOB_SYSTEM_DESIGN.md) §7.3):

| Limit / Setting | Config Key | Default | User-Configurable Range | Backend / Format Limit | Hard Safety Ceiling |
|-----------------|------------|---------|-------------------------|------------------------|---------------------|
| Max source pixels | `convert.toml [limits] max_pixels` | `100_000_000` (`100 MP`) | `1_000_000 ..= 500_000_000` (`1..=500 MP`) | `builtin.image` default `100 MP` (up to `500 MP` when user-configured) | `500_000_000` (`500 MP`) |
| Max source/output dimension | `convert.toml [limits] max_dimension` | `32,768` px | `64 ..= 65,500` px | WebP output: `16,383` px (`FormatTraits::max_dimension`); JPEG output: `65,500` px | `65,500` px |
| Max source file size | `convert.toml [limits] max_file_mb` | `512 MiB` | `1 ..= 2,048 MiB` | — | `2,048 MiB` |
| Worker concurrent memory budget | `config.toml [jobs] memory_budget_mb` | `0` (Auto: `clamp(25% MemTotal, 256 MiB, 1024 MiB)`) | `0` (Auto) or `256 ..= 8,192 MiB` | — | `8,192 MiB` |
| Per-item exclusive allocation limit | Derived (`ResourceGovernor` + `image::Limits.max_alloc`) | `estimate(100 MP)` (`≈ 900 MiB`) | Scales with `max_pixels` (`estimate = w·h·bpp_decoded × 2.2 + 16 MiB`) | Items with `estimate > memory_budget_mb` and `pixels ≤ max_pixels` run **exclusively** (`in_use == 0`, FIFO queue) with `image::Limits.max_alloc = Some(estimate.memory_bytes)` | `estimate(500 MP)` (`≈ 4.4 GiB`) |

## 10. Batch behaviour

- Each selected file becomes a **job item** with its own state and result. One job represents one user request.
- Failure isolation: an item failure never rolls back committed outputs of other items. The job continues with the next item.
- **Fast-fail for systemic errors:** after 3 consecutive `NoSpace` or `DestinationNotWritable` failures targeting the same destination directory, the remaining items for that directory are failed with the same reason without being attempted. The user sees one clear cause, not hundreds of identical errors.
- Skips (unsupported, already target format, not a regular file, animated) are reported per item.
- Ordering: items are processed in selection order. Parallelism across items is up to `max_parallel`.
- Two sources mapping to the same output name (`photo.png`, `photo.jpg` → WebP) get `photo.webp` and `photo (1).webp`. This is recorded as a `RenamedDueToCollision` warning.
- Limits per job: 10,000 items (same as the context cap). Maximum queued jobs: 256. Maximum total pending items: 50,000. Submissions beyond the limits are rejected with a clear message.

## 11. Output naming and collisions

- Name: `{stem}{suffix}.{ext}`, where `stem` is the source file name without its last extension (raw OS bytes preserved) and `ext` is the target's extension (`jpg` by default for JPEG).
- Collision policy (only policy in v0.2.0): **append a number**, `photo (1).webp`, `photo (2).webp`, … up to 9999, then `CollisionExhausted`. Collision checks are race-free: every candidate is committed with an atomic no-replace primitive, never with check-then-rename (SECURITY_AND_FILE_SAFETY §3).
- **No silent overwrites. No overwrite option in v0.2.0.**
- Destination: beside the source (default), or a fixed folder chosen in settings or in the Advanced window. If the source directory is read-only, the item fails with "Can't write to this folder". There is no silent relocation. Retry with another destination is offered (CONVERSION_UX flow 7).

## 12. Error behaviour (user-visible)

| Condition | Detection | Item result (message) | Retryable |
|-----------|-----------|----------------------|-----------|
| Corrupted / truncated file | Decoder error | Failed: "The image is damaged or incomplete." | No (Retry anyway allowed) |
| Unsupported format (content) | Sniff / decoder | Skipped: "Not a supported image (detected: GIF)" | No |
| Extension mismatch | Sniff ≠ extension | Converted with warning "File extension doesn't match its content" | — |
| Malformed dimensions (0, overflow) | Header | Failed: "The image has invalid dimensions." | No |
| Extremely large image | Header vs `limits` | Failed: "Image is too large (24000×24000, 576 MP; limit 100 MP)." | No (raise the limit in settings) |
| Decompression bomb | `image::Limits` (`max_alloc = Some(estimate.memory_bytes)`) before allocation + governor | Failed as above, or "Image needs more memory than allowed" | No |
| Insufficient disk space | `fstatvfs` precheck (reserve `max(16 MiB, 2 × source size)`) / `ENOSPC` | Failed: "Not enough disk space in <folder>." (+ fast-fail) | Yes |
| Permission denied (source) | `open` EACCES | Failed: "No permission to read this file." | Yes |
| Read-only destination | `EROFS` / `EACCES` on staging/temp create | Failed: "Can't write to <folder> (read-only)." | Yes (another destination) |
| Destination filesystem unsupported | Neither `RENAME_NOREPLACE`/`RENAME_EXCL` (Rung A) nor `linkat` (Rung B) is supported by the destination mount | Failed: "This folder's filesystem doesn't support safe atomic file creation." (`filesystem_unsupported`) | No (choose another destination folder) |
| Output collision | `EEXIST` on commit | Auto-numbered (warning). Exhausted → Failed | — |
| Interrupted write (crash/power loss) | Recovery journal (`C0`–`C4`) | Interrupted: "Stopped before finishing." Verified by `(temp_dev, temp_ino)`; no partial final output left | Yes |
| Source changed during conversion | `fstat` identity before/after | Failed: "The file changed while it was being converted." | Yes |
| Cancellation | User | Cancelled. Committed outputs are kept. | Yes |
| Partial batch failure | Per item | Job Completed with N errors (UI label) | Failed items |
| Decoder panic / internal bug | `catch_unwind` | Failed: "Internal error while converting (see log)." | Yes |
| Per-item timeout (hung decoder / I/O) | Watchdog + Commit Fence (`JOB_SYSTEM_DESIGN` §7.5) | Pre-commit timeout: Failed ("Took too long."). In-commit timeout: reconciled by `(temp_dev, temp_ino)` after bounded ≤ 5 s worker drain and restart (`exit(75)`). | Yes |

## 13. Notifications

| Event | Summary | Body example |
|-------|---------|--------------|
| Single item success | "Converted to WebP" | "photo.webp saved beside the original." / "Saved as photo (1).webp (name was taken)." |
| Batch success | "Converted 12 images to WebP" | "Saved beside the originals. 2 skipped (already WebP)." |
| Partial failure | "Converted 10 of 12 images" | "2 failed. Open Radivine → Jobs for details." |
| Total failure | "Couldn't convert photo.png" | "The image is damaged or incomplete." |
| Cancelled | "Conversion cancelled" | "4 of 12 images were converted and kept." |
| Submission failure (daemon side) | "Action failed (radivine.convert.image.webp)" | "Couldn't start the conversion service: …" |

One notification per job (no per-item spam). Governed by `[jobs] notify` and the existing `[notifications] enabled` master switch (the worker reads that key as well). Notification action buttons ("Open folder", "Show in Jobs") are deferred.
