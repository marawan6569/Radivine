/** Formula ports from radivine-wheel geometry/layout/concentric; defaults from AppConfig. */
import { itemsAt, type Action } from "./contexts.ts";
export type Point = { x: number; y: number };
export type Layout = "branching" | "concentric";
export type Environment =
  "desktop" | "small" | "uncomposited" | "small-targets";
export type Span = { start: number; end: number; center: number };
export interface Wheel {
  path: number[];
  center: Point;
  inner: number;
  outer: number;
  spans: Span[];
  bridge?: Span;
  corridor?: Point[];
  attach?: number;
}
export const config = Object.freeze({
  root: [40, 128],
  child: [34, 108],
  deep: [26, 84],
  childGap: 20,
  dwell: 110,
  grace: 180,
  hysteresis: 3.5,
  slop: 6,
  margin: 12,
  gap: 2.5,
  bridge: 70,
  band: 0.65,
  hubClose: 150,
  fresh: 2,
});
export const viewport = { width: 900, height: 800 };
export const anchor: Point = { x: 450, y: 350 };
export const tau = Math.PI * 2;
export const radians = (d: number) => (d * Math.PI) / 180;
export const angleDiff = (a: number, b: number) =>
  Math.atan2(Math.sin(a - b), Math.cos(a - b));
export const distance = (a: Point, b: Point) =>
  Math.hypot(a.x - b.x, a.y - b.y);
