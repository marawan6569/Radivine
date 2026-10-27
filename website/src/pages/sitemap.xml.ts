import type { APIRoute } from "astro";
import { withBase } from "../lib/urls";
import { routes } from "../data/routes";
export const GET: APIRoute = ({ site }) =>
  new Response(
    `<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${routes.map((path) => `<url><loc>${new URL(withBase(path), site).href}</loc></url>`).join("")}</urlset>`,
    { headers: { "Content-Type": "application/xml" } },
  );
