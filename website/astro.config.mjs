// @ts-check
import { defineConfig } from "astro/config";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  site: process.env.SITE_ORIGIN || "https://marawan6569.github.io",
  base: process.env.SITE_BASE || "/Radivine/",
  output: "static",
  trailingSlash: "always",
  vite: { plugins: [tailwindcss()] },
});
