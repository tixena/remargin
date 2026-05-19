# remargin-website

Public landing page for [Remargin](https://github.com/tixena/remargin), deployed via GitHub Pages at <https://tixena.github.io/remargin/>.

## Status

Scaffold only. The current `src/pages/index.astro` is a placeholder used to verify the build & deploy pipeline. The real design is produced via Claude Design — see the brief at `personal/remargin/demo/05-claude-design-prompt.md` in the `eburgos_notes` vault.

## Local development

This package is a member of the pnpm workspace rooted at `packages/`. Install once from the workspace root:

```bash
cd packages
pnpm install
```

Then:

```bash
# Dev server (http://localhost:4321/remargin/)
pnpm --filter remargin-website dev

# Production build (outputs to dist/)
pnpm --filter remargin-website build

# Preview the production build
pnpm --filter remargin-website preview

# Type-check
pnpm --filter remargin-website check

# Lint with biome
pnpm --filter remargin-website lint
```

## Deployment

Deployment is automated via GitHub Actions. Pushing to `master` with changes under `packages/remargin-website/**` (or the workflow file itself) triggers `.github/workflows/pages.yml`, which:

1. Installs the pnpm workspace
2. Builds `remargin-website`
3. Uploads `packages/remargin-website/dist/` as a GitHub Pages artifact
4. Deploys to the `github-pages` environment

Manual deploys: trigger the workflow via the Actions tab → "Deploy site" → "Run workflow".

### One-time repo settings

Before the first deploy works:

1. Repo settings → Pages → Source: **GitHub Actions**.
2. Repo settings → Actions → Workflow permissions: ensure `Read and write` is enabled (or rely on the per-workflow `permissions:` block declared in the YAML).

### Base path

The site is served from `/remargin/` (not the root). `astro.config.mjs` sets:

```js
site: 'https://tixena.github.io',
base: '/remargin',
```

All internal links and asset references must respect `import.meta.env.BASE_URL`. Do **not** hard-code `/`.

## Assets to add later

- `public/casts/multi-agent-thread.cast` — asciinema recording of the live multi-agent demo (planner → engineer → qa).
- `public/og.png` — Open Graph image, 1200×630, rendered from the logo + tagline.
- `public/favicon.png` — 32×32 PNG (the SVG favicon is already wired).
- `public/remargin-logo.svg` — already wired in the placeholder; copy from `packages/remargin-obsidian/src/assets/remargin-logo.svg`.

## License

MIT, same as the rest of the repo. Made by [Tixena Labs](https://tixenalabs.com/).
