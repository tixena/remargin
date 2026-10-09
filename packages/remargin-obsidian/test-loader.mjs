/**
 * Node ESM loader for the `.test.ts` files: rewrites the `@/` path alias, resolves
 * extensionless specifiers, transpiles `.tsx` and the `.ts` syntax node's type-stripper
 * rejects through esbuild, stubs the `obsidian` module and empties CSS imports.
 */

import { readFile, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import esbuild from "esbuild";

const OBSIDIAN_STUB_URL = new URL("./test-obsidian-stub.mjs", import.meta.url).href;
async function fileExists(url) {
  try {
    const info = await stat(fileURLToPath(url));
    // A directory must lose to its `/index.ts`, or node raises ERR_UNSUPPORTED_DIR_IMPORT.
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
  // The real `obsidian` package has an empty `main`; every import goes to the local stub.
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
  // CSS is bundle-only, so tests resolve it to an empty module.
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
  // Parameter-property constructors and `const enum` are syntax node's strip-only loader
  // rejects; a regex match on either sends the file through esbuild.
  if (url.endsWith(".ts") && url.startsWith("file:")) {
    const source = await readFile(new URL(url), "utf8");
    const hasParamProps = /constructor\s*\([^)]*(private|public|protected|readonly)\b/.test(source);
    const hasConstEnum = /\bconst\s+enum\s+\w/.test(source);
    if (hasParamProps || hasConstEnum) {
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
