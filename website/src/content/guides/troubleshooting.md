---
title: Find the missing connection.
description: Check the session, daemon and action requirements before changing your configuration.
slug: troubleshooting
lang: en
updatedOn: "2026-10-27"
productVersion: "0.3.0"
evidenceIds:
  [doctor, cli, platform, config, actions, custom, packaging, worker, n5]
draft: false
---

## Collect the basics

Run these as your ordinary desktop user:

```sh
radivine --version
radivine doctor
radivine status --json
```

Doctor probes X11 extensions, visuals, compositing, monitors and capabilities. On a systemd-managed installation, inspect the service and recent logs:

```sh
systemctl --user status radivine.service
journalctl --user -u radivine.service -n 100 --no-pager
```

Diagnostics and logs can contain local paths or filenames. Redact personal information before sharing them. Do not run automated pointer-injection tests against your active desktop to troubleshoot a normal installation.

## The package will not install

Confirm the amd64 architecture, package checksum and required libraries. Use `apt install ./filename.deb` from the download directory so dependencies can be resolved. Do not substitute a package from another architecture. A missing dependency or desktop library needs investigation on that distribution; it is not evidence that all Debian-based desktops are unsupported.

## The wheel does not appear

Make sure the daemon is running in the intended graphical user session. Start it from Control Center. A system service or a command run with sudo is not equivalent to the desktop user service. Inspect Doctor and service logs before retrying.

The shipping wheel requires X11. Log into an X11 desktop session for Drag + Shift. Native Wayland is unsupported. XWayland Invoke is conditional and can have restricted pointer positioning; a functioning Control Center or worker does not prove that the wheel is available.

## Shift does nothing during a drag

Begin dragging first, then press the configured modifier freshly. Check the trigger setting and try the primary Nemo workflow. Other XDND sources need separate verification. A second press may hide an already active wheel. Use Nemo’s Open in Radivine action to test Invoke independently.

## Concentric becomes Branching

Check compositing and logical workarea. The default layout reserves 668 × 742 logical pixels, so a high physical resolution can still be too small after scaling. Custom geometry and undersized targets can also trigger fallback. The saved layout is not rewritten; inspect status/Diagnostics and test again on the next summon.

Mixed-DPI, monitor edges, negative origins and hotplug behavior retain manual verification gaps. Include exact monitor configuration in a report rather than assuming global Xft scaling proves per-monitor fidelity.

## An action is missing, disabled or only logs

Wait for bounded inspection to finish and read the disabled reason. Text preview requires one eligible local text file. Quick Convert requires supported static raster images, and a same-target request can be skipped. Properties requires a compatible FileManager1 service.

Default terminal, checksum and executable examples are logging-only. Real custom actions need valid TOML, whole-argument placeholders, a selection matching their filters and an installed executable at the configured absolute path. Reload after changing actions and inspect warnings.

## A conversion failed or stopped

Inspect the individual result in Jobs. Check format/animation, image dimensions, size and allocation limits, destination permissions and available space. Failed or interrupted work can be retried as a linked job. Do not assume an interrupted job resumes automatically, or remove staging/state files while the worker is active.

## Fonts, themes or startup look wrong

Install the declared DejaVu and Fontconfig dependencies. Arabic/non-Latin rendering depends on system fonts; widget containers remain LTR. Website theme settings do not configure Radivine. In the application, choose a specific theme to distinguish auto-detection from rendering issues.

For login startup, check Control Center’s opt-in setting and the user service, not a system-wide service. On non-systemd desktops the fallback lifecycle path needs its own environment checks.

## Still stuck?

Include the version, installation method, OS, desktop/window manager, session protocol, file manager, compositor and scaling; list exact steps and expected versus actual behavior. Use disposable sample files and redacted diagnostics. Report security-sensitive findings privately rather than attaching them to a public issue.
