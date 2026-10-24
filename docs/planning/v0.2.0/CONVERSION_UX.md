# Radivine v0.2.0 — Conversion User Experience

Status: **Proposed — pending owner review** · Prepared: 2026-10-24
Related: [CONVERSION_PRODUCT_SPEC](CONVERSION_PRODUCT_SPEC.md) · [CONVERSION_ARCHITECTURE §7](CONVERSION_ARCHITECTURE.md) · [JOB_SYSTEM_DESIGN](JOB_SYSTEM_DESIGN.md)

Wireframe-level requirements only. **No UI code is part of this planning task.** The approved Branching/Vine wheel (geometry, animations, navigation, visual identity) is **unchanged** [APPROVED 4.9]. Only new menu *content* is added.

---

## 1. Radial menu

### 1.1 Structure

```text
Root (image selection, e.g. photo.png)          Root (text selection) — unchanged from v0.1.1
  ├─ Convert ▸                                    ├─ File Preview ▸
  ├─ Copy                                         ├─ Copy
  ├─ Properties                                   ├─ Properties
  └─ Custom ▸                                     └─ Custom ▸
        │
        └─ Convert wheel:  [ Image ▸ ]
                               │
                               └─ Image wheel:  [ JPEG ]  [ WebP ]  [ Advanced… ]      (PNG hidden: source is PNG)
```

| Node | Label (segment) | Icon | Caption (description / disabled reason) |
|------|-----------------|------|------------------------------------------|
| `radivine.convert` | Convert | `lucide:arrow-right-left` (new SVG) | "Convert to another format" |
| `radivine.convert.image` | Image | `lucide:image` (exists) | "Image formats" |
| `…image.png` | PNG | `lucide:file-image` (new SVG) | "Convert photo.webp to PNG" / "Convert 12 images to PNG" / "Convert 2 of 3 images to PNG · 1 already PNG" / "Convert 3 of 5 files to PNG · 2 not supported" |
| `…image.jpeg` | JPEG | `lucide:file-image` | same pattern + " · transparency → white" when relevant (from extension only: PNG/WebP sources) |
| `…image.webp` | WebP | `lucide:file-image` | same pattern |
| `…image.advanced` | Advanced… | `lucide:sliders-horizontal` (new SVG) | "Choose quality, size and destination…" |

Labels are ≤ 9 characters (segment-safe at the default radii). Longer text lives in the caption (`WHEEL_ENGINE_DESIGN.md` §4.5: 240×44 lp caption box).

### 1.2 Rules

- **Layout from BasicContext only.** Which targets appear depends on file extensions (plus the static capability catalog). Enrichment never adds or removes segments (ADR-012).
- **No overcrowding.** The Image wheel has at most 4 items (3 targets + Advanced). The root gains one item for image selections. "More…" overflow is never triggered by conversion items under default limits.
- **Depth.** Leaves are at depth 3 (root=1). `max_depth = 4` is respected.
- **First frame.** No filesystem probing happens in `is_visible`, `description_for`, or `build_menu` (CONVERSION_ARCHITECTURE §7.2). Capability probing (future external backends) happens on a background thread at daemon start.
- **Disabled reasons** (live, from `FileFacts`): "Checking…", "Already WebP", "No convertible images", "No permission to read", "Needs libvips-tools" (future optional backend).
- **Mixed selections:** targets are shown when valid for at least one eligible item. The caption states how many files will be converted and how many are not supported, *before* the drop. On completion, skipped files are reported (never silently ignored).

## 2. Flows

### Flow 1 — Quick conversion of a single image

1. In Nemo, the user drags `photo.png`, presses **Shift**: the wheel appears at the cursor (unchanged behaviour).
2. Hover **Convert** → child wheel → hover **Image** → child wheel shows **JPEG · WebP · Advanced…**. The caption on WebP says "Convert photo.png to WebP".
3. Release on **WebP**. The wheel closes immediately (commit animation as today).
4. In the background, the job is submitted (the worker is launched on demand if needed), then converted with the saved preset (Balanced: WebP lossy q80).
5. `photo.webp` appears beside `photo.png`. The original is untouched.
6. Notification: **"Converted to WebP"** / "photo.webp saved beside the original."