export const angle = (a: Point, b: Point) => Math.atan2(b.y - a.y, b.x - a.x);
export const polar = (c: Point, r: number, a: number): Point => ({
  x: c.x + r * Math.cos(a),
  y: c.y + r * Math.sin(a),
});
export function selection(requested: Layout, environment: Environment) {
  const fallback =
    requested === "concentric"
      ? {
          desktop: "",
          small:
            "Insufficient space: the default rings reserve 668 × 742 logical pixels.",
          uncomposited:
            "No compositor: the desktop application falls back to Branching.",
          "small-targets":
            "Targets too small: a ring must fit a 32 × 32 logical-pixel target core.",
        }[environment]
      : "";
  return { effective: fallback ? ("branching" as const) : requested, fallback };
}
export function rootSpans(count: number): Span[] {
  const step = tau / count;
  return Array.from({ length: count }, (_, i) => {
    const center = -Math.PI / 2 + i * step;
    return { start: center - step / 2, end: center + step / 2, center };
  });
}
export function childSpans(count: number, beta: number) {
  const width = radians(config.bridge),
    step = Math.min((tau - width) / count, radians(72));
  const start =
    Math.abs(count * step - (tau - width)) < 1e-5
      ? beta + width / 2
      : beta + Math.PI - (count * step) / 2;
  return {
    bridge: { start: beta - width / 2, end: beta + width / 2, center: beta },
    spans: Array.from({ length: count }, (_, i) => ({
      start: start + i * step,
      end: start + (i + 1) * step,
      center: start + (i + 0.5) * step,
    })),
  };
}
const cross = (a: Point, b: Point, c: Point) =>
  (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
function hull(points: Point[]): Point[] {
  const sorted = [...points].sort((a, b) => a.x - b.x || a.y - b.y);
  const half = (ps: Point[]) => {
    const out: Point[] = [];
    for (const p of ps) {
      while (out.length > 1 && cross(out.at(-2)!, out.at(-1)!, p) <= 0)
        out.pop();
      out.push(p);
    }
    return out.slice(0, -1);
  };
  return [...half(sorted), ...half(sorted.reverse())];
}
export function inside(p: Point, poly: Point[]) {
  return (
    poly.every((a, i) => cross(a, poly[(i + 1) % poly.length], p) >= -1e-5) ||
    poly.every((a, i) => cross(a, poly[(i + 1) % poly.length], p) <= 1e-5)
  );
}
export function intentTriangle(exit: Point, child: Wheel): Point[] {
  const d = distance(exit, child.center),
    r = child.outer + config.margin;
  const a = angle(child.center, exit),
    off = Math.acos(Math.min(1, r / Math.max(d, r)));
  return [
    exit,
    polar(child.center, r, a + off),
    polar(child.center, r, a - off),
  ];
}
export function placeChild(
  parent: Wheel,
  phi: number,
  nominal: number,
  ancestors: Wheel[],
) {
  const center = { x: viewport.width / 2, y: viewport.height / 2 };
  const d0 = parent.outer + config.childGap + nominal;
  const sign =
    distance(polar(parent.center, d0, phi + radians(12)), center) <=
    distance(polar(parent.center, d0, phi - radians(12)), center)
      ? 1
      : -1;
  const radii = [];
  for (let i = 0; nominal * (1 - i * 0.1) > 64.5; i++)
    radii.push(nominal * (1 - i * 0.1));
  radii.push(64);
  for (const r of radii)
    for (let i = 0; i < 13; i++) {
      const a =
        phi + (i ? Math.ceil(i / 2) * (i % 2 ? sign : -sign) * radians(12) : 0);
      const c = polar(parent.center, parent.outer + config.childGap + r, a);
      if (
        c.x >= r + 8 &&
        c.x <= viewport.width - r - 8 &&
        c.y >= r + 8 &&
        c.y <= viewport.height - r - 8 &&
        ancestors.every((w) => distance(c, w.center) >= r + w.outer + 6 - 1e-3)
      )
        return { center: c, outer: r, attach: a };
    }
  // Bounded fixtures must fit; never invent a crowded/clamped interactive placement.
  throw new Error("Fixture exceeds verified Branching viewport");
}
export function wheelsFor(
  menu: Action[],
  path: number[],
  layout: Layout,
): Wheel[] {
  const wheels: Wheel[] = [];
  for (let level = 0; level <= path.length; level++) {
    const prefix = path.slice(0, level),
      items = itemsAt(menu, prefix);
    if (!items.length) break;
    if (!level || layout === "concentric") {
      wheels.push({
        path: prefix,
        center: anchor,
        inner: level ? 128 + (level - 1) * 66 + 6 : 40,
        outer: 128 + level * 66,
        spans: rootSpans(items.length),
      });
    } else {
      const parent = wheels[level - 1],
        span = parent.spans[path[level - 1]],
        pair = level === 1 ? config.child : config.deep;
      const placement = placeChild(
        parent,
        span.center,
        pair[1],
        wheels.slice(0, -1),
      );
      const beta = angle(placement.center, parent.center),
        parts = childSpans(items.length, beta);
      const delta = config.gap / 2 / parent.outer;
      const corridor = hull([
        polar(parent.center, parent.outer, span.start + delta),
        polar(parent.center, parent.outer, span.end - delta),
        polar(
          placement.center,
          placement.outer + 12,
          placement.attach + Math.PI / 2,
        ),
        polar(
          placement.center,
          placement.outer + 12,
          placement.attach - Math.PI / 2,
        ),
      ]);
      wheels.push({
        path: prefix,
        ...placement,
        inner: (placement.outer * pair[0]) / pair[1],
        ...parts,
        corridor,
      });
    }
  }
  return wheels;
}
export type Hit =
  | { kind: "item"; level: number; index: number }
  | { kind: "hub" | "neutral" | "outside" | "bridge"; level: number };
export function hitTest(p: Point, wheels: Wheel[], layout: Layout): Hit {
  if (!Number.isFinite(p.x + p.y)) return { kind: "outside", level: 0 };
  for (const [level, w] of [...wheels.entries()].reverse()) {
    const d = distance(p, w.center),
      a = angle(w.center, p);
    if (
      layout === "concentric" &&
      (Math.abs(d - w.inner) <= 1e-4 || Math.abs(d - w.outer) <= 1e-4)
    )
      return { kind: "neutral", level };
    if (d < w.inner && (layout === "branching" || level === 0))
      return { kind: "hub", level };
    if (d > w.outer + (layout === "branching" ? 12 : 0) || d < w.inner)
      continue;
    if (
      layout === "branching" &&
      level &&
      d > w.outer &&
      distance(p, wheels[level - 1].center) <= wheels[level - 1].outer
    )
      continue;
    if (w.bridge && Math.abs(angleDiff(a, w.bridge.center)) < radians(35))
      return { kind: "bridge", level };
    for (const [index, span] of w.spans.entries()) {
      // Browser release always rejects visible separators, including Branching's visual-only gaps.
      const inset =
        w.spans.length === 1 && layout === "concentric"
          ? 0
          : layout === "concentric"
            ? Math.asin(config.gap / (2 * w.inner)) + 1e-5
            : config.gap / 2 / Math.max(d, w.inner);
      if (
        Math.abs(angleDiff(a, span.center)) <
        (span.end - span.start) / 2 - inset
      )
        return { kind: "item", level, index };
    }
    return { kind: "neutral", level };
  }
  if (layout === "branching")
    for (const [level, w] of [...wheels.entries()].reverse())
      if (w.corridor && inside(p, w.corridor)) return { kind: "bridge", level };
  const d = distance(p, anchor);
  return {
    kind:
      layout === "concentric" && d <= wheels.at(-1)!.outer + 12
        ? "neutral"
        : "outside",
    level: 0,
  };
}
export function itemPoint(wheel: Wheel, index: number, radius?: number) {
  return polar(
    wheel.center,
    radius ?? wheel.inner + 0.55 * (wheel.outer - wheel.inner),
    wheel.spans[index].center,
  );
}
export function sectorPath(w: Wheel, s: Span, layout: Layout): string {
  if (layout === "concentric" && w.spans.length === 1) {
    // Separate closed contours avoid a painted radial seam in a full annulus.
    const { x, y } = w.center,
      outer = w.outer,
      inner = w.inner;
    return `M${x},${y - outer} A${outer},${outer} 0 1 1 ${x},${y + outer} A${outer},${outer} 0 1 1 ${x},${y - outer} Z M${x},${y - inner} A${inner},${inner} 0 1 0 ${x},${y + inner} A${inner},${inner} 0 1 0 ${x},${y - inner} Z`;
  }

  const di =
    layout === "concentric"
      ? w.spans.length === 1
        ? 0
        : Math.asin(2.5 / (2 * w.inner))
      : 1.25 / w.inner;
  const dout = layout === "concentric" ? di : 1.25 / w.outer;
  // Two arcs per edge also handle a full-circle one-item concentric annulus.
  const mid = s.center,
    a = polar(w.center, w.outer, s.start + dout),
    b = polar(w.center, w.outer, mid),
    c = polar(w.center, w.outer, s.end - dout),
    d = polar(w.center, w.inner, s.end - di),
    e = polar(w.center, w.inner, mid),
    f = polar(w.center, w.inner, s.start + di);
  return `M${a.x},${a.y} A${w.outer},${w.outer} 0 0 1 ${b.x},${b.y} A${w.outer},${w.outer} 0 0 1 ${c.x},${c.y} L${d.x},${d.y} A${w.inner},${w.inner} 0 0 0 ${e.x},${e.y} A${w.inner},${w.inner} 0 0 0 ${f.x},${f.y} Z`;
}
