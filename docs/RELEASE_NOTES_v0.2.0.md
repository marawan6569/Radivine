# Radivine v0.2.0 — Built-In Image Conversion, Durable Background Jobs & Desktop Polish

**Branch into action.**  
*A fast, extensible, context-aware radial action wheel for your Linux desktop.*

---

## Highlights of v0.2.0

`v0.2.0` introduces a complete, safety-first **Image Conversion Subsystem**, an on-demand **Durable Background Job Worker** (`radivine-worker`), new **Conversion** and **Jobs** pages in the **Radivine Control Center**, a standalone **Advanced Convert** dialog (`radivine-control convert`), native desktop clipboard interoperability, and Arabic/non-Latin system-font fallback:

1. **Radial Wheel Image Conversion (`Convert ▸ Image`):**
   - Select one or more images (`PNG`, `JPEG`, `WebP`) in Nemo via `Drag + Shift` or `radivine invoke` and branch into **`Convert ▸ Image ▸ PNG / JPEG / WebP / Advanced…`**.
   - **Quick Convert:** One-gesture conversion using your per-format default preset (`web-optimized` for WebP, `balanced` for JPEG, `lossless` for PNG). Submits a durable job in `< 7 ms` p95 so the radial wheel closes instantaneously.
   - **Smart Selection Handling:** Hides redundant same-format Quick Convert targets on single-item selections, summarizes skipped or unsupported items in mixed selections directly in the wheel caption, and supports batches up to `10,000` files.
2. **Standalone Advanced Convert Window (`radivine-control convert`):**
   - Choose **`Convert ▸ Image ▸ Advanced…`** from the radial wheel (or run `radivine-control convert [FILES…]`) to open a dedicated `640×720` conversion dialog.
   - Select from 5 built-in presets (`Web Optimized`, `Balanced`, `High Quality`, `Lossless`, `Compact Share`) or custom presets, adjust explicit format quality/subsampling/compression settings, configure Lanczos3 aspect-preserving resize (`Original`, `Width`, `Height`, `Fit`), set alpha flatten background color, choose output destination (`Beside original` or custom folder) and filename suffix, and control EXIF/ICC/orientation metadata policies.
   - Header-only background inspection (`inspect_file_header`) keeps startup fast (`~18 ms` headless ready) without decoding full pixels in the UI process.
3. **Built-In Image Codec Engine & Strict File Safety (`radivine-convert`, `radivine-convert-image`, `ffi/radivine-webp`):**
   - Pure-Rust PNG (`png` `0.18`), JPEG (`zune-jpeg` `0.5` + SIMD `jpeg-encoder` `0.6`), and lossless WebP (`image-webp` `0.2`) paired with SIMD + multi-threaded lossy WebP (`libwebp` `1.5.0` isolated in `ffi/radivine-webp`).
   - Full support for 8-bit/16-bit/grayscale/CMYK inputs, EXIF orientation `1..=8` auto-rotation with tag-`0x0112` reset, ICC profile preservation across PNG/JPEG/WebP (`VP8X` + `ICCP` RIFF muxer), and **privacy-by-default EXIF stripping** (optional EXIF preservation with explicit GPS privacy warning).
   - **Sacred Source & Zero-Overwrite Guarantees:** Source files are opened `O_RDONLY | O_NOFOLLOW | O_NONBLOCK` and verified via `fstat`. Outputs are written into a private `0700` `.rvstage-<uid>` staging directory, `fsync`ed, and committed exclusively via atomic `renameat2(RENAME_NOREPLACE)` / `linkat` with automatic `stem (1).ext`..`stem (999).ext` collision numbering—never overwriting an existing file.
4. **On-Demand Durable Background Worker (`radivine-worker` & `radivine-jobs`):**
   - Conversions run out-of-process in `radivine-worker` backed by a SQLite WAL database (`~/.local/state/radivine/jobs.sqlite3`, `0600`) and `$XDG_RUNTIME_DIR/radivine/worker.sock` (`SO_PEERCRED` UID-authenticated).
   - Survives closing the Control Center or restarting `radivine.service`, reconciles interrupted items and `.rvstage-<uid>` directories after crashes or power loss, enforces bounded memory reservations (`ResourceGovernor`), exits automatically after `60 s` of idle time (`0.00%` idle CPU), and sends consolidated D-Bus desktop notifications.
5. **Control Center `Jobs` & `Conversion` Pages (`radivine-control`):**
   - **Jobs Page (`--page jobs`):** Real-time progress streaming, state filter pills, virtualized 200-item-per-page item tables (`~0.76 ms` frame time on 1,000 items), cooperative `Cancel`, linked immutable retries (`Retry failed`, `Retry failed & cancelled`, `Retry all`, `Retry to folder…`), `Open folder` (`org.freedesktop.FileManager1.ShowItems`), `Clear history`, concurrency selector, and staging cleanup.
   - **Conversion Page (`--page conversion`):** Format-preserving `toml_edit` editor for `~/.config/radivine/convert.toml` and `config.toml [jobs]` with conflict detection, custom preset CRUD, and live worker settings reload.
6. **Native Clipboard & Arabic / Non-Latin Script Support:**
   - Full system clipboard integration (`Ctrl+C`, `Ctrl+V`, `Ctrl+X`, and copy buttons) via `arboard` on X11 and `smithay-clipboard` on Wayland.
   - Automatic system-font fallback (`fontique` / Fontconfig) and HarfBuzz complex-script shaping (`cosmic-text`) for Arabic, CJK, Hebrew, Cyrillic, and Devanagari text across all Control Center pages, Advanced Convert, and `radivine-viewer`. *(Note: `egui 0.37` widget container layout alignment remains LTR.)*

---

## Installation & Upgrade (`radivine_0.2.0_amd64.deb`)

Download `radivine_0.2.0_amd64.deb` and `SHA256SUMS.txt` from the release assets:

```bash
# 1. Verify the SHA-256 checksum
sha256sum -c SHA256SUMS.txt

# 2. Install or upgrade the Debian package (preserves existing config.toml & actions.toml)
sudo apt install ./radivine_0.2.0_amd64.deb

# 3. Reload systemd user units and restart the background daemon if upgrading from v0.1.1
systemctl --user daemon-reload
systemctl --user try-restart radivine.service

# 4. Open Radivine Control Center
radivine-control
```

---

## License

Dual-licensed under **`MIT OR Apache-2.0`**. See [`LICENSE-MIT`](LICENSE-MIT), [`LICENSE-APACHE`](LICENSE-APACHE), and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