Edge: if the job takes more than 2 s (large image), the Jobs page shows it as running. No notification is sent at start, to avoid noise.

### Flow 2 — Advanced conversion of a single image

1. Drag `photo.png` → Shift → Convert → Image → **Advanced…** → release.
2. The wheel closes. The **Advanced Convert** window opens (≈ 300–600 ms cold start [TARGET], see §5).
3. The window shows the source summary (header info only), the target format (WebP preselected if it was the last used), the preset, quality, resize, metadata, and destination.
4. The user adjusts (e.g. preset "Web (1920 px)", WebP quality 85), sees the destination preview `photo.webp → ~/Pictures/`, and clicks **Convert**.
5. The window shows "Started as job #43" with an **Open Jobs** link (which spawns `radivine-control --page jobs`) and closes after 1.5 s (unless "Keep window open" is ticked).
6. The completion notification arrives as in Flow 1.

### Flow 3 — Quick conversion of multiple images

1. The user drags 24 files (20 PNG, 3 JPEG, 1 `notes.txt`) → Shift → Convert → Image.
2. The Image wheel shows **PNG · JPEG · WebP · Advanced…**. The WebP caption says "Convert 23 of 24 files to WebP · 1 not supported".
3. Release on **WebP** → one job with 24 items.
4. Worker: `notes.txt` is skipped (unsupported). The 23 images are converted with up to `max_parallel` in parallel.
5. Notification: **"Converted 23 images to WebP"** / "Saved beside the originals. 1 skipped (not an image)."
6. The Jobs page lists job #44 with 23 succeeded and 1 skipped, and the skipped reason is visible in its details.

### Flow 4 — Advanced batch conversion

1. Select 300 photos → Shift → Convert → Image → **Advanced…**.
2. The window shows the summary: "300 files · 287 JPEG, 13 PNG · 1.8 GB · largest 6000×4000". The inspection runs in the background with a progress line, capped at 1,000 headers. Beyond the cap, it reports "+N not inspected yet" and the counts are by extension.
3. The user chooses JPEG target, quality High, resize Fit 2560×2560, EXIF Strip, destination folder `~/Pictures/export`, and suffix `-2560`.
4. Validation shows "287 files are already JPEG: they will be re-encoded because resize is set." The **Convert** button is enabled.
5. Start → job #45 (300 items). The window offers "Show progress" (spawns `radivine-control --page jobs`) and closes.

### Flow 5 — Unsupported source types

- **Only unsupported files** (`notes.txt`, `report.pdf`): **Convert does not appear** in the root wheel (no eligible extension).
- **Wrong extension** (`scan.jpg` that is really a GIF): Convert is visible (extension says JPEG). After enrichment, the targets show *disabled* with the caption reason **"No convertible images"** (sniffed GIF is unsupported). The existing pre-dispatch enablement re-check (`session.rs:1554`) prevents committing a disabled item. For files beyond the 64-item enrichment cap (no facts), the worker detects the real content and skips the file with "Not a supported image (detected: GIF)".
- **Animated WebP/APNG:** visible by extension. The worker skips it with "Animated images aren't supported yet". There is no frame dropping.

### Flow 6 — Missing optional backend dependency (framework; no optional backend ships in v0.2.0 unless D3)

1. Suppose a future target (e.g. AVIF via `external.vips`) is not installed.
2. The target segment is **visible but disabled**, with the caption "Needs libvips-tools". Visibility comes from the static catalog, so the layout is stable.
3. Control Center → Conversion → **Backends** lists "Large-image converter (vips): Not installed · Install: `sudo apt install libvips-tools`" with a **Check again** button (re-probe on a background thread). The daemon swaps its catalog snapshot on the next `reload`.
4. Settings option "Hide formats that need additional software" (default off).

### Flow 7 — Conversion failures

1. A 40-file batch: 2 files are corrupt and the destination for 5 others is read-only.
2. The job completes; the label is "Completed with 7 errors".
3. Notification: **"Converted 33 of 40 images"** / "7 failed. Open Radivine → Jobs for details."
4. Jobs page → job → **Details**: a failed-items list with per-file messages ("The image is damaged or incomplete." / "Can't write to /media/usb/photos (read-only).").
5. Actions:
   - **Retry failed** (same settings);
   - **Retry to another folder…** (prompts for a folder path and creates a new attempt with only `output.location` changed);
   - **Copy error details** (uses the egui clipboard, see §8).

