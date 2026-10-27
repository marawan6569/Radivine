/** Progressive enhancement only: no content depends on this module executing. */
const reduce = matchMedia("(prefers-reduced-motion: reduce)");
const desktop = matchMedia("(min-width: 52rem) and (min-height: 38rem)");
const finePointer = matchMedia("(hover: hover) and (pointer: fine)");
let dispose = () => {};

function setupMotion() {
  dispose();
  if (reduce.matches) return;
  const cleanups: (() => void)[] = [];
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("is-visible");
        observer.unobserve(entry.target);
      }
    },
    { threshold: 0.2 },
  );
  document
    .querySelectorAll("[data-reveal]")
    .forEach((node) => observer.observe(node));
  cleanups.push(() => {
    observer.disconnect();
    document
      .querySelectorAll(".is-visible")
      .forEach((node) => node.classList.remove("is-visible"));
  });

  const hero = document.querySelector<HTMLElement>(".hero");
  const art = document.querySelector<HTMLElement>("[data-pointer-art]");
  if (hero && art && desktop.matches && finePointer.matches) {
    let frame = 0;
    const move = (event: PointerEvent) => {
      if (event.pointerType !== "mouse") return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const rect = hero.getBoundingClientRect();
        art.style.setProperty(
          "--pointer-x",
          `${((event.clientX - rect.left) / rect.width - 0.5) * 16}px`,
        );
        art.style.setProperty(
          "--pointer-y",
          `${((event.clientY - rect.top) / rect.height - 0.5) * 12}px`,
        );
      });
    };
    const reset = () => {
      cancelAnimationFrame(frame);
      art.style.removeProperty("--pointer-x");
      art.style.removeProperty("--pointer-y");
    };
    hero.addEventListener("pointermove", move, { passive: true });
    hero.addEventListener("pointerleave", reset);
    cleanups.push(() => {
      reset();
      hero.removeEventListener("pointermove", move);
      hero.removeEventListener("pointerleave", reset);
    });
  }

  const story = document.querySelector<HTMLElement>("[data-story]");
  const steps = [
    ...document.querySelectorAll<HTMLElement>("[data-story-step]"),
  ];
  if (story && steps.length && desktop.matches) {
    let frame = 0;
    const update = () => {
      frame = 0;
      const center = innerHeight * 0.5;
      let nearest = 0;
      let distance = Infinity;
      steps.forEach((step, index) => {
        const rect = step.getBoundingClientRect();
        const delta = Math.abs(rect.top + rect.height / 2 - center);
        if (delta < distance) {
          distance = delta;
          nearest = index;
        }
      });
      story.dataset.step = String(nearest);
      steps.forEach((step, index) =>
        step.classList.toggle("is-current", index === nearest),
      );
      const first = steps[0].getBoundingClientRect();
      const last = steps[steps.length - 1].getBoundingClientRect();
      const progress = Math.max(
        0,
        Math.min(
          1,
          (center - first.top - first.height / 2) / (last.top - first.top),
        ),
      );
      story.style.setProperty("--story-progress", String(progress));
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(update);
    };
    const visibility = new IntersectionObserver(([entry]) => {
      window.removeEventListener("scroll", schedule);
      if (entry.isIntersecting) {
        update();
        window.addEventListener("scroll", schedule, { passive: true });
      }
    });
    visibility.observe(story);
    window.addEventListener("resize", schedule, { passive: true });
    cleanups.push(() => {
      visibility.disconnect();
      cancelAnimationFrame(frame);
      window.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      delete story.dataset.step;
      story.style.removeProperty("--story-progress");
      steps.forEach((step) => step.classList.remove("is-current"));
    });
  }
  dispose = () => {
    cleanups.forEach((cleanup) => cleanup());
    dispose = () => {};
  };
}
setupMotion();
[reduce, desktop, finePointer].forEach((query) =>
  query.addEventListener("change", setupMotion),
);
window.addEventListener("pagehide", () => dispose());
window.addEventListener("pageshow", (event) => {
  if (event.persisted) setupMotion();
});
