import { initial, nextDeadline, reduce, type Event } from "./model.ts";
import type { ContextId } from "./contexts.ts";
import type { Environment, Layout, Point } from "./geometry.ts";
import { createView } from "./view.ts";
export function mount(root: HTMLElement) {
  const get = <T extends HTMLElement = HTMLElement>(selector: string) =>
    root.querySelector<T>(selector)!;
  const stage = get("[data-stage]"),
    svg = root.querySelector<SVGSVGElement>("[data-wheel-svg]")!;
  const render = createView(root);
  let state = initial(),
    deadline: ReturnType<typeof setTimeout> | undefined;
  let held: number | null = null,
    dragging = false,
    shiftReady = false,
    dragOrigin: Point | null = null,
    lastPoint: Point | null = null,
    visible = true;
  const abort = new AbortController(),
    options = { signal: abort.signal };
  const point = (event: PointerEvent): Point => {
    const matrix = svg.getScreenCTM();
    if (!matrix) return { x: NaN, y: NaN };
    const p = new DOMPoint(event.clientX, event.clientY).matrixTransform(
      matrix.inverse(),
    );
    return { x: p.x, y: p.y };
  };
  const releaseCapture = () => {
    const id = held;
    held = null;
    dragging = false;
    dragOrigin = null;
    lastPoint = null;
    root.removeAttribute("data-dragging");
    if (id !== null && stage.hasPointerCapture(id))
      stage.releasePointerCapture(id);
  };
  const send = (event: Event) => {
    clearTimeout(deadline);
    state = reduce(state, event, performance.now());
    render(state);
    root.dataset.session = String(state.generation); // QA-visible session boundary, no user data.
    if (state.phase !== "active" && event.type !== "move") releaseCapture();
    const next = nextDeadline(state);
    if (next !== null && visible)
      deadline = setTimeout(
        () => send({ type: "tick" }),
        Math.max(0, next - performance.now()),
      );
  };
  const start = () => {
    releaseCapture();
    send({ type: "start" });
    stage.focus({ preventScroll: true });
  };
  for (const selector of ["[data-open]", "[data-invoke]"])
    get(selector).addEventListener("click", start, options);
  get("[data-reset]").addEventListener(
    "click",
    () => {
      send({ type: "reset" });
      get("[data-open]").focus({ preventScroll: true });
    },
    options,
  );
  get("[data-cancel]").addEventListener(
    "click",
    () => {
      send({ type: "cancel" });
      get("[data-open]").focus({ preventScroll: true });
    },
    options,
  );
  get("[data-back]").addEventListener(
    "click",
    () => {
      send({ type: "back" });
      stage.focus({ preventScroll: true });
    },
    options,
  );
  get<HTMLSelectElement>("[data-context]").addEventListener(
    "change",
    (e) =>
      send({
        type: "configure",
        context: (e.target as HTMLSelectElement).value as ContextId,
      }),
    options,
  );
  for (const radio of root.querySelectorAll<HTMLInputElement>(
    '[name="wheel-layout"]',
  ))
    radio.addEventListener(
      "change",
      () => send({ type: "configure", requested: radio.value as Layout }),
      options,
    );
  get<HTMLSelectElement>("[data-environment]").addEventListener(
    "change",
    (e) =>
      send({
        type: "configure",
        environment: (e.target as HTMLSelectElement).value as Environment,
      }),
    options,
  );
  const list = get("[data-action-list]");
  list.addEventListener(
    "click",
    (e) => {
      const button = (e.target as Element).closest<HTMLButtonElement>(
        "[data-action]",
      );
      if (button)
        send({
          type: "activate",
          path: [...state.path, Number(button.dataset.action)],
        });
    },
    options,
  );
  list.addEventListener(
    "focusin",
    (e) => {
      const button = (e.target as Element).closest<HTMLButtonElement>(
        "[data-action]",
      );
      if (button)
        send({
          type: "highlight",
          path: [...state.path, Number(button.dataset.action)],
        });
    },
    options,
  );
  stage.addEventListener(
    "pointerdown",
    (e) => {
      if (e.button !== 0 || !e.isPrimary) return;
      const target = e.target as Element;
      if (target.closest("[data-drag]") && e.pointerType !== "touch") {
        e.preventDefault();
        held = e.pointerId;
        dragging = true;
        shiftReady = !e.shiftKey;
        dragOrigin = point(e);
        lastPoint = dragOrigin;
        stage.setPointerCapture(held);
        stage.focus({ preventScroll: true });
        root.dataset.dragging = "true";
        get("[data-caption]").textContent =
          "Keep holding. Move the sample, then press Shift to summon the wheel.";
        return;
      }
      if (target.closest("button") || state.phase !== "active") return;
      if (e.pointerType === "touch" && matchMedia("(max-width: 52rem)").matches)
        return;
      e.preventDefault();
      held = e.pointerId;
      stage.setPointerCapture(held);
      stage.focus({ preventScroll: true });
      send({ type: "press", point: point(e) });
    },
    options,
  );
  stage.addEventListener(
    "pointermove",
    (e) => {
      if (!e.isPrimary || (held !== null && held !== e.pointerId)) return;
      lastPoint = point(e);
      if (state.phase === "active") send({ type: "move", point: lastPoint });
    },
    options,
  );
  stage.addEventListener(
    "pointerup",
    (e) => {
      if (held !== e.pointerId) return;
      if (state.phase === "active") send({ type: "release", point: point(e) });
      releaseCapture();
    },
    options,
  );
  const cancel = () => {
    releaseCapture();
    send({ type: "cancel" });
  };
  stage.addEventListener("pointercancel", cancel, options);
  stage.addEventListener(
    "lostpointercapture",
    () => {
      if (held !== null) cancel();
    },
    options,
  );
  stage.addEventListener(
    "pointerleave",
    () => {
      if (held === null && state.phase === "active")
        send({ type: "move", point: { x: -1000, y: -1000 } });
    },
    options,
  );
  stage.addEventListener(
    "keydown",
    (e) => {
      if (e.target !== stage || e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "Shift" && !e.repeat && dragging && shiftReady) {
        if (state.phase === "active") cancel();
        else if (
          dragOrigin &&
          lastPoint &&
          Math.hypot(lastPoint.x - dragOrigin.x, lastPoint.y - dragOrigin.y) >=
            6
        )
          send({ type: "start", mode: "drag", point: lastPoint });
        return;
      }
      if (e.key === "Tab") {
        if (dragging) cancel();
        return;
      }
      if (
        state.phase === "active" &&
        [
          "ArrowUp",
          "ArrowDown",
          "ArrowLeft",
          "ArrowRight",
          "Enter",
          " ",
          "Backspace",
          "Escape",
          "1",
          "2",
          "3",
          "4",
          "5",
          "6",
          "7",
          "8",
          "9",
        ].includes(e.key)
      ) {
        e.preventDefault();
        send({ type: "key", key: e.key });
      }
    },
    options,
  );
  stage.addEventListener(
    "keyup",
    (e) => {
      if (e.key === "Shift") shiftReady = true;
    },
    options,
  );
  stage.addEventListener(
    "focusout",
    (e) => {
      if (held !== null && !stage.contains(e.relatedTarget as Node)) cancel();
    },
    options,
  );
  window.addEventListener(
    "blur",
    () => {
      if (held !== null) cancel();
    },
    options,
  );
  const visibility = () => {
    if (document.hidden) cancel();
  };
  document.addEventListener("visibilitychange", visibility, options);
  const reduced = matchMedia("(prefers-reduced-motion: reduce)");
  reduced.addEventListener(
    "change",
    () => {
      root.querySelectorAll("[data-ghost]").forEach((node) => node.remove());
    },
    options,
  );
  const observer = new IntersectionObserver((entries) => {
    visible = entries.some((e) => e.isIntersecting);
    root.dataset.visible = String(visible);
    if (!visible) cancel();
  });
  observer.observe(root);
  window.addEventListener(
    "pagehide",
    () => {
      cancel();
      observer.disconnect();
      abort.abort();
    },
    { once: true },
  );
  // bfcache restoration receives a fresh controller and a clean practice session.
  window.addEventListener(
    "pageshow",
    (e) => {
      if (e.persisted) mount(root);
    },
    { once: true },
  );
  get("[data-demo-interactive]").hidden = false;
  get("[data-demo-static]").hidden = true;
  render(state);
}
