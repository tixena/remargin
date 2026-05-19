// @ts-check
import { defineConfig } from 'astro/config';

// Deployed at https://tixena.github.io/remargin/
// The `base` value MUST match the GitHub Pages path prefix.
// All internal links should respect `import.meta.env.BASE_URL`.
export default defineConfig({
  site: 'https://tixena.github.io',
  base: '/remargin',
  output: 'static',
  trailingSlash: 'ignore',
  build: {
    format: 'directory',
  },
});
