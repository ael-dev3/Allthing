#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Bundles web/stdb/bridge.ts, the SpacetimeDB TypeScript SDK and the
// module's generated bindings into one minified ES module for the browser,
// which `cargo xtask dist` fingerprints into dist/assets/stdb-<hash>.js.
// The app adds it only when a signed-in reader opens /messages.
//
//   node scripts/build-stdb.mjs <out.js>
import { build } from "esbuild";

const out = process.argv[2];
if (!out) {
  console.error("usage: node scripts/build-stdb.mjs <out.js>");
  process.exit(2);
}
await build({
  entryPoints: [new URL("../web/stdb/bridge.ts", import.meta.url).pathname],
  outfile: out,
  bundle: true,
  format: "esm",
  platform: "browser",
  target: ["es2022"],
  minify: true,
  legalComments: "eof",
  logLevel: "warning",
});
