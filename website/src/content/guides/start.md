---
title: A little closer to your files.
description: Start the daemon, meet the wheel, and find the right tool for the files in front of you.
slug: start
lang: en
updatedOn: "2026-10-27"
productVersion: "0.3.0"
evidenceIds: [cli, wheel, actions, preview, convert, worker, control, config]
draft: false
---

## Start the application

After installation, open **Radivine Control Center** from the application menu and choose **Start Daemon** on Dashboard. Login startup is optional. Closing Control Center leaves the wheel daemon running; Stop Daemon stops it. The website theme is independent of the desktop application.

## Drag, then Shift

On the primary tested Cinnamon X11 / Nemo setup, select files and start dragging. Press **Shift freshly during the drag** to summon the wheel. Move into a submenu, then release over an enabled leaf action. A submenu itself is navigation, not a command to execute.

With the safe default `input.passthrough = false`, releasing outside the active wheel cancels. A second Shift press hides the wheel so native dragging can resume. Do not assume keyboard cancellation works during every foreign drag: other applications can hold keyboard grabs.

## Prefer a click or the keyboard?

Select files in Nemo and choose **Open in Radivine**, or pass local paths to the CLI:

```sh
radivine invoke /path/to/example.txt
```

Replace that example path with an existing file. Invoke needs a running daemon and a usable X11 display. Use arrows to navigate, Enter or Space to activate, Backspace to go up, digits 1–9 to activate indexed entries, and Escape to cancel. The application is not a global shortcut manager; configure a desktop shortcut separately if wanted.

## Choose a layout

**Control Center → Wheel → Wheel Layout** offers Branching (the default) and Concentric Rings. A change during an active session applies on the next summon. Concentric needs compositing and enough logical workarea; fallback to Branching does not change the saved preference.

## Read a file

For one eligible local text file, choose **File Preview → Full Preview, Head or Tail**. The separate viewer is read-only, with virtualized reads and search. It is not an editor, graphical image viewer or live-follow terminal. The default Head and Tail line counts can be configured.

## Convert an image

For supported static images, choose **Convert → Image → PNG, JPEG or WebP**. This queues work; selecting the action does not mean conversion has already finished. Mixed batches report skips. Same-target Quick Convert may be disabled or skipped.

Use **Advanced Convert** for presets, quality, resizing, orientation, metadata and output choices, including same-format re-encoding. Conversion produces separate outputs using no-overwrite commit handling. Animated files and other formats are outside this built-in pipeline.

## Follow background work

Open **Control Center → Jobs** to inspect progress and per-item results, cancel work or create linked retries. The on-demand worker runs independently of the wheel and keeps durable history. Crash recovery reconciles interrupted work; it does not automatically resume every unfinished conversion. Finish or cancel jobs before uninstalling or restarting their service.

## Know what a menu entry means

Actions can be Checking…, enabled, or disabled with a reason. Visibility and enablement depend on context and bounded inspection. Copy places file URIs on the X11 clipboard for a later paste. Properties needs a compatible file-manager service.

The default **Open in Terminal**, **Compute SHA-256** and **Make Executable** examples only log. To run a real external command, configure an executable custom action with an installed program.
