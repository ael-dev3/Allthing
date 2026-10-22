// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test } from "@playwright/test";

/*
 * `/api/v1/notifications` is a signed-in reader's own: the Worker reads
 * whose they are from the session, never from the query, and answers
 * nothing without one. These ask the real Worker, with no session (a
 * server here may not even have SESSION_SECRET, which refuses every
 * session, as it should).
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker and no sign-in");

const SIGN_IN = { error: "Sign in to see your notifications" };

test("the notifications endpoint answers only a session, and never from cache", async ({ request, baseURL }) => {
  const plain = await request.get("/api/v1/notifications");
  expect(plain.status()).toBe(401);
  expect(plain.headers()["cache-control"]).toBe("no-store");
  expect(await plain.json()).toEqual(SIGN_IN);

  // An account named in the query is no session; neither is a page of
  // someone's, or a cookie the Worker didn't sign.
  for (const path of ["/api/v1/notifications?fid=3", "/api/v1/notifications?cursor=eyJiZWZvcmVfdGltZXN0YW1wIjoxfQ"]) {
    const asked = await request.get(path);
    expect(asked.status(), path).toBe(401);
    expect(await asked.json(), path).toEqual(SIGN_IN);
  }
  const forged = await request.get("/api/v1/notifications", {
    headers: { cookie: `__Host-allthing-session=3.${Math.floor(Date.now() / 1000) + 3600}.${"0".repeat(64)}` },
  });
  expect(forged.status()).toBe(401);
  // The user cookie only labels the header; it signs no one in.
  const labelled = await request.get("/api/v1/notifications", {
    headers: { cookie: `__Host-allthing-user=${encodeURIComponent(JSON.stringify({ fid: 3, username: "dwr" }))}` },
  });
  expect(labelled.status()).toBe(401);

  // It only reads.
  const posted = await request.post("/api/v1/notifications", { headers: { origin: baseURL! }, data: {} });
  expect(posted.status()).toBe(405);
});
