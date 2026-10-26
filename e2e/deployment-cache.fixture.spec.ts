// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type APIResponse } from "@playwright/test";
import { readFileSync } from "node:fs";

// These ask the actual Worker/static binding. No API payload or browser
// asset request is intercepted: an old shell naming unavailable code fails.
test.skip(Boolean(process.env.E2E_PAGES), "the static host has no version-bound Worker shell");

const policy = readFileSync("web/_headers", "utf8").match(/Content-Security-Policy: (.+)/)![1];
function security(response: APIResponse) {
  expect(response.headers()["x-content-type-options"]).toBe("nosniff");
  expect(response.headers()["referrer-policy"]).toBe("strict-origin-when-cross-origin");
  expect(response.headers()["permissions-policy"]).toBe("camera=(), microphone=(), geolocation=(), payment=(), usb=()");
  expect(response.headers()["content-security-policy"]).toBe(policy);
}

test("document routes serve the same current no-store shell and retain its security policy", async ({ request }) => {
  const version = await request.get("/version");
  expect(version.status()).toBe(200);
  const commit = (await version.json()).commit;
  let shell: string | undefined;
  for (const path of ["/", "/index.html", "/library?tab=seen", "/u/reader.eth/followers", "/unknown-document-route"]) {
    const response = await request.get(path, { headers: { accept: "text/html" }, maxRedirects: 0 });
    expect(response.status(), path).toBe(200);
    expect(response.headers()["content-type"], path).toContain("text/html");
    expect(response.headers()["cache-control"], path).toBe("no-store");
    expect(response.headers()["cdn-cache-control"], path).toBe("no-store");
    expect(response.headers()["x-allthing-commit"], path).toBe(commit);
    expect(response.headers()["etag"], path).toBeUndefined();
    expect(response.headers()["set-cookie"], path).toBeUndefined();
    security(response);
    const html = await response.text();
    expect(html, path).toContain('id="boot"');
    if (shell === undefined) shell = html; else expect(html, path).toBe(shell);
  }
  const head = await request.head("/library", { headers: { "if-none-match": '"old-shell"', range: "bytes=0-0", authorization: "Bearer invalid-fixture-only" } });
  expect(head.status()).toBe(200);
  expect(head.headers()["cache-control"]).toBe("no-store");
  expect(head.headers()["x-allthing-commit"]).toBe(commit);
  expect((await head.body()).byteLength).toBe(0);
  security(head);
});

test("every entry asset named by the shell exists with its real type and immutable cache policy", async ({ request }) => {
  const shell = await request.get("/");
  expect(shell.status()).toBe(200);
  const html = await shell.text();
  const paths = [...new Set([...html.matchAll(/(?:href|src)="(\/(?:assets|wasm)\/[^"?]+)"/g)].map(match => match[1]))];
  expect(paths.some(path => /\/boot-[0-9a-f]+\.js$/.test(path))).toBe(true);
  expect(paths.some(path => /\/paint-[0-9a-f]+\.js$/.test(path))).toBe(true);
  expect(paths.some(path => /\/allthing-[0-9a-f]+\/allthing\.js$/.test(path))).toBe(true);
  expect(paths.some(path => /\/allthing-[0-9a-f]+\.wasm$/.test(path))).toBe(true);
  for (const path of paths) {
    const response = await request.head(path, { headers: { "accept-encoding": "identity" } });
    expect(response.status(), path).toBe(200);
    expect(response.headers()["cache-control"], path).toContain("immutable");
    const type = response.headers()["content-type"] ?? "";
    expect(type, path).not.toContain("text/html");
    if (path.endsWith(".js")) expect(type, path).toMatch(/(?:javascript|ecmascript)/);
    if (path.endsWith(".css")) expect(type, path).toContain("text/css");
    if (path.endsWith(".wasm")) expect(type, path).toContain("application/wasm");
  }
});

test("missing deployment assets return non-cacheable real 404s even for document navigation", async ({ request }) => {
  for (const path of [
    "/assets/allthing-deadbeef00/allthing.js", "/assets/allthing-deadbeef00/allthing-split.js",
    "/assets/boot-deadbeef00.js", "/assets/paint-deadbeef00.js", "/assets/allthing-deadbeef00.css",
    "/assets/allthing-deadbeef00/split_library.wasm", "/assets/allthing-deadbeef00.wasm", "/wasm/allthing-deadbeef00.wasm",
  ]) {
    const response = await request.get(path, { headers: { accept: "text/html", "sec-fetch-mode": "navigate" } });
    expect(response.status(), path).toBe(404);
    expect(response.headers()["content-type"], path).not.toContain("text/html");
    expect(response.headers()["cache-control"], path).toBe("no-store");
    expect(response.headers()["cdn-cache-control"], path).toBe("no-store");
    expect(await response.json(), path).toEqual({ error: "Not found" });
    security(response);
  }
});
