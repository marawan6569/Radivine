import { test } from "node:test";
import assert from "node:assert/strict";
import {
  initial,
  reduce,
  geometry,
  nextDeadline,
  canSelect,
  type State,
} from "../src/demo/model.ts";
import { menuFor, itemsAt } from "../src/demo/contexts.ts";
import {
  anchor,
  config,
  childSpans,
  rootSpans,
  hitTest,
  itemPoint,
  polar,
  distance,
  selection,
  sectorPath,
  type Layout,
} from "../src/demo/geometry.ts";
const start = (
  layout: Layout = "branching",
  context: State["context"] = "text",
  mode: State["mode"] = "invoke",
) => reduce(initial(context, layout), { type: "start", mode }, 0);
const key = (s: State, key: string, t = 0) =>
  reduce(s, { type: "key", key }, t);
const move = (
  s: State,
  level: number,
  index: number,
  t: number,
  radius?: number,
) =>
  reduce(
    s,
    { type: "move", point: itemPoint(geometry(s)[level], index, radius) },
    t,
  );
const tick = (s: State, t: number) => reduce(s, { type: "tick" }, t);

test("application defaults and source menu snapshots (AC 4.1 / C3.1)", () => {
  assert.deepEqual(config.root, [40, 128]);
  assert.deepEqual(config.child, [34, 108]);
  assert.equal(config.dwell, 110);
  assert.equal(config.grace, 180);
  assert.deepEqual(
    menuFor("text").map((n) => n.id),
    [
      "radivine.preview",
      "radivine.copy",
      "radivine.properties",
      "radivine.custom",
    ],
  );
  assert.deepEqual(
    itemsAt(menuFor("image"), [0, 0]).map((n) => n.id),
    [
      "radivine.convert.image.jpeg",
      "radivine.convert.image.webp",
      "radivine.convert.image.advanced",
    ],
  );
  assert(
    itemsAt(menuFor("multiple"), [0]).every(
      (n) =>
        n.state === "disabled" &&
        n.reason === "Select a single file to preview",
    ),
  );
});
test("root top alignment, capped child arcs and reserved rings", () => {
  assert.equal(rootSpans(4)[0].center, -Math.PI / 2);
  const spans = childSpans(1, Math.PI / 2);
  assert(
    Math.abs(spans.spans[0].end - spans.spans[0].start - (72 * Math.PI) / 180) <
      1e-6,
  );
  let s = key(key(start("concentric", "image"), "1"), "1");
  assert.deepEqual(
    geometry(s).map((w) => [w.inner, w.outer]),
    [
      [40, 128],
      [134, 194],
      [200, 260],
    ],
  );
  assert(geometry(s).every((w) => distance(w.center, anchor) === 0));
  assert(
    !sectorPath(geometry(s)[1], geometry(s)[1].spans[0], "concentric").includes(
      "NaN",
    ),
  );
});
test("Branching placement rotates to fit, avoids ancestors and leaves neutral bridge", () => {
  const s = key(key(start("branching", "image"), "1"), "1"),
    wheels = geometry(s);
  for (const w of wheels) {
    assert(w.center.x >= w.outer + 8);
    assert(w.center.y >= w.outer + 8);
    assert(w.center.x + w.outer <= 892);
    assert(w.center.y + w.outer <= 792);
  }
  assert(
    distance(wheels[0].center, wheels[2].center) >=
      wheels[0].outer + wheels[2].outer + 6 - 1e-3,
  );
  const w = wheels[1];
  assert.equal(
    hitTest(polar(w.center, 70, w.bridge!.center), wheels, "branching").kind,
    "bridge",
  );
  assert(
    Math.abs(distance(wheels[0].center, w.center) - (128 + 20 + w.outer)) <
      1e-6,
  );
});
for (const layout of ["branching", "concentric"] as const) {
  test(`${layout}: dwell boundary, outer push, keyboard enter/back/digits`, () => {
    let s = move(start(layout), 0, 0, 0, 70);
    assert.deepEqual(s.path, []);
    assert.deepEqual(tick(s, 109).path, []);
    s = tick(s, 110);
    assert.deepEqual(s.path, [0]);
    s = key(s, "Backspace");
    assert.deepEqual(s.path, []);
    s = key(s, "1");
    assert.deepEqual(s.path, [0]);
    assert.deepEqual(s.hover, [0, 0]);
    s = key(s, "3");
    assert.equal(s.phase, "result");
    assert.match(s.result, /Tail/);
    let band = move(start(layout), 0, 0, 0, 70);
    band = move(band, 0, 0, 10, 110);
    assert.deepEqual(band.path, [0]);
  });
  test(`${layout}: leaf drag release, neutral rejection and stale coordinates`, () => {
    let s = move(start(layout, "text", "drag"), 0, 1, 10);
    assert.equal(s.phase, "active");
    assert.equal(
      reduce(s, { type: "release", point: itemPoint(geometry(s)[0], 1) }, 11)
        .phase,
      "result",
    );
    for (const point of [
      anchor,
      { x: 899, y: 799 },
      polar(anchor, 85, -Math.PI / 4),
    ])
      assert.equal(
        reduce(s, { type: "release", point }, 11).phase,
        "cancelled",
      );
    assert.equal(
      reduce(s, { type: "release", point: itemPoint(geometry(s)[0], 2) }, 11)
        .phase,
      "cancelled",
    );
  });
  test(`${layout}: Invoke pairs press/release and allows hub marking`, () => {
    let s = start(layout);
    const p = itemPoint(geometry(s)[0], 1);
    s = reduce(s, { type: "press", point: p }, 1);
    assert.equal(reduce(s, { type: "release", point: p }, 2).phase, "result");
    let wrong = reduce(
      s,
      { type: "move", point: itemPoint(geometry(s)[0], 2) },
      2,
    );
    assert.equal(
      reduce(wrong, { type: "release", point: itemPoint(geometry(s)[0], 2) }, 3)
        .phase,
      "active",
    );
    let mark = reduce(start(layout), { type: "press", point: anchor }, 0);
    mark = move(mark, 0, 1, 1);
    assert.equal(
      reduce(mark, { type: "release", point: p }, 2).phase,
      "result",
    );
  });
  test(`${layout}: disabled and pending actions cannot commit; reset invalidates timers`, () => {
    let disabled = key(key(start(layout, "multiple"), "1"), "1");
    assert.equal(disabled.phase, "active");
    assert.match(disabled.result, /Select a single file/);
    let pending = key(key(start(layout, "checking"), "1"), "1");
    assert.equal(pending.phase, "active");
    assert.match(pending.result, /Checking/);
    pending = tick(pending, 1800);
    assert.equal(pending.inspected, true);
    assert.equal(key(pending, "1").phase, "result");
    let reset = reduce(pending, { type: "configure", context: "image" }, 1801);
    assert.deepEqual(reset.path, []);
    assert.equal(nextDeadline(reset), null);
    assert.equal(tick(reset, 5000).phase, "idle");
  });
  test(`${layout}: hub dwell and cancellation clear all scheduling`, () => {
    let s = key(start(layout), "1");
    s = reduce(s, { type: "move", point: { x: anchor.x + 3, y: anchor.y } }, 0);
    // Keyboard takeover is fresh but initial slop still needs a real outward motion.
    s = move(s, 1, 0, 1);
    s = reduce(s, { type: "move", point: anchor }, 10);
    assert.deepEqual(tick(s, 159).path, [0]);
    assert.deepEqual(tick(s, 160).path, []);
    s = key(s, "Escape");
    assert.equal(s.phase, "cancelled");
    assert.equal(nextDeadline(s), null);
  });
}
test("Concentric: gap/boundary/outer-margin are strict neutral, retain hierarchy", () => {
  let s = key(start("concentric"), "1");
  s = move(s, 1, 1, 0);
  for (const r of [128, 131, 134, 194, 199]) {
    const point = polar(anchor, r, 0),
      moved = reduce(s, { type: "move", point }, 1);
    assert.notEqual(hitTest(point, geometry(s), "concentric").kind, "item");
    assert.deepEqual(moved.path, [0]);
  }
});
test("Concentric: ancestor sibling grace, jitter cancellation, explicit click, two-sample shortcut", () => {
  let s = key(start("concentric"), "1");
  s = move(s, 0, 3, 0, 70);
  assert.equal(s.timer?.kind, "sibling");
  assert.deepEqual(tick(s, 179).path, [0]);
  assert.deepEqual(tick(s, 180).path, [3]);
  const back = move(s, 0, 0, 50, 70);
  assert.equal(back.timer, null);
  assert.deepEqual(tick(back, 500).path, [0]);
  const pushed = move(s, 0, 3, 10, 110);
  assert.deepEqual(pushed.path, [3]);
  const p = itemPoint(geometry(s)[0], 3, 70);
  s = reduce(s, { type: "press", point: p }, 10);
  s = reduce(s, { type: "release", point: p }, 11);
  assert.deepEqual(s.path, [3]);
});
test("Concentric: new geometry needs 2px fresh motion; release does not clear guards", () => {
  let s = move(start("concentric"), 0, 0, 0, 70);
  s = tick(s, 110);
  const child = itemPoint(geometry(s)[1], 0);
  assert.equal(
    reduce(s, { type: "release", point: child }, 111).phase,
    "active",
  );
  assert(s.guards[1]);
  s = reduce(s, { type: "move", point: child }, 112);
  assert.equal(s.guards[1], undefined);
});
test("Concentric: displaced synthetic start requires hub entry, keyboard bypass stays local", () => {
  let s = reduce(
    initial("text", "concentric"),
    { type: "start", mode: "drag", point: { x: 100, y: 100 } },
    0,
  );
  s = move(s, 0, 1, 10);
  assert.equal(s.approach, true);
  assert.equal(s.hover, null);
  s = reduce(s, { type: "move", point: anchor }, 20);
  assert.equal(s.approach, false);
  s = move(s, 0, 1, 30);
  assert.deepEqual(s.hover, [1]);
});
test("geometry and context changes clear press, dwell and selection; Tab is untouched", () => {
  let s = move(start(), 0, 0, 0, 70);
  s = reduce(s, { type: "configure", requested: "concentric" }, 10);
  assert.equal(s.phase, "idle");
  assert.equal(s.hover, null);
  assert.equal(s.press, null);
  assert.equal(s.timer, null);
  assert.equal(key(start(), "Tab").phase, "active");
});
test("fallbacks preserve requested layout and choose Branching", () => {
  for (const environment of [
    "small",
    "uncomposited",
    "small-targets",
  ] as const) {
    assert.equal(selection("concentric", environment).effective, "branching");
    assert(selection("concentric", environment).fallback);
  }
  assert.equal(selection("concentric", "desktop").effective, "concentric");
});
test("conversion and custom outcomes never claim native execution", () => {
  let s = key(key(key(start("concentric", "image"), "1"), "1"), "2");
  assert.match(s.result, /WebP.*queued/);
  assert.match(s.result, /no file was converted/);
  s = key(key(start(), "4"), "1");
  assert.match(s.result, /logging-only/);
});

