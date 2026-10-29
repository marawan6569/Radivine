/** Internal paths are relative to Astro's configured, case-sensitive base. */
export function withBase(path = ""): string {
  return `${import.meta.env.BASE_URL.replace(/\/$/, "")}/${path.replace(/^\/+/, "")}`;
}
