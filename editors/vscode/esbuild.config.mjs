import { build, context } from "esbuild";

const watch = process.argv.includes("--watch");

const options = {
  entryPoints: ["src/extension.ts"],
  bundle: true,
  outfile: "out/extension.js",
  // `vscode` is always external (the runtime injects it).
  // `vscode-languageclient` and its transitive protocol/jsonrpc
  // stack are externalized so the Phase V1 LSP scaffold does not
  // quadruple the bundle size. The VSIX packager still ships them
  // as runtime `node_modules` alongside `out/extension.js`, which
  // is the standard pattern for VS Code extensions that depend on
  // vscode-languageclient (see the official MS samples). This
  // trades a slightly bigger VSIX for a much smaller parsed JS
  // footprint on every activation.
  external: [
    "vscode",
    "vscode-languageclient",
    "vscode-languageclient/node",
    "vscode-languageclient/node.js",
    "vscode-languageserver-protocol",
    "vscode-jsonrpc",
    "semver",
    "minimatch",
  ],
  format: "cjs",
  platform: "node",
  target: "node18",
  sourcemap: true,
  minify: !watch,
  logLevel: "info",
};

if (watch) {
  const ctx = await context(options);
  await ctx.watch();
  console.log("esbuild: watching...");
} else {
  await build(options);
}