// These expectations were emitted by the unmodified Rust WheelSession, not this reducer.
import nativeTraces from "./fixtures/demo-native-traces.json" with { type: "json" };
import type { Event } from "../src/demo/model.ts";
import { angleDiff } from "../src/demo/geometry.ts";
for (const fixture of nativeTraces.cases) {
  test(`Rust differential trace: ${fixture.layout} / ${fixture.context} / ${fixture.name}`, () => {
    let s = start(
      fixture.layout as Layout,
      fixture.context as State["context"],
    );
    for (const step of fixture.trace) {
      s = reduce(s, step.input as Event, step.at);
      assert.deepEqual(s.path, step.path, `path at ${step.at}`);
      assert.deepEqual(s.hover, step.hover, `hover at ${step.at}`);
      if (step.commit) assert.equal(s.phase, "result");
      const wheels = geometry(s);
      assert.equal(wheels.length, step.wheels.length);
      for (const [i, expected] of step.wheels.entries()) {
        const actual = wheels[i];
        assert(
          distance(actual.center, expected.center) < 0.002,
          `center ${i} at ${step.at}`,
        );
        assert(Math.abs(actual.inner - expected.inner) < 0.002);
        assert(Math.abs(actual.outer - expected.outer) < 0.002);
        for (const [j, span] of expected.spans.entries()) {
          assert(
            Math.abs(angleDiff(actual.spans[j].center, span.center)) < 0.00001,
          );
          assert(
            Math.abs(actual.spans[j].end - actual.spans[j].start - span.width) <
              0.00001,
          );
        }
      }
    }
  });
}

