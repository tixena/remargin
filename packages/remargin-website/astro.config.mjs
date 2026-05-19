import { defineConfig } from 'astro/config';

export default defineConfig({
  site: 'https://tixena.github.io',
  base: '/remargin',
  output: 'static',
  trailingSlash: 'ignore',
  build: { format: 'directory' },
});
