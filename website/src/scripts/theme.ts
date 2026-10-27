type Preference = "system" | "light" | "dark";
const key = "radivine:theme";
const root = document.documentElement;
const select = document.querySelector<HTMLSelectElement>("#theme");
const media = window.matchMedia("(prefers-color-scheme: dark)");
const normalize = (value: string | null | undefined): Preference =>
  value === "light" || value === "dark" ? value : "system";
let preference = normalize(root.dataset.preference);
function apply(value: Preference) {
  preference = value;
  root.dataset.preference = value;
  root.dataset.theme =
    value === "system" ? (media.matches ? "dark" : "light") : value;
  root.style.colorScheme = root.dataset.theme;
  if (select) select.value = value;
}
apply(preference);
if (select) {
  select.closest<HTMLElement>(".theme-control")?.removeAttribute("hidden");
  select.addEventListener("change", () => {
    apply(normalize(select.value));
    try {
      localStorage.setItem(key, preference);
    } catch {
      /* Session selection still works. */
    }
  });
}
media.addEventListener("change", () => {
  if (preference === "system") apply("system");
});
window.addEventListener("storage", (event) => {
  if (event.key === key || event.key === null) apply(normalize(event.newValue));
});
