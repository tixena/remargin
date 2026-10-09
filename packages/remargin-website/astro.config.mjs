/** Astro configuration for the remargin website. */

import { defineConfig } from "astro/config";

// Served at the apex of a custom domain, so `base` is "/". Every asset path goes through
// `asset()` (src/lib/url.ts), which reads the base, so no domain is hardcoded in them.
export default defineConfig({
  site: "https://remargin.io",
  base: "/",
  output: "static",
  trailingSlash: "ignore",
  build: { format: "directory" },
});
