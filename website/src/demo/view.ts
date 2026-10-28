import { actionAt, contexts, itemsAt } from "./contexts.ts";
import { canSelect, geometry, type State } from "./model.ts";
import { angle, itemPoint, polar, sectorPath } from "./geometry.ts";
const ns = "http://www.w3.org/2000/svg";
function svg<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number>,
  text?: string,
) {
  const el = document.createElementNS(ns, tag);
  for (const [key, value] of Object.entries(attrs))
    el.setAttribute(key, String(value));
  if (text) el.textContent = text;
  return el;
}
export function createView(root: HTMLElement) {
  const get = <T extends HTMLElement = HTMLElement>(selector: string) =>
    root.querySelector<T>(selector)!;
  const layers = root.querySelector<SVGGElement>("[data-wheel-layers]")!;
  const list = get<HTMLOListElement>("[data-action-list]");
  let listKey = "",
    previousSession = "";
  return (s: State) => {
    root.dataset.phase = s.phase;
    const active = s.phase === "active",
      context = contexts.find((c) => c.id === s.context)!;
    get("[data-file-name]").textContent = context.file;
    get("[data-panel-file]").textContent = context.file;
    get("[data-context-detail]").textContent = context.detail;
    get("[data-launch]").hidden = active;
    get("[data-layout-label]").textContent =
      s.effective === "branching"
        ? "Branching / follow the stem"
        : "Concentric / move through the rings";
    get("[data-phase]").textContent = active
      ? s.mode === "drag"
        ? "DRAG PRACTICE · RELEASE TO CHOOSE"
        : "INVOKE PRACTICE · CLICK TO CHOOSE"
      : s.phase === "result"
        ? "SIMULATION COMPLETE"
        : s.phase === "cancelled"
          ? "SAFELY CANCELLED"
          : "READY WHEN YOU ARE";
    get("[data-fallback]").textContent =
      s.fallback ||
      (s.requested === "concentric"
        ? "Concentric Rings fit this simulated desktop. The session center stays fixed."
        : "Branching is the desktop application's default layout.");
    if (get("[data-outcome]").textContent !== s.result)
      get("[data-outcome]").textContent = s.result;
    get<HTMLButtonElement>("[data-back]").disabled = !active || !s.path.length;
    get<HTMLButtonElement>("[data-cancel]").disabled = !active;
    get<HTMLButtonElement>("[data-invoke]").textContent = active
      ? "Restart Invoke practice ↗"
      : s.phase === "idle"
        ? "Start Invoke practice ↗"
        : "Try again ↗";
    const crumbs = s.path.map(
      (_, i) => actionAt(s.menu, s.path.slice(0, i + 1))!.label,
    );
    get("[data-path]").textContent = ["Files", ...crumbs].join(" / ");
    const hovered = s.hover && actionAt(s.menu, s.hover);
    const armed = canSelect(s);
    get("[data-caption]").textContent = active
      ? s.approach
        ? "Enter the center hub to arm the wheel"
        : hovered
          ? `${hovered.label}${hovered.state !== "enabled" ? ` · ${hovered.reason}` : hovered.children ? " · follow this branch" : s.timer?.kind === "sibling" ? " · waiting for intent" : armed ? " · ready to choose" : " · move inside this action to choose"}`
          : "Hub, gaps and outside areas do not select an action."
      : s.phase === "result"
        ? "A direction chosen. See the simulated outcome →"
        : "Choose a sample. Follow a direction.";
    if (s.keyboard && hovered) {
      const spoken = `${hovered.label}. ${hovered.reason ?? (hovered.children ? "Submenu." : "Ready to choose.")}`;
      if (get("[data-key-status]").textContent !== spoken)
        get("[data-key-status]").textContent = spoken;
    }
    // Reconcile each open wheel by path; hover never rebuilds geometry or restarts animation.
    const session = `${s.context}:${s.effective}:${active}`;
    if (session !== previousSession) {
      layers.replaceChildren();
      previousSession = session;
    }
    const wheels = geometry(s),
      keys = new Set(wheels.map((w) => w.path.join(".") || "root"));
    for (const child of [...layers.children])
      if (
        !keys.has(child.getAttribute("data-wheel")!) &&
        !child.hasAttribute("data-ghost")
      ) {
        if (matchMedia("(prefers-reduced-motion: reduce)").matches || !active)
          child.remove();
        else {
          child.removeAttribute("data-wheel");
          child.setAttribute("data-ghost", "");
          child.classList.add("is-retracting");
          child.addEventListener("animationend", () => child.remove(), {
            once: true,
          });
        }
      }
    for (const [level, w] of wheels.entries()) {
      const key = w.path.join(".") || "root";
      let group = layers.querySelector<SVGGElement>(`[data-wheel="${key}"]`);
      if (!group) {
        group = svg("g", {
          "data-wheel": key,
          class: "practice-wheel",
          style: `transform-origin:${w.center.x}px ${w.center.y}px`,
        });
        if (level && s.effective === "branching") {
          const parent = wheels[level - 1],
            span = parent.spans[s.path[level - 1]];
          const a = polar(parent.center, parent.outer, span.center),
            b = polar(w.center, w.outer, angle(w.center, parent.center));
          const ctrl = polar(parent.center, parent.outer + 20, span.center);
          group.append(
            svg("path", {
              d: `M${a.x},${a.y} Q${ctrl.x},${ctrl.y} ${b.x},${b.y}`,
              class: "practice-stem",
            }),
          );
        }
        group.append(
          svg("circle", {
            cx: w.center.x,
            cy: w.center.y,
            r: w.outer,
            class: "practice-wheel-outline",
          }),
        );
        for (const [index, span] of w.spans.entries()) {
          const node = itemsAt(s.menu, w.path)[index],
            p = itemPoint(w, index);
          const segment = svg("g", {
            "data-segment": `${[...w.path, index].join(".")}`,
          });
          segment.append(
            svg("path", {
              d: sectorPath(w, span, s.effective),
              class: "practice-sector",
            }),
          );
          segment.append(
            svg(
              "text",
              { x: p.x, y: p.y - 7, class: "practice-number" },
              `${index + 1}${node.children ? " ↗" : ""}`,
            ),
          );
          segment.append(
            svg(
              "text",
              { x: p.x, y: p.y + 12, class: "practice-segment-label" },
              node.short,
            ),
          );
          segment.append(
            svg("text", { x: p.x, y: p.y + 29, class: "practice-state-label" }),
          );
          group.append(segment);
        }
        if (s.effective === "branching" || level === 0) {
          group.append(
            svg("circle", {
              cx: w.center.x,
              cy: w.center.y,
              r: w.inner - 5,
              class: "practice-hub",
            }),
          );
          group.append(
            svg(
              "text",
              { x: w.center.x, y: w.center.y + 6, class: "practice-hub-label" },
              level ? "←" : "◇",
            ),
          );
        }
        layers.append(group);
      }
      group.classList.toggle("is-ancestor", level < wheels.length - 1);
      for (const [index, node] of itemsAt(s.menu, w.path).entries()) {
        const path = [...w.path, index].join("."),
          segment = group.querySelector<SVGGElement>(
            `[data-segment="${path}"]`,
          )!;
        segment.classList.toggle(
          "is-hovered",
          active && path === s.hover?.join("."),
        );
        segment.classList.toggle(
          "is-open",
          path === s.path.slice(0, level + 1).join(".") &&
            level < s.path.length,
        );
        segment.classList.toggle("is-disabled", node.state !== "enabled");
        segment.classList.toggle(
          "is-retained",
          active &&
            path === s.hover?.join(".") &&
            !armed &&
            !node.children &&
            node.state === "enabled",
        );
        segment.querySelector(".practice-state-label")!.textContent =
          node.state === "pending"
            ? "Checking…"
            : node.state === "disabled"
              ? "Unavailable"
              : "";
      }
    }
    const nextListKey = `${s.context}:${s.path.join(".")}:${s.inspected}:${active}`;
    if (listKey !== nextListKey) {
      const hadFocus = list.contains(document.activeElement);
      listKey = nextListKey;
      const entries = itemsAt(s.menu, s.path).map((action, index) => {
        const li = document.createElement("li"),
          button = document.createElement("button");
        button.type = "button";
        button.dataset.action = String(index);
        button.setAttribute(
          "aria-disabled",
          String(!active || action.state !== "enabled"),
        );
        const number = document.createElement("span");
        number.className = "practice-action-number";
        number.textContent = String(index + 1);
        number.setAttribute("aria-hidden", "true");
        const body = document.createElement("span"),
          label = document.createElement("strong"),
          detail = document.createElement("small");
        label.textContent = action.label;
        detail.textContent =
          action.reason ??
          (action.children
            ? "Open submenu"
            : action.id.startsWith("custom.")
              ? "Logging-only example"
              : "Simulated outcome");
        body.append(label, detail);
        const arrow = document.createElement("span");
        arrow.textContent = action.children ? "↗" : "→";
        arrow.setAttribute("aria-hidden", "true");
        button.append(number, body, arrow);
        li.append(button);
        return li;
      });
      list.replaceChildren(...entries);
      if (hadFocus) {
        if (active)
          list.querySelector("button")?.focus({ preventScroll: true });
        else
          get<HTMLButtonElement>("[data-invoke]").focus({
            preventScroll: true,
          });
      }
    }
    for (const button of list.querySelectorAll<HTMLButtonElement>("button"))
      button.classList.toggle(
        "is-current",
        active &&
          s.hover?.length === s.path.length + 1 &&
          s.hover.at(-1) === Number(button.dataset.action),
      );
  };
}
