import { defineConfig } from 'astro/config';

// Served at the apex of a custom domain, so `base` is "/". Every asset goes
// through `asset()` (src/lib/url.ts), which reads `import.meta.env.BASE_URL`,
// so all asset paths stay relative to this base — no domain hardcoded in them.
export default defineConfig({
  site: 'https://remargin.io',
  base: '/',
  output: 'static',
  trailingSlash: 'ignore',
  build: { format: 'directory' },
});
