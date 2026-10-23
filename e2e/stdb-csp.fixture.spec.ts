// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";
import { buildSync } from "esbuild";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { createRequire } from "node:module";

test.skip(Boolean(process.env.E2E_PAGES), "signed-in Worker app");
const temp = mkdtempSync(join(tmpdir(), "allthing-stdb-wire-"));
buildSync({ entryPoints: [resolve("e2e/fixtures/stdb-wire.ts")], outfile: join(temp, "wire.cjs"), bundle: true, platform: "node", format: "cjs", logLevel: "silent" });
const wire = createRequire(resolve("package.json"))(join(temp, "wire.cjs"));
const policy = readFileSync(resolve("web/_headers"), "utf8").match(/Content-Security-Policy: (.+)/)![1];
test.afterAll(() => rmSync(temp, { recursive: true, force: true }));

test("the actual SpacetimeDB bundle connects, decodes rows and verifies/sends under the production CSP", async ({ page }) => {
  const errors: string[] = [], reducers: string[] = [];
  let attestations = 0;
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    document.cookie = `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid: 1234, username: "reader", display_name: "Reader" }))}; Path=/; Secure; SameSite=Lax`;
    (window as any).__cspFailures = [];
    document.addEventListener("securitypolicyviolation", (event) => (window as any).__cspFailures.push(`${event.effectiveDirective}:${event.blockedURI}`));
  });
  // Always enforce the exact checked-in policy, even on local asset servers.
  await page.route("**/messages*", async (route) => {
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), "content-security-policy": policy } });
  });
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture" } }));
  await page.route("**/api/v1/auth/attest", (route) => {
    expect(route.request().postDataJSON()).toEqual({ identity: wire.identity.toHexString() });
    attestations++;
    return route.fulfill({ json: { identity: wire.identity.toHexString(), fid: 1234, expiry: 1_800_000_600, signature: "11".repeat(64) } });
  });
  await page.routeWebSocket("wss://maincloud.spacetimedb.com/**", (socket) => {
    setTimeout(() => socket.send(wire.initial()), 30);
    socket.onMessage((bytes) => {
      const message = wire.client(bytes);
      if (message.tag === "Subscribe") socket.send(wire.snapshot(message.value.requestId, message.value.querySetId));
      else if (message.tag === "CallReducer") {
        const { reducer, args, requestId } = message.value;
        reducers.push(reducer);
        if (reducer === "verify_fid") {
          const decoded = wire.reducer(reducer, args);
          expect(decoded.fid).toBe(1234n);
          expect(decoded.expiry).toBe(1_800_000_600n);
          expect(Array.from(decoded.signature)).toEqual(Array(64).fill(0x11));
        }
        if (reducer === "send_message") expect(wire.reducer(reducer, args)).toEqual({ roomId: 1n, text: "A CSP-safe outgoing message" });
        socket.send(wire.ok(requestId));
      }
    });
  });
  const response = await page.goto("/messages?room=general");
  expect(response!.headers()["content-security-policy"]).toBe(policy);
  expect(policy).not.toContain("'unsafe-eval'");
  // No script route mocks: this is the actual built, fingerprinted SDK/bridge.
  await expect(page.locator(".rt-text")).toContainText("A message decoded by the real SDK under CSP");
  await expect(page.locator(".rt-online")).toHaveText("1 online");
  const composer = page.getByLabel("Message #general", { exact: true });
  await expect(composer).toBeVisible();
  expect(attestations).toBe(1);
  expect(reducers).toContain("verify_fid");
  expect(reducers).toContain("set_public_key");
  await composer.fill("A CSP-safe outgoing message");
  await composer.press("Enter");
  await expect.poll(() => reducers).toContain("send_message");
  await expect(composer).toHaveValue("");
  expect(await page.evaluate(() => (window as any).__cspFailures)).toEqual([]);
  expect(errors).toEqual([]);
});