### Flow 8 — Output collisions

1. `photo.webp` already exists beside `photo.png`; Quick Convert to WebP.
2. Output: `photo (1).webp`. The existing file is untouched.
3. Notification body: "Saved as photo (1).webp (name was taken)."
4. Jobs page details show the warning icon "Renamed: name was taken".

### Flow 9 — Cancelling a running conversion

1. Control Center → Jobs → running job #45 (120 of 300) → **Cancel**. No confirmation is needed because cancelling is non-destructive.
2. The state becomes **Cancelling…** immediately (target < 100 ms) with the tooltip "Finishing the current files". No new items start.
3. In-flight items stop at the next checkpoint. Their temp files are removed.
4. The state becomes **Cancelled**. The summary reads "120 converted (kept) · 180 not started".
5. Notification: "Conversion cancelled" / "120 of 300 images were converted and kept."
6. **Retry** on the cancelled job converts the remaining 180 (scope: not-succeeded items).

### Flow 10 — Retrying a failed conversion

1. Jobs page → a failed or interrupted job → **Retry failed**.
2. A new job #46 appears ("Retry of #45"), containing only the failed, interrupted, and cancelled items, using the original settings snapshot.
3. The original job #45 remains in history, linked ("Retried as #46").
4. **Retry all…** (in the overflow menu) warns: "Files that were already converted will be converted again as new copies."

## 3. Notifications

Text in CONVERSION_PRODUCT_SPEC §13. One notification per job. They use the existing pure-Rust D-Bus client with app name "Radivine" and the `radivine` icon. Notification actions are deferred.

## 4. Jobs page (Control Center)

Sidebar placement: **Dashboard · General · Wheel · Custom Actions · Conversion · Jobs · Diagnostics · About** (`ControlPage::{Conversion, Jobs}` added; `--page jobs`). A badge on "Jobs" shows the number of running jobs (leaf green) or failed unseen jobs (amber).

```text
┌ Jobs ─────────────────────────────────────────────────────────────── [ Max parallel: 2 ▾ ] ┐
│ Monitor background conversions. Jobs keep running when this window is closed.              │
│                                                                                             │
│ ACTIVE                                                                                      │
│ ┌──────────────────────────────────────────────────────────────────────────────────────┐   │
│ │ #45  Convert 300 images to JPEG                    Running · 120 of 300 · 1 failed      │  │
│ │ ███████████████░░░░░░░░░░░░░░  ~40%          Encoding IMG_2231.jpg        [ Cancel ]   │  │
│ └──────────────────────────────────────────────────────────────────────────────────────┘   │
│ ┌──────────────────────────────────────────────────────────────────────────────────────┐   │
│ │ #47  Convert photo.png to WebP                     Queued                  [ Cancel ]  │  │
│ └──────────────────────────────────────────────────────────────────────────────────────┘   │
│                                                                                             │
│ HISTORY        [All] [Completed] [Failed] [Cancelled] [Interrupted]      [ Clear history ]  │
│  ✓ #44  Convert 23 images to WebP     23 converted · 1 skipped   2 min ago   [Open folder] ⋯│
│  ⚠ #43  Convert 40 images to JPEG     33 converted · 7 failed    1 h ago     [Retry failed]⋯│
│  ⊘ #41  Convert 300 images to PNG     Cancelled · 120 kept       yesterday   [Retry]       ⋯│
│  ↻ #40  Convert 12 images to WebP     Interrupted (session ended) 2 d ago   [Retry]       ⋯│
│                                                                                             │
│ ▸ Details for #43   (expanded inline)                                                       │
│     Settings: JPEG · High (q 92) · Fit 2560 · EXIF stripped · → ~/Pictures/export           │
│     Failed (7):  IMG_1001.png — The image is damaged or incomplete.        [Show file]      │
│                  …                                                [Copy error details]      │
│     Skipped (0) · Warnings (3): Renamed (name was taken) ×2 · Transparency → white ×1       │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

Requirements:
- **Data source:** the worker over IPC (`list` + `subscribe`). The page restores the full history when reopened. It never blocks the egui thread (JOB_SYSTEM_DESIGN §10).
- **Worker offline:** opening the page launches it. If it cannot start, a banner says "Background job service unavailable: <reason>" with **Retry** and **Open Diagnostics**.
- **Progress bar:** input-bytes weighted, labelled approximate. Per-file activity is shown as stage text, with no fake percentage.
- **Open folder:** `org.freedesktop.FileManager1.ShowItems([output uri])` (selects the file in Nemo). Fallback: `xdg-open <dir>`. Runs on the CC worker thread.
- **Details:** paged (200 items per page) through `get`, failed first.
- **Clear history:** removes terminal jobs only, with a confirmation ("Remove 18 finished jobs from history? Converted files are not affected.").
- **Concurrency preference:** "Max parallel" dropdown (Auto, 1–8). It writes `config.toml [jobs] max_parallel` through the existing `toml_edit` persistence, then sends `reload_settings`.
- **Overflow menu (⋯):** Retry all…, Copy job summary, Remove from history, "Inspect leftover staging files" (safe journal-verified cleanup via `cleanup_temp_safely` and inspection of any `OrphanedTempPreserved` entries — never a blind wildcard directory scan; see [`SECURITY_AND_FILE_SAFETY.md`](SECURITY_AND_FILE_SAFETY.md) §3.6 and [`JOB_SYSTEM_DESIGN.md`](JOB_SYSTEM_DESIGN.md) §8).
- **Visual design:** existing `BrandPalette` (vine green `#044C3C`, leaf green `#00A87C`, dark canvas `#0A1A16`, light canvas `#F4FBF8`), existing `section_card`/`kv_cell` helpers, Inter font. Status colours: running = leaf green, queued = muted, completed = green check, completed-with-errors = amber, failed = existing `err` colour, cancelled/interrupted = grey. Logos and branding assets are **unchanged**. A dark and a light screenshot are added to `--export-screenshots`.

