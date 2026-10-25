# Radivine v0.2.0 — Image Conversion & Background Jobs

**Branch into action.**  
*A fast, extensible, context-aware radial action wheel and desktop utility suite for Linux.*

---

## Highlights of v0.2.0

`v0.2.0` introduces a complete, safety-first **Image Conversion Subsystem** (`PNG`, `JPEG`, `WebP`), an on-demand **Durable Background Job Worker** (`radivine-worker`), new **Conversion** and **Jobs** pages in the **Radivine Control Center**, a standalone **Advanced Convert** desktop window (`radivine-control convert`), native desktop clipboard interoperability, and Arabic/non-Latin system-font fallback:

### 1. Radial Wheel Quick Convert (`Convert ▸ Image`)
- Select one or more images (`PNG`, `JPEG`, `WebP`) in Nemo via `Drag + Shift` or `Open in Radivine` (`radivine invoke`) and branch into **`Convert ▸ Image ▸ PNG / JPEG / WebP / Advanced…`**.
- **One-Gesture Quick Convert:** Converts using your per-format default preset (`web-optimized` for WebP, `balanced` for JPEG, `lossless` for PNG). Submits a durable job to `radivine-worker` in milliseconds so the radial wheel closes immediately without blocking your desktop.
- **Smart Selection Handling:** Hides redundant same-format Quick Convert targets on single-item selections, summarizes skipped or unsupported files in mixed selections directly in the wheel caption, and supports large batches up to `10,000` items.

### 2. Standalone Advanced Convert Window (`radivine-control convert`)
- Choose **`Convert ▸ Image ▸ Advanced…`** from the radial wheel (or run `radivine-control convert [FILES…]`) to open a dedicated `640×720` conversion dialog that runs independently of the main Control Center window.
- **Presets & Explicit Codec Controls:** Choose from 5 built-in presets (`Web Optimized`, `Balanced`, `High Quality`, `Lossless`, `Compact Share`) or custom presets from `~/.config/radivine/convert.toml`, or fine-tune format-specific controls:
  - **WebP:** Lossy (`quality 1..100`, `method 0..6`, `sharp_yuv`) or Lossless.
  - **JPEG:** `quality 1..100`, chroma subsampling (`4:2:0`, `4:2:2`, `4:4:4`), baseline/progressive, and configurable alpha-flattening background color (`#RRGGBB`).
  - **PNG:** Lossless compression level (`Fast`, `Default`, `Best`) and adaptive filter strategy.
- **Resize, Output & Metadata Policies:** Configure aspect-preserving Lanczos3 resize (`Original`, `Width`, `Height`, `Fit` with optional `allow_upscale`), output directory (`Beside original` or custom folder), filename suffix, collision policy (`Numbered suffix`, `Skip existing`, `Fail item`), mtime preservation, and EXIF/ICC/orientation handling.
- **Header-Only Inspection:** Background `inspect_file_header` probes image dimensions, color type, alpha, EXIF orientation, and ICC profiles in milliseconds without decoding full pixel buffers in the UI process.

### 3. Built-In Image Codec Engine & Crash-Safe File Handling
- **Zero External CLI Tools Required:** Built-in pure-Rust PNG (`png` `0.18`), JPEG (`zune-jpeg` `0.5` + SIMD `jpeg-encoder` `0.7`), and lossless WebP (`image-webp` `0.2`) paired with SIMD (`SSE4.1`/`AVX2`) + multi-threaded lossy WebP (`libwebp` `1.5.0` isolated behind a memory-governed wrapper in `ffi/radivine-webp`).
- **Color, Orientation & Metadata Fidelity:** Supports 8-bit/16-bit/grayscale/CMYK inputs, EXIF orientation `1..=8` auto-rotation with in-place tag-`0x0112` reset, ICC color profile preservation across PNG/JPEG/WebP (`VP8X` + `ICCP` RIFF muxing), and **privacy-by-default EXIF stripping** (optional EXIF preservation with an explicit GPS privacy warning banner).
- **Sacred Source & Zero-Overwrite Guarantees (`ADR-040`, `ADR-041`):**
  - Source files are opened strictly `O_RDONLY | O_NOFOLLOW | O_NONBLOCK` and verified via `fstat`—never modified, truncated, unlinked, or moved.
  - Outputs are written into a private `0700` `.rvstage-<uid>` staging directory as `.part` files, `fsync`ed, and committed exclusively via atomic `renameat2(RENAME_NOREPLACE)` / `linkat` with automatic `stem (1).ext`..`stem (999).ext` collision numbering—never overwriting an existing file.

### 4. On-Demand Durable Background Worker & Crash Recovery (`radivine-worker`)
- **Out-of-Process Execution:** Conversions run in `radivine-worker`, backed by a SQLite WAL database (`~/.local/state/radivine/jobs.sqlite3`, `0600`) and `$XDG_RUNTIME_DIR/radivine/worker.sock` (`SO_PEERCRED` peer-UID authenticated).
- **Lifecycle Independence & Zero Idle Overhead:** Active jobs continue if the Control Center is closed or `radivine.service` restarts. When no jobs are running, `radivine-worker` exits automatically after `60 s` of idle time (`0.0%` CPU, `0 B` RSS when idle) and is relaunched on demand.
- **Startup Crash Recovery:** On startup, `radivine-worker` reconciles any interrupted `Preparing`/`Running`/`Cancelling` jobs from its `active_stages` crash journal, removes orphaned `.part` files and empty `.rvstage-<uid>` directories, and marks interrupted items as `Failed (WorkerInterrupted)` so they can be retried cleanly.