test("Branching: angular hysteresis absorbs boundary jitter without accepting a separator release", () => {
  let s = move(start("branching", "text", "drag"), 0, 1, 0);
  s = reduce(
    s,
    { type: "move", point: polar(anchor, 85, -Math.PI / 4 - 0.02) },
    10,
  );
  assert.deepEqual(s.hover, [1]);
  assert.equal(
    reduce(s, { type: "release", point: polar(anchor, 85, -Math.PI / 4) }, 11)
      .phase,
    "cancelled",
  );
});
test("Branching: tangent intent protects diagonal travel only until the 180ms deadline", () => {
  let s = tick(move(start(), 0, 0, 0, 70), 110);
  s = reduce(
    s,
    { type: "move", point: polar(anchor, 45, (-55 * Math.PI) / 180) },
    120,
  );
  s = reduce(
    s,
    { type: "move", point: polar(anchor, 132, (-41 * Math.PI) / 180) },
    130,
  );
  assert.equal(s.timer?.kind, "intent");
  assert.deepEqual(s.hover, [0]);
  assert.deepEqual(tick(s, 309).path, [0]);
  assert.deepEqual(tick(s, 310).path, []);
});

test("retained hover never advertises a neutral zone as ready to select", () => {
  let s = move(start("concentric"), 0, 1, 0);
  assert.equal(canSelect(s), true);
  s = reduce(s, { type: "move", point: polar(anchor, 85, -Math.PI / 4) }, 10);
  assert.deepEqual(s.hover, [1]);
  assert.equal(canSelect(s), false);
});
