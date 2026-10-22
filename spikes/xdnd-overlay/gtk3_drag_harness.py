#!/usr/bin/env python3
"""
GTK 3.24.41 Drag Source & Underlying Drop Target Harness for Radivine M0 Spikes.
Uses the exact same libgtk-3 C library (gtk_drag_begin / gdkdnd-x11.c) as Nemo 6.6.3,
while logging internal GdkDragContext state, signals, and underlying target drops.
"""
import argparse
import os
import sys
import time
import urllib.parse

import gi
gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, GdkX11, GLib, Gtk


def now_us(t0: float) -> int:
    return int((time.monotonic() - t0) * 1_000_000)


class HarnessWindow(Gtk.Window):
    def __init__(self, uris: list[str], t0: float):
        super().__init__(title="Radivine-M0-GTK3-Harness")
        self.uris = uris
        self.t0 = t0
        self.drag_context = None
        self.set_default_size(500, 300)
        self.move(100, 100)

        box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        self.add(box)

        # Left side: Drag Source Box (100..300, 100..400)
        self.src_box = Gtk.EventBox()
        self.src_box.set_size_request(240, 280)
        src_label = Gtk.Label(label="GTK3 Drag Source\n(Nemo-identical targets & actions)")
        self.src_box.add(src_label)
        box.pack_start(self.src_box, True, True, 0)

        # Right side: Underlying Folder Drop Target (simulating Nemo folder underneath overlay)
        self.dst_box = Gtk.EventBox()
        self.dst_box.set_size_request(240, 280)
        dst_label = Gtk.Label(label="Underlying GTK3 Drop Target\n(Simulates Nemo Folder)")
        self.dst_box.add(dst_label)
        box.pack_start(self.dst_box, True, True, 0)

        targets = [
            Gtk.TargetEntry.new("x-special/gnome-icon-list", 0, 1),
            Gtk.TargetEntry.new("text/uri-list", 0, 2),
            Gtk.TargetEntry.new("UTF8_STRING", 0, 3),
            Gtk.TargetEntry.new("text/plain", 0, 4),
        ]
        actions = (
            Gdk.DragAction.MOVE
            | Gdk.DragAction.COPY
            | Gdk.DragAction.LINK
            | Gdk.DragAction.ASK
        )

        self.src_box.drag_source_set(
            Gdk.ModifierType.BUTTON1_MASK,
            targets,
            actions,
        )
        self.src_box.connect("drag-begin", self.on_drag_begin)
        self.src_box.connect("drag-data-get", self.on_drag_data_get)
        self.src_box.connect("drag-data-delete", self.on_drag_data_delete)
        self.src_box.connect("drag-failed", self.on_drag_failed)
        self.src_box.connect("drag-end", self.on_drag_end)

        self.dst_box.drag_dest_set(
            Gtk.DestDefaults.ALL,
            targets,
            actions,
        )
        self.dst_box.connect("drag-motion", self.on_dest_motion)
        self.dst_box.connect("drag-leave", self.on_dest_leave)
        self.dst_box.connect("drag-drop", self.on_dest_drop)
        self.dst_box.connect("drag-data-received", self.on_dest_data_received)

    def on_drag_begin(self, widget, context):
        self.drag_context = context
        context.connect("action-changed", self.on_context_action_changed)
        context.connect("drop-performed", self.on_context_drop_performed)
        context.connect("dnd-finished", self.on_context_dnd_finished)
        context.connect("cancel", self.on_context_cancel)
        print(
            f"[{now_us(self.t0):>8}µs] GTK_SOURCE_DRAG_BEGIN: actions={context.get_actions()!r}",
            flush=True,
        )
        GLib.timeout_add(10, self.poll_context)

    def poll_context(self):
        if self.drag_context is None:
            return False
        dest = self.drag_context.get_dest_window()
        dest_xid = dest.get_xid() if dest is not None else 0
        sel_act = self.drag_context.get_selected_action()
        sug_act = self.drag_context.get_suggested_action()
        last = getattr(self, "_last_polled", None)
        cur = (dest_xid, int(sel_act), int(sug_act))
        if cur != last:
            self._last_polled = cur
            print(
                f"[{now_us(self.t0):>8}µs] GTK_CONTEXT_STATE: dest_xid={dest_xid:#x}, selected_action={sel_act.value_names}, suggested_action={sug_act.value_names}",
                flush=True,
            )
        return True

    def on_context_action_changed(self, context, action):
        dest = context.get_dest_window()
        dest_xid = dest.get_xid() if dest is not None else 0
        print(
            f"[{now_us(self.t0):>8}µs] GTK_CONTEXT_ACTION_CHANGED: dest_xid={dest_xid:#x}, action={action.value_names}",
            flush=True,
        )

    def on_context_drop_performed(self, context, time_):
        dest = context.get_dest_window()
        dest_xid = dest.get_xid() if dest is not None else 0
        print(
            f"[{now_us(self.t0):>8}µs] GTK_CONTEXT_DROP_PERFORMED: dest_xid={dest_xid:#x}, time={time_}",
            flush=True,
        )

    def on_context_dnd_finished(self, context):
        print(f"[{now_us(self.t0):>8}µs] GTK_CONTEXT_DND_FINISHED", flush=True)

    def on_context_cancel(self, context, reason):
        print(
            f"[{now_us(self.t0):>8}µs] GTK_CONTEXT_CANCEL: reason={reason.value_nick}",
            flush=True,
        )

    def on_drag_data_get(self, widget, context, data, info, time_):
        target_name = data.get_target().name()
        payload = "".join(f"{u}\r\n" for u in self.uris).encode("utf-8")
        data.set(data.get_target(), 8, payload)
        print(
            f"[{now_us(self.t0):>8}µs] GTK_SOURCE_DATA_GET: target={target_name}, bytes={len(payload)}, time={time_}",
            flush=True,
        )

    def on_drag_data_delete(self, widget, context):
        print(
            f"[{now_us(self.t0):>8}µs] GTK_SOURCE_DATA_DELETE_CALLED: CRITICAL WARNING!",
            flush=True,
        )

    def on_drag_failed(self, widget, context, result):
        print(
            f"[{now_us(self.t0):>8}µs] GTK_SOURCE_DRAG_FAILED: result={result.value_nick}",
            flush=True,
        )
        return False

    def on_drag_end(self, widget, context):
        sel_act = context.get_selected_action()
        print(
            f"[{now_us(self.t0):>8}µs] GTK_SOURCE_DRAG_END: final_selected_action={sel_act.value_names}",
            flush=True,
        )
        self.drag_context = None

    def on_dest_motion(self, widget, context, x, y, time_):
        sug = context.get_suggested_action()
        print(
            f"[{now_us(self.t0):>8}µs] UNDERLYING_FOLDER_MOTION: pos=({x},{y}), suggested_action={sug.value_names}",
            flush=True,
        )
        return True

    def on_dest_leave(self, widget, context, time_):
        print(f"[{now_us(self.t0):>8}µs] UNDERLYING_FOLDER_LEAVE", flush=True)

    def on_dest_drop(self, widget, context, x, y, time_):
        sel = context.get_selected_action()
        sug = context.get_suggested_action()
        print(
            f"[{now_us(self.t0):>8}µs] UNDERLYING_FOLDER_DROP: pos=({x},{y}), selected_action={sel.value_names}, suggested_action={sug.value_names}",
            flush=True,
        )
        return True

    def on_dest_data_received(self, widget, context, x, y, data, info, time_):
        sel = context.get_selected_action()
        print(
            f"[{now_us(self.t0):>8}µs] UNDERLYING_FOLDER_DATA_RECEIVED: selected_action={sel.value_names}, bytes={len(data.get_data())}",
            flush=True,
        )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--files", nargs="*", default=["/tmp/radivine-m0-test/main/alpha_test.txt"])
    parser.add_argument("--dir-500", action="store_true")
    parser.add_argument("--large-incr", action="store_true")
    parser.add_argument("--sftp-uri", action="store_true")
    parser.add_argument("--duration-ms", type=int, default=4000)
    args = parser.parse_args()

    t0 = time.monotonic()
    uris = []
    if args.large_incr:
        d = "/tmp/radivine-m0-test/many500"
        base_files = [
            "file://" + urllib.parse.quote(os.path.join(d, name))
            for name in sorted(os.listdir(d))
        ]
        # Repeat 6x = 3000 URIs (~318,000 bytes > 262,140 bytes GTK_SELECTION_MAX_SIZE)
        uris = base_files * 6
    elif args.dir_500:
        d = "/tmp/radivine-m0-test/many500"
        for name in sorted(os.listdir(d)):
            p = os.path.join(d, name)
            uris.append("file://" + urllib.parse.quote(p))
    elif args.sftp_uri:
        uris.append("sftp://example.local/home/user/remote_notes.txt")
        uris.append("file:///tmp/radivine-m0-test/main/alpha_test.txt")
    else:
        for f in args.files:
            uris.append("file://" + urllib.parse.quote(os.path.abspath(f)))

    win = HarnessWindow(uris, t0)
    win.connect("destroy", Gtk.main_quit)
    win.show_all()

    def report_ready():
        gdk_win = win.get_window()
        xid = gdk_win.get_xid() if gdk_win else 0
        rx, ry = win.get_position()
        print(
            f"[{now_us(t0):>8}µs] HARNESS_READY: xid={xid:#x}, pos=({rx},{ry}), uris_count={len(uris)}",
            flush=True,
        )
        return False

    GLib.timeout_add(150, report_ready)
    GLib.timeout_add(args.duration_ms, lambda: (Gtk.main_quit(), False)[1])
    Gtk.main()


if __name__ == "__main__":
    main()