### 5. Control Center `Jobs` & `Conversion` Pages (`radivine-control`)
- **Jobs Page (`radivine-control --page jobs`):**
  - Live progress streaming (`JobsClient::subscribe`), status filter pills (`All`, `Active`, `Succeeded`, `Partial`, `Failed`, `Cancelled`), and a virtualized 200-item-per-page item table (`~0.11 ms` UI frame time on `10,000`-item jobs).
  - Cooperative per-item **Cancel** (`<= 65 ms` drain on in-flight 12 MP conversions), immutable linked **Retry** (`Retry failed`, `Retry failed & cancelled`, `Retry all`, `Retry to folder…`), **Open folder** (`org.freedesktop.FileManager1.ShowItems`), **Clear history**, live worker concurrency selector, and `.rvstage-<uid>` staging cleanup.
- **Conversion Settings Page (`radivine-control --page conversion`):**
  - Format-preserving `toml_edit` editor for `~/.config/radivine/convert.toml` and `config.toml [jobs]` with external-edit conflict detection, `.bak` backups, custom preset CRUD, resource governor controls, backend capability matrix, and live worker settings reload.

### 6. Native Clipboard & Arabic / Non-Latin Font Fallback
- **Native System Clipboard:** Enabled `eframe` system clipboard support (`arboard` on X11, `smithay-clipboard` on Wayland) for `Ctrl+C`, `Ctrl+V`, `Ctrl+X`, and copy buttons across Custom Actions, Conversion Settings, Advanced Convert, Jobs, Diagnostics, and `radivine-viewer`.
- **Arabic & Non-Latin Font Fallback:** Enabled `fontique`/Fontconfig system-font discovery paired with `cosmic-text` HarfBuzz complex-script shaping so Arabic (`تحويل الصور`), CJK, Hebrew, Cyrillic, and Devanagari filenames, paths, and text inputs render with shaped system glyphs instead of tofu boxes.

---

## Supported Linux Distributions & Installation

- **Primary Target & Verified Desktop:** **Linux Mint 22.x (Cinnamon on X11, `amd64`)**
- **Compatible Debian/Ubuntu Base:** **Ubuntu 24.04 LTS+** and derivatives (`amd64`, `glibc >= 2.34`)

Download `radivine_0.2.0_amd64.deb` and `SHA256SUMS.txt` from the release assets below:

```bash
# 1. Verify the SHA-256 checksum
sha256sum -c SHA256SUMS.txt

# 2. Install the Debian package
sudo apt install ./radivine_0.2.0_amd64.deb

# 3. Launch Radivine Control Center (or open "Radivine" from your desktop menu)
radivine-control
```

### SHA-256 Checksum

```text
ae31469335904b235d99e635384e86ee9c2332f1679e0cd0e5fd3c91a4d6d8c4  radivine_0.2.0_amd64.deb
```

---

## Upgrade Notes from v0.1.1

- **Configuration & Custom Actions Preserved:** Upgrading via `sudo apt install ./radivine_0.2.0_amd64.deb` preserves your existing `~/.config/radivine/config.toml` and `~/.config/radivine/actions.toml` byte-for-byte. Conversion settings (`~/.config/radivine/convert.toml`) use built-in defaults until customized in **Control Center → Conversion**.
- **Reload Systemd User Units After Upgrade:** After installing `radivine_0.2.0_amd64.deb` over `v0.1.1`, reload your user systemd manager and restart the wheel daemon so the updated `radivine` binary and new `radivine-worker.service` unit are active:
  ```bash
  systemctl --user daemon-reload
  systemctl --user try-restart radivine.service
  ```

---

## Known Limitations

- **X11 Display Server Required for Radial Wheel Overlay:** The global drag-and-drop (`XDND`) and pointer-tracking radial wheel (`radivine daemon`) requires an **X11 session** (`XDG_SESSION_TYPE=x11`). Running `radivine daemon` on Wayland prints a diagnostic notice and exits cleanly.
- **Static Raster Images Only (`PNG`, `JPEG`, `WebP`):** `v0.2.0` converts single-frame static `PNG`, `JPEG`, and `WebP` images (`<= 16,384 × 16,384` px, `<= 100 MP`). Animated WebP and APNG inputs are detected and rejected cleanly with `AnimatedInputUnsupported` without modifying any files.
- **Left-to-Right Widget Container Alignment (`egui 0.37`):** While `cosmic-text` performs contextual Arabic glyph shaping and inline bidirectional run reordering, `egui 0.37` widget container layout alignment remains left-to-right (LTR).

---

## License

Dual-licensed under **`MIT OR Apache-2.0`**. See [`LICENSE-MIT`](https://github.com/marawan6569/Radivine/blob/main/LICENSE-MIT), [`LICENSE-APACHE`](https://github.com/marawan6569/Radivine/blob/main/LICENSE-APACHE), and [`THIRD_PARTY_NOTICES.md`](https://github.com/marawan6569/Radivine/blob/main/THIRD_PARTY_NOTICES.md).
