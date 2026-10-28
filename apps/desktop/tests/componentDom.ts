// Rendering the app's React components in tests: a happy-dom window as the
// globals, and Node module hooks that resolve the app's extensionless
// imports (as Vite does), load its .ts/.tsx through TypeScript's transpiler
// with an empty import.meta.env, and stylesheets as nothing. Import this
// first, then the components with `await import(...)`.
import { existsSync, readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import ts from "typescript";
import { Window } from "happy-dom";

registerHooks({
  resolve(spec, context, next) {
    if (spec.startsWith(".") && context.parentURL?.includes("/src/") && !/\.[cm]?[jt]sx?$|\.css$/.test(spec)) {
      for (const ext of [".ts", ".tsx"]) {
        const url = new URL(spec + ext, context.parentURL);
        if (existsSync(url)) return { url: url.href, shortCircuit: true };
      }
    }
    return next(spec, context);
  },
  load(url, context, next) {
    if (url.endsWith(".css")) return { format: "module", source: "", shortCircuit: true };
    if (url.includes("/src/") && /\.tsx?$/.test(url)) {
      // Vite's import.meta.env: a production build off the mock backend.
      const src = readFileSync(new URL(url), "utf8").replaceAll("import.meta.env", "({})");
      const out = ts.transpileModule(src, {
        compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022, verbatimModuleSyntax: true },
        fileName: url,
      });
      return { format: "module", source: out.outputText, shortCircuit: true };
    }
    return next(url, context);
  },
});

export const win = new Window({ url: "http://localhost/" });
Object.assign(globalThis, {
  window: win,
  document: win.document,
  HTMLElement: win.HTMLElement,
  Element: win.Element,
  Node: win.Node,
  DOMRect: win.DOMRect,
  MouseEvent: win.MouseEvent,
  KeyboardEvent: win.KeyboardEvent,
  IS_REACT_ACT_ENVIRONMENT: true,
});
