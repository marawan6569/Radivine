---
title: Your wheel. Your way.
description: Use Control Center for everyday settings, or edit the same TOML files deliberately.
slug: configuration
lang: en
updatedOn: "2026-10-27"
productVersion: "0.3.0"
evidenceIds: [config, custom, action-tests, control, examples, worker]
draft: false
---

## Start in Control Center

Dashboard manages the daemon and opt-in login startup. General and Wheel cover appearance, input and layouts. Custom Actions edits executable actions; Conversion manages conversion settings and presets; Jobs shows work; Diagnostics helps inspect the environment.

Settings updates preserve comments and unknown TOML keys. If another editor changes the file, resolve the reported conflict instead of overwriting it blindly. Active sessions keep their configuration snapshot until the next summon.

## Files and locations

Under the usual XDG defaults:

- `~/.config/radivine/config.toml` — wheel, trigger, appearance, input, preview, notifications and job settings.
- `~/.config/radivine/actions.toml` — executable custom actions and menu overrides.
- `~/.config/radivine/convert.toml` — conversion presets and limits, resolved by the worker.
- `~/.local/state/radivine/jobs.sqlite3` — durable job history; do not edit the database manually.

XDG directory overrides can change these paths. A daemon started with an explicit `--config` path also changes where its companion actions file is read. After manually editing the daemon configuration or actions, run:

```sh
radivine reload
```

Use Control Center for conversion settings: the worker snapshots them when preparing a job, not through the wheel daemon’s reload command. Existing jobs retain their resolved settings.

## Appearance and safe defaults

The application accepts `theme = "dark"`, `"light"` or `"auto"`. Scale accepts `"auto"` or a numeric factor from 0.5 to 4.0. Reduced motion accepts `"auto"`, `true` or `false`. The default drag modifier is Shift; supported alternatives are Ctrl, Alt, Super or disabled.

Keep **`input.passthrough = false`** unless deliberately testing the alternative. It captures the overlay during an active drag so an outside release cancels. Turning pass-through on can allow a native drop or move into a file manager underneath.

## Custom actions are real programs

Each `exec` is an argument vector, not a shell command. The first argument must be an absolute executable path. Install the external program yourself; a menu entry cannot supply it. Shell interpreters, pipelines, redirection, environment-variable expansion, and fragments such as `"--file={path}"` are not accepted substitutes for argv.

Placeholders must occupy an entire argument: `{path}`, `{paths}`, `{uri}`, `{uris}`, `{dir}` or `{name}`. A single-file placeholder needs `per_item = true` or a one-item count filter. The example below uses an existing file and the desktop’s default application; it requires `/usr/bin/xdg-open`. Check your installation with `command -v xdg-open` and use its actual absolute path if different.

An accepted configuration is not a sandbox for the selected program. Its effects remain those of the command you configure. The website does not execute this example.
