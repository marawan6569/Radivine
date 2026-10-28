import {
  actionAt,
  itemsAt,
  menuFor,
  type ContextId,
  type Action,
} from "./contexts.ts";
import { outcome } from "./outcomes.ts";
import {
  anchor,
  config,
  distance,
  angle,
  angleDiff,
  radians,
  inside,
  intentTriangle,
  hitTest,
  selection,
  wheelsFor,
  type Layout,
  type Environment,
  type Point,
  type Hit,
} from "./geometry.ts";
export type Phase = "idle" | "active" | "result" | "cancelled";
type Timer = {
  kind: "dwell" | "sibling" | "hub" | "intent";
  at: number;
  path: number[];
  generation: number;
  entered: number;
};
export interface State {
  context: ContextId;
  requested: Layout;
  environment: Environment;
  effective: Layout;
  fallback: string;
  phase: Phase;
  mode: "invoke" | "drag";
  menu: Action[];
  path: number[];
  hover: number[] | null;
  pointer: Point;
  summon: Point;
  moved: boolean;
  approach: boolean;
  keyboard: Point | null;
  generation: number;
  guards: Record<number, Point>;
  timer: Timer | null;
  press: {
    path: number[] | null;
    hub: boolean;
    generation: number;
    eligible: boolean;
  } | null;
  inspectionAt: number | null;
  inspected: boolean;
  result: string;
  lastInside: Point;
  intentExit: Point | null;
}
export type Event =
  | {
      type: "configure";
      context?: ContextId;
      requested?: Layout;
      environment?: Environment;
    }
  | { type: "start"; mode?: "invoke" | "drag"; point?: Point }
  | { type: "move" | "press" | "release"; point: Point }
  | { type: "key"; key: string }
  | { type: "activate" | "highlight"; path: number[] }
  | { type: "tick" | "cancel" | "reset" | "back" };
const same = (a: number[] | null, b: number[] | null) =>
  a?.join(".") === b?.join(".");
export function initial(
  context: ContextId = "text",
  requested: Layout = "branching",
  environment: Environment = "desktop",
  generation = 0,
): State {
  return {
    context,
    requested,
    environment,
    ...selection(requested, environment),
    phase: "idle",
    mode: "invoke",
    menu: menuFor(context),
    path: [],
    hover: null,
    pointer: anchor,
    summon: anchor,
    moved: false,
    approach: false,
    keyboard: null,
    generation,
    guards: {},
    timer: null,
    press: null,
    inspectionAt: null,
    inspected: false,
    result:
      "Choose a sample, then open the wheel. Everything here is simulated.",
    lastInside: anchor,
    intentExit: null,
  };
}
export const geometry = (s: State) => wheelsFor(s.menu, s.path, s.effective);
export const hitPath = (s: State, h: Hit): number[] | null =>
  h.kind === "item" ? [...s.path.slice(0, h.level), h.index] : null;
