// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("../scripts/find-wasm-opt.sh", import.meta.url));
const target = process.platform === "darwin"
  ? `${process.arch === "arm64" ? "arm64" : "x86_64"}-macos`
  : `${process.arch === "arm64" ? "aarch64" : "x86_64"}-linux`;
let scratch;
before(async () => { scratch = await mkdtemp(path.join(tmpdir(), "allthing-wasm-opt-")); });
after(async () => { await rm(scratch, { recursive: true, force: true }); });

async function binary(file, executable = true) {
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, "#!/bin/sh\nexit 0\n");
  if (executable) await chmod(file, 0o755);
  return file;
}
function find(roots, env = {}) {
  return execFileSync("/bin/bash", [script, ...roots], {
    encoding: "utf8", env: { ...process.env, PATH: "/usr/bin:/bin", WASM_OPT_BIN: "", ...env },
  }).trim();
}

test("honors an explicit optimizer path, including spaces", () => {
  const explicit = path.join(scratch, "chosen optimizer", "wasm-opt");
  assert.equal(find([], { WASM_OPT_BIN: explicit }), explicit);
});

test("uses the installed PATH optimizer before a cached one", async () => {
  const installed = await binary(path.join(scratch, "tools", "wasm-opt"));
  assert.equal(find([scratch], { PATH: `${path.dirname(installed)}:/usr/bin:/bin` }), installed);
});

test("finds a platform cache after an empty XDG root and skips other architectures or non-executables", async () => {
  const cache = path.join(scratch, "Library", "Caches with spaces");
  await binary(path.join(cache, "worker-build", "wasm-opt-wrong-platform-132", "bin", "wasm-opt"));
  await binary(path.join(cache, "worker-build", `wasm-opt-${target}-131`, "bin", "wasm-opt"), false);
  const wanted = await binary(path.join(cache, "worker-build", `wasm-opt-${target}-132`, "bin", "wasm-opt"));
  assert.equal(find([path.join(scratch, "empty-xdg"), cache]), wanted);
});

test("an absent optimizer remains optional", () => {
  assert.equal(find([path.join(scratch, "no-cache")]), "");
});
