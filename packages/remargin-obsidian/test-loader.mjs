import { access, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import esbuild from "esbuild";

/**
 * Node ESM loader used by `.test.ts` files that need to import `.tsx`
 * components (component tests).
 *
 * Handles three concerns node's built-in loader does not:
 *
 *   1. Rewrites the `@/` tsconfig path alias to `./src/`.
 *   2. Resolves extensionless specifiers by probing `.ts`, `.tsx`,
 *      `/index.ts`, and `/index.tsx`.
 *   3. Transpiles `.tsx` sources via esbuild (JSX + types) so node's
 *      experimental type-stripper never sees JSX.
 */
async function fileExists(url) {
  try {
    await access(fileURLToPath(url));
    return true;
  } catch {
    return false;
  }
}

async function resolveWithExtensions(url) {
  if (await fileExists(url)) return url.href;
  for (const suffix of [".ts", ".tsx", "/index.ts", "/index.tsx"]) {
    const candidate = new URL(url.href + suffix);
    if (await fileExists(candidate)) return candidate.href;
  }
  return null;
}

export async function resolve(specifier, context, nextResolve) {
  let rewritten = specifier;
  if (rewritten.startsWith("@/")) {
    rewritten = new URL(`./src/${rewritten.slice(2)}`, import.meta.url).href;
  }
  if (rewritten.startsWith("./") || rewritten.startsWith("../") || rewritten.startsWith("file:")) {
    const base = rewritten.startsWith("file:")
      ? new URL(rewritten)
      : new URL(rewritten, context.parentURL ?? import.meta.url);
    const resolved = await resolveWithExtensions(base);
    if (resolved) {
      return nextResolve(resolved, context);
    }
  }
  return nextResolve(rewritten, context);
}

export async function load(url, context, nextLoad) {
  if (url.endsWith(".tsx")) {
    const source = await readFile(new URL(url), "utf8");
    const { code } = await esbuild.transform(source, {
      loader: "tsx",
      format: "esm",
      jsx: "automatic",
      target: "es2020",
    });
    return { format: "module", shortCircuit: true, source: code };
  }
  return nextLoad(url, context);
}
