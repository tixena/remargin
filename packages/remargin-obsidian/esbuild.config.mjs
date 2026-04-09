import esbuild from "esbuild";
import postcss from "postcss";
import tailwindcss from "tailwindcss";
import autoprefixer from "autoprefixer";
import { readFile } from "fs/promises";
import { resolve, dirname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const prod = process.argv[2] === "production";

/** @type {import('esbuild').Plugin} */
const inlineCssPlugin = {
  name: "inline-css",
  setup(build) {
    build.onResolve({ filter: /\.css$/ }, (args) => ({
      path: resolve(dirname(args.importer), args.path),
      namespace: "inline-css",
    }));
    build.onLoad({ filter: /.*/, namespace: "inline-css" }, async (args) => {
      const css = await readFile(args.path, "utf8");
      const result = await postcss([tailwindcss, autoprefixer]).process(css, {
        from: args.path,
      });
      return {
        contents: `
          (function() {
            var style = document.createElement('style');
            style.setAttribute('data-remargin', '');
            style.textContent = ${JSON.stringify(result.css)};
            document.head.appendChild(style);
          })();
        `,
        loader: "js",
      };
    });
  },
};

const context = await esbuild.context({
  entryPoints: [resolve(__dirname, "src/main.ts")],
  bundle: true,
  external: [
    "obsidian",
    "electron",
    "@codemirror/autocomplete",
    "@codemirror/collab",
    "@codemirror/commands",
    "@codemirror/language",
    "@codemirror/lint",
    "@codemirror/search",
    "@codemirror/state",
    "@codemirror/view",
    "@lezer/common",
    "@lezer/highlight",
    "@lezer/lr",
  ],
  format: "cjs",
  target: "es2018",
  outfile: resolve(__dirname, "main.js"),
  platform: "browser",
  sourcemap: prod ? false : "inline",
  minify: prod,
  plugins: [inlineCssPlugin],
  jsx: "automatic",
  jsxImportSource: "react",
  alias: {
    "@": resolve(__dirname, "src"),
  },
  logLevel: "info",
});

if (prod) {
  await context.rebuild();
  await context.dispose();
} else {
  await context.watch();
}
