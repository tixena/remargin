import { readFile, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import esbuild from "esbuild";

/**
 * Node ESM loader used by `.test.ts` files that need to import `.tsx`
 * components (component tests).
 *
 * Handles four concerns node's built-in loader does not:
 *
 *   1. Rewrites the `@/` tsconfig path alias to `./src/`.
 *   2. Resolves extensionless specifiers by probing `.ts`, `.tsx`,
 *      `/index.ts`, and `/index.tsx`.
 *   3. Transpiles `.tsx` sources via esbuild (JSX + types) so node's
 *      experimental type-stripper never sees JSX.
 *   4. Stubs out the `obsidian` module — the npm package only ships
 *      type declarations, so any component that imports from it would
 *      crash at test time. The stub exposes the surface area component
 *      code touches today (`setIcon`, `MarkdownRenderer`, etc.) as
 *      no-ops, which is the canonical pattern the T36 ticket cites for
 *      headless verification.
 */

const OBSIDIAN_STUB_URL = new URL("./test-obsidian-stub.mjs", import.meta.url).href;
async function fileExists(url) {
  try {
    const info = await stat(fileURLToPath(url));
    // Reject directories — `./backend` resolves to a directory under
    // src/, but only the `/index.ts` candidate should win. Accepting
    // the bare directory short-circuits the extension probe and yields
    // ERR_UNSUPPORTED_DIR_IMPORT downstream.
    return info.isFile();
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
  // Intercept the `obsidian` module: redirect every import to the local
  // stub so component code can import named exports without the test
  // process exploding when the real package's empty `main` is hit.
  if (specifier === "obsidian") {
    return nextResolve(OBSIDIAN_STUB_URL, context);
  }
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
  // CSS imports are bundle-only — esbuild handles them at production
  // time. Tests just need the import to resolve to an empty module so
  // `main.ts`'s side-effect import (`./styles/globals.css`) does not
  // crash the loader.
  if (url.endsWith(".css")) {
    return { format: "module", shortCircuit: true, source: "export default '';" };
  }
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
  // Selectively transpile `.ts` files that use parameter-property
  // constructor syntax — node's strip-only TS loader rejects them.
  // Cheapest signal short of parsing: a `constructor(...)` declaration
  // with a `private` / `public` / `protected` / `readonly` parameter.
  if (url.endsWith(".ts") && url.startsWith("file:")) {
    const source = await readFile(new URL(url), "utf8");
    if (/constructor\s*\([^)]*(private|public|protected|readonly)\b/.test(source)) {
      const { code } = await esbuild.transform(source, {
        loader: "ts",
        format: "esm",
        target: "es2020",
      });
      return { format: "module", shortCircuit: true, source: code };
    }
  }
  return nextLoad(url, context);
}