function truncate(s: State, level: number) {
  s.path = s.path.slice(0, level);
  s.generation++;
  s.timer = null;
  s.intentExit = null;
  if (s.effective === "concentric") s.guards = { [level]: s.pointer };
}
function open(s: State, path: number[], keyboard: boolean) {
  if (same(s.path.slice(0, path.length), path) && s.path.length >= path.length)
    return;
  s.path = [...path];
  s.generation++;
  s.timer = null;
  s.intentExit = null;
  if (s.effective === "concentric")
    s.guards = { ...s.guards, [path.length]: s.pointer };
  if (keyboard) s.hover = [...path, 0];
  s.result = `${actionAt(s.menu, path)!.label} opened. Choose a direction or use the action list.`;
}
function activate(s: State, path: number[], keyboard: boolean) {
  const action = actionAt(s.menu, path);
  if (!action) return;
  s.hover = path;
  if (action.state !== "enabled") {
    s.result = `${action.label}: ${action.reason}`;
    return;
  }
  if (action.children) open(s, path, keyboard);
  else {
    s.phase = "result";
    s.result = outcome(action);
    s.timer = null;
    s.inspectionAt = null;
    s.press = null;
  }
}
function cancel(s: State) {
  s.phase = "cancelled";
  s.timer = null;
  s.inspectionAt = null;
  s.press = null;
  s.hover = null;
  s.result =
    "Cancelled safely. No sample or local file was changed. Open the wheel to try again.";
}
function schedule(
  s: State,
  kind: Timer["kind"],
  path: number[],
  now: number,
  delay: number,
) {
  if (s.timer?.kind === kind && same(s.timer.path, path)) return;
  s.timer = {
    kind,
    path,
    at: now + delay,
    entered: now,
    generation: s.generation,
  };
}
function eligible(s: State, path: number[] | null) {
  return (
    !!path &&
    s.moved &&
    !s.approach &&
    !s.keyboard &&
    !s.guards[path.length - 1] &&
    s.timer?.kind !== "sibling" &&
    s.timer?.kind !== "intent" &&
    same(s.hover, path) &&
    actionAt(s.menu, path)?.state === "enabled"
  );
}
function move(s: State, p: Point, now: number) {
  if (s.keyboard && distance(p, s.keyboard) < config.fresh) return;
  s.keyboard = null;
  const prev = s.pointer,
    oldHit = hitTest(prev, geometry(s), s.effective);
  s.pointer = p;
  if (!s.moved && distance(p, s.summon) < config.slop) return;
  s.moved = true;
  for (const [level, origin] of Object.entries(s.guards))
    if (distance(p, origin) >= config.fresh) delete s.guards[Number(level)];
  if (s.approach && distance(p, anchor) < 40 - 1e-4) s.approach = false;
  if (s.approach) {
    s.hover = null;
    s.timer = null;
    return;
  }
  const wheels = geometry(s),
    h = hitTest(p, wheels, s.effective),
    path = hitPath(s, h);
  if (h.kind === "hub") {
    s.hover = null;
    if (s.path.length > h.level)
      schedule(s, "hub", s.path.slice(0, h.level), now, config.hubClose);
    else s.timer = null;
    return;
  }
  if (h.kind !== "item") {
    s.timer = null;
    s.intentExit = null;
    // Keep visual hover near separators/gaps, never action eligibility.
    const previous = s.hover,
      w = previous && wheels[previous.length - 1],
      span = w && w.spans[previous!.at(-1)!];
    const d = w ? distance(p, w.center) : 0;
    const retained =
      w &&
      span &&
      h.kind === "neutral" &&
      d > w.inner - 3 &&
      d < w.outer + 3 &&
      Math.abs(angleDiff(angle(w.center, p), span.center)) <
        (span.end - span.start) / 2 + radians(config.hysteresis);
    s.hover =
      h.kind === "bridge"
        ? s.path.slice(0, h.level)
        : retained
          ? previous
          : null;
    return;
  }
  if (s.guards[h.level]) {
    s.hover = null;
    s.timer = null;
    return;
  }
  const w = wheels[h.level],
    radial = distance(p, w.center),
    band = w.inner + config.band * (w.outer - w.inner);
  const outward = radial >= band && radial > distance(prev, w.center);
  if (h.level < s.path.length) {
    if (h.index === s.path[h.level]) {
      s.hover = path;
      s.timer = null;
      s.intentExit = null;
      s.lastInside = p;
      return;
    }
    if (s.effective === "concentric") {
      const pending = s.timer?.kind === "sibling" && same(s.timer.path, path);
      if (
        pending &&
        oldHit.kind === "item" &&
        oldHit.level === h.level &&
        oldHit.index === h.index &&
        outward
      ) {
        truncate(s, h.level);
        s.guards = {};
        s.hover = path;
        if (actionAt(s.menu, path!)?.children) open(s, path!, false);
        return;
      }
      schedule(s, "sibling", path!, now, config.grace);
      s.hover = path;
      return;
    }
    const child = wheels[h.level + 1];
    if (h.level + 2 === wheels.length) {
      if (
        !s.intentExit &&
        oldHit.kind === "item" &&
        oldHit.index === s.path[h.level] &&
        oldHit.level === h.level
      )
        s.intentExit = s.lastInside;
      if (
        s.intentExit &&
        inside(p, intentTriangle(s.intentExit, child)) &&
        distance(p, child.center) < distance(s.intentExit, child.center)
      ) {
        schedule(s, "intent", path!, now, config.grace);
        s.hover = s.path.slice(0, h.level + 1);
        return;
      }
    }
    truncate(s, h.level);
  }
  const changed = !same(s.hover, path);
  s.hover = path;
  s.lastInside = p;
  const action = actionAt(s.menu, path!);
  if (action?.children && action.state === "enabled") {
    const twoSamples =
      s.effective === "branching" ||
      (oldHit.kind === "item" &&
        oldHit.level === h.level &&
        oldHit.index === h.index);
    if (outward && twoSamples) open(s, path!, false);
    else if (changed || !s.timer)
      schedule(s, "dwell", path!, now, config.dwell);
  } else s.timer = null;
}
export function reduce(previous: State, event: Event, now: number): State {
  // All state is serializable. The reducer owns time; DOM code never mutates it.
  const s = structuredClone(previous);
  if (event.type === "configure")
    return initial(
      event.context ?? s.context,
      event.requested ?? s.requested,
      event.environment ?? s.environment,
      s.generation + 1,
    );
  if (event.type === "reset")
    return initial(s.context, s.requested, s.environment, s.generation + 1);
  if (event.type === "start") {
    const next = initial(
      s.context,
      s.requested,
      s.environment,
      s.generation + 1,
    );
    next.phase = "active";
    next.mode = event.mode ?? "invoke";
    next.pointer = event.point ?? anchor;
    next.summon = next.pointer;
    next.approach =
      next.effective === "concentric" && distance(next.pointer, anchor) > 0.5;
    next.inspectionAt = next.context === "checking" ? now + 1800 : null;
    next.result = next.approach
      ? "Move into the center hub to arm the wheel."
      : "Wheel open. Hover to explore; choose a leaf to see its simulated outcome.";
    return next;
  }
  if (event.type === "cancel") {
    if (s.phase === "active") cancel(s);
    return s;
  }
  if (s.phase !== "active") return s;
  if (event.type === "tick") {
    if (s.inspectionAt !== null && now >= s.inspectionAt) {
      s.inspectionAt = null;
      s.inspected = true;
      s.menu = menuFor(s.context, true);
      s.generation++;
      s.timer = null;
      s.press = null;
      if (s.effective === "concentric")
        s.guards = Object.fromEntries(
          geometry(s).map((_, i) => [i, s.pointer]),
        );
      s.result =
        "Simulated inspection finished. Preview is available. Move again or use the action list to choose it.";
    }
    const t = s.timer;
    if (t && now >= t.at) {
      s.timer = null;
      if (t.generation !== s.generation) return s;
      if (t.kind === "hub") {
        truncate(s, t.path.length);
        s.hover = null;
      } else if (t.kind === "dwell") {
        if (same(s.hover, t.path)) open(s, t.path, false);
      } else {
        truncate(s, t.path.length - 1);
        s.guards = {};
        s.hover = t.path;
        if (actionAt(s.menu, t.path)?.children) {
          if (t.kind === "sibling" && now >= t.entered + config.dwell)
            open(s, t.path, false);
          else schedule(s, "dwell", t.path, now, config.dwell);
        }
      }
    }
    return s;
  }
  if (event.type === "move") {
    move(s, event.point, now);
    return s;
  }
  if (event.type === "press") {
    move(s, event.point, now);
    const h = hitTest(event.point, geometry(s), s.effective),
      path = hitPath(s, h);
    s.press = {
      path,
      hub: h.kind === "hub",
      generation: s.generation,
      eligible: eligible(s, path),
    };
    return s;
  }
  if (event.type === "release") {
    // Fresh coordinates, but release is not a motion sample: cannot clear exposure or take over keyboard.
    const h = hitTest(event.point, geometry(s), s.effective),
      path = hitPath(s, h),
      press = s.press;
    s.press = null;
    const valid = eligible(s, path);
    if (s.mode === "drag") {
      if (valid && !actionAt(s.menu, path!)?.children)
        activate(s, path!, false);
      else cancel(s);
    } else if (
      h.kind === "outside" ||
      (h.kind === "hub" && h.level === 0 && (!press || press.hub))
    )
      cancel(s);
    else if (h.kind === "hub" && h.level > 0) {
      truncate(s, h.level - 1);
      s.hover = null;
    } else if (
      path &&
      actionAt(s.menu, path)?.children &&
      s.moved &&
      !s.approach &&
      !s.guards[path.length - 1] &&
      (s.effective === "branching" ||
        press?.hub ||
        (same(press?.path ?? null, path) && press?.generation === s.generation))
    ) {
      open(s, path, false);
    } else if (valid && path) {
      const action = actionAt(s.menu, path)!;
      if (action.children) open(s, path, false);
      else if (
        press?.hub ||
        (press?.eligible &&
          press.generation === s.generation &&
          same(press.path, path))
      )
        activate(s, path, false);
    }
    return s;
  }
  if (event.type === "back") {
    if (s.path.length) {
      const old = s.path;
      truncate(s, s.path.length - 1);
      s.hover = old;
    }
    return s;
  }
  if (event.type === "activate" || event.type === "highlight") {
    // Accessible list is the same state machine, restricted to currently open levels.
    if (
      event.path.length !== s.path.length + 1 ||
      !same(event.path.slice(0, -1), s.path)
    )
      return s;
    s.keyboard = s.pointer;
    s.timer = null;
    s.hover = event.path;
    if (event.type === "activate") activate(s, event.path, true);
    return s;
  }
  if (event.type === "key") {
    s.keyboard = s.pointer;
    s.timer = null;
    s.intentExit = null;
    const items = itemsAt(s.menu, s.path),
      current =
        s.hover?.length === s.path.length + 1 ? s.hover.at(-1) : undefined;
    if (event.key === "Escape") cancel(s);
    else if (event.key === "Backspace") return reduce(s, { type: "back" }, now);
    else if (
      ["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp"].includes(event.key)
    ) {
      const next = ["ArrowRight", "ArrowDown"].includes(event.key);
      s.hover = [
        ...s.path,
        current === undefined
          ? next
            ? 0
            : items.length - 1
          : (current + (next ? 1 : items.length - 1)) % items.length,
      ];
    } else if (
      (event.key === "Enter" || event.key === " ") &&
      current !== undefined
    )
      activate(s, [...s.path, current], true);
    else if (/^[1-9]$/.test(event.key) && Number(event.key) <= items.length)
      activate(s, [...s.path, Number(event.key) - 1], true);
  }
  return s;
}
export function nextDeadline(s: State): number | null {
  const deadlines = [s.timer?.at, s.inspectionAt].filter(
    (v): v is number => v !== null && v !== undefined,
  );
  return s.phase === "active" && deadlines.length
    ? Math.min(...deadlines)
    : null;
}