## 5. Advanced Convert window

Process: `radivine-control convert` (separate process that explicitly **bypasses `ControlInstanceLock::acquire_or_notify_existing()`** in `apps/radivine-control/src/single_instance.rs:39-42` so multiple Advanced Convert windows can run concurrently alongside an open Control Center Settings/Jobs window; it reuses `BrandPalette`, fonts, and widgets). Launched by `AdvancedConvertAction` in a transient user cgroup scope with the item list on stdin (`--stdin`, bounded to `≤ 10,000` lines, `≤ 16 KiB` per line, and `≤ 16 MiB` total stdin bytes), or from a terminal with paths as arguments.

```text
┌ Radivine — Convert images ───────────────────────────────────────────────────────┐
│ SOURCE                                                                             │
│  300 files · 287 JPEG · 13 PNG · 1.8 GB · largest 6000×4000                         │
│  ⚠ 2 files not supported (notes.txt, clip.gif)                         [ Show ▾ ]   │
│  ⓘ 13 files have transparency                                                       │
│                                                                                     │
│ CONVERT TO   ( PNG )  (• JPEG )  ( WebP )                                           │
│ PRESET       [ Web (1920 px)                    ▾ ]   [ Save as preset… ]           │
│ QUALITY      ( Small ) (• Balanced ) ( High ) ( Lossless — n/a for JPEG )           │
│              JPEG quality  [ 82 ]  ▭▭▭▭▭▭▭▭●▭▭                                       │
│ SIZE         ( Original ) (• Fit within ) ( Width ) ( Height )                       │
│              [ 1920 ] × [ 1920 ] px   ☐ Allow enlarging smaller images               │
│ BACKGROUND   [■ #FFFFFF ]  (White) (Black) (Grey)   — used for 13 transparent files │
│ METADATA     ☑ Rotate by EXIF orientation   ☑ Keep colour profile                   │
│              ☐ Keep camera & location data (EXIF)                                    │
│ SAVE TO      (• Same folder as each file ) ( Folder: [ ~/Pictures/export     ] )     │
│              Name: photo.jpg  · suffix [      ]   · existing names get " (1)"        │
│                                                                                     │
│ ✓ Ready: 298 files will be converted.                                               │
│                                         [ Cancel ]                [ Convert 298 ]   │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

Requirements:
- **No full image loads.** Source inspection uses `ConversionEngine::inspect` (header parse: dimensions, alpha, ICC presence, orientation) on a background thread. At most 1,000 items are inspected eagerly; the rest are summarized by extension. No thumbnails in v0.2.0 (a deliberate choice: thumbnails would require decoding untrusted images in the UI process).
- **Live validation:** `ConversionOptions::validate()` (pure) runs on every change. Errors are shown inline, next to the field. Routing checks show "Needs libvips-tools" style messages. The Convert button is disabled while errors exist.
- **Destination checks** (exists, is a directory, writable, free space) run on the background thread, debounced 300 ms.
- **Estimated output size: not shown** (not realistically computable without encoding; brief §10).
- **Folder entry:** a text field with validation plus quick buttons (Same folder, Pictures, Home, Recent). A native folder picker is deferred: `rfd`'s portal backend pulls in zbus + an async runtime, and its GTK backend conflicts with winit's loop (precedent in CONTROL_CENTER_DESIGN §5). Owner may revisit.
- **Save as preset…** writes a `[[preset]]` to `convert.toml` through `toml_edit` atomic persistence.
- **Start:** builds a `JobSpec` with `PresetRef::Inline(options)` and `origin = Advanced`, then submits through `JobsClient` on a background thread. On failure it shows an error banner and stays open.
- **Keyboard:** Enter = Convert (when valid), Esc = Cancel. Tab order follows the visual order.
- **Window:** 640×720 logical px, resizable. Title "Radivine — Convert images". Uses the theme from `config.toml [appearance] theme`.

## 6. Conversion settings page (Control Center)

Sections (cards):
1. **Quick Convert defaults:** preset per target (PNG/JPEG/WebP dropdowns). Default: Balanced.
2. **Output:** location (beside source / folder), suffix, JPEG extension (`.jpg`/`.jpeg`), background colour, "Keep original modification time".
3. **Metadata:** auto-orient, keep colour profile, keep EXIF (with a privacy note: "Includes camera details and GPS location if present").
4. **Presets:** list (built-ins read-only, user presets editable/deletable), editor form.
5. **Limits:** max pixels (MP), max file size (MB) with clamped ranges.
6. **Backends:** table of backends with their availability (built-in ✓ version; optional ones with install hints), "Check again" button, preference order (future).
7. **Notifications:** Always / Failures only / Never (`config.toml [jobs] notify`).

Save uses the existing patterns (validation, conflict detection, `.bak` overwrite option). The worker picks up changes through `reload_settings` (sent if the worker is running; otherwise read at its next start). The wheel daemon never reads `convert.toml` and needs **no reload** for any conversion setting or preset change.

## 7. Error and empty states

| Situation | Where | Message |
|-----------|-------|---------|
| Worker failed to start (submit) | Notification | "Action failed (radivine.convert.image.webp)" / "Couldn't start the conversion service: <reason>" |
| Worker failed to start (Jobs page) | Banner | "Background job service unavailable: <reason>" [Retry] [Open Diagnostics] |
| No jobs yet | Jobs page | Illustration-free empty state: "No conversions yet. Drag images in Nemo, press Shift, and choose Convert." |
| History DB from a newer version | Jobs page | "Job history was created by a newer Radivine version and is read-only." |

## 8. Interaction with GitHub issue #1 (clipboard in Custom Actions fields)

The Advanced Convert window (folder path, suffix, colour hex, preset name) and the Jobs page ("Copy error details" → `ctx.copy_text`) rely on the **same egui/eframe clipboard integration** as the Custom Actions editor. Investigation note (not a fix): the workspace enables `eframe` with `default-features = false` and `["default_fonts","glow","x11","wayland"]`, which omits eframe 0.37's `clipboard` feature (default upstream). This is a likely cause of issue #1. **Issue #1 stays a separate fix with its own commit and verification.** It is recommended (owner decision) before or alongside C4 so that the new text fields do not ship with the same defect. It is not bundled into conversion commits.

## 9. Accessibility and consistency

- Wheel: unchanged keyboard navigation in Invoke mode (digits, arrows, Enter). Convert items behave like any other submenu or leaf.
- Control Center pages: every control is reachable by keyboard (egui focus order). Colour is never the only status signal (icons + text: ✓ ⚠ ⊘ ↻).
- Wording: short and literal. Use "Convert" (not "Transcode") and "Saved beside the original".
