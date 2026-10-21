// SPDX-License-Identifier: AGPL-3.0-or-later
import { expect, test, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";

/*
 * A profile shows the account's website under its bio: a link out, with a
 * link icon, on one line, its label cut short with an ellipsis when long,
 * never wider than a phone. It comes with the header, so it never moves
 * what is already shown, and its link is a target 24 px tall with a mouse
 * and 44 px on a touch screen. A website the account cleared, or one that
 * isn't a web address (a javascript: one), shows no link; and the label
 * always names where the link goes.
 */

test.skip(Boolean(process.env.E2E_PAGES), "uses the Worker API contract");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const REL = "noopener noreferrer nofollow ugc";
const longHref = `https://www.example.com/${"a-rather-long-path/".repeat(10)}end/`;
const longLabel = longHref.replace("https://www.", "").replace(/\/$/, "");

type Website = { href: string; label: string };
// What each profile's payload says of the account's website: as Allthing's
// build makes it, or, cleared and hostile, as another server might send it.
const accounts: Record<string, { fid: number; bio: string; website?: Website }> = {
  dan: { fid: 3, bio: "Interested in technology and other stuff.", website: { href: "https://danromero.org", label: "danromero.org" } },
  quiet: { fid: 7, bio: "", website: { href: "https://quiet.example/", label: "quiet.example" } },
  long: { fid: 8, bio: "A website as long as a profile can hold.", website: { href: longHref, label: longLabel } },
  cleared: { fid: 2, bio: "Cleared it.", website: { href: "", label: "" } },
  never: { fid: 99, bio: "Never set one." },
  hostile: { fid: 5, bio: "Says it's a website.", website: { href: "javascript:alert(document.domain)", label: "danromero.org" } },
  forged: { fid: 6, bio: "Its label says one place, its link another.", website: { href: "https://evil.example/login", label: "danromero.org" } },
};

async function serve(page: Page) {
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/user?*", (route) => {
    const name = new URL(route.request().url()).searchParams.get("name")!;
    const { fid, bio, website } = accounts[name];
    const user = {
      fid, username: name, display_name: name[0].toUpperCase() + name.slice(1), bio,
      follower_count: 1200, following_count: 34, registered_at: 1759222921, ...(website ? { website } : {}),
    };
    const pool = [{ hash: hex(fid), author: { fid, username: name }, text: `${name}'s own cast`, ts }];
    return route.fulfill({ json: { user, pool, notes: {}, meta } });
  });
  await page.route("**/api/v1/vote?*", (route) => route.fulfill({ json: { fid: 0, onchain: "stay", meta } }));
  await page.route("**/api/v1/wallets?*", (route) =>
    route.fulfill({ json: { fid: 0, addresses: [], chains: [], total: 0, sources: [], fetched_at: Date.now() } }),
  );
  // Whether the website was there in the same render as the header, and
  // where it was in the header then (against the header itself: the page
  // rises into place as it comes, which moves both).
  await page.addInitScript(() => {
    const w = window as unknown as { __website?: { atHeader: boolean; top: number | null } };
    new MutationObserver((_, observer) => {
      const header = document.querySelector("#app section.profile");
      if (!header?.querySelector(".profile-name")) return;
      const link = header.querySelector(".profile-website a");
      const top = link ? link.getBoundingClientRect().top - header.getBoundingClientRect().top : null;
      w.__website = { atHeader: Boolean(link), top };
      observer.disconnect();
    }).observe(document, { childList: true, subtree: true });
  });
}

/** Where the website's link is in its header. */
function topInHeader(link: Locator) {
  return link.evaluate((a) => a.getBoundingClientRect().top - a.closest("section.profile")!.getBoundingClientRect().top);
}

/** The link's size as laid out, which the page's rise into place doesn't change. */
function size(link: Locator) {
  return link.evaluate((a) => {
    const style = getComputedStyle(a);
    return { width: parseFloat(style.width), height: parseFloat(style.height) };
  });
}

async function at(browser: Browser, info: TestInfo, width: number) {
  const phone = width < 600;
  const context = await browser.newContext({
    baseURL: info.project.use.baseURL, viewport: { width, height: phone ? 844 : 900 }, isMobile: phone, hasTouch: phone,
  });
  return { context, page: await context.newPage() };
}

async function open(page: Page, name: string) {
  await page.goto(`/u/${name}`);
  const profile = page.locator("section.profile");
  await expect(profile.locator(".profile-name")).toHaveText(name[0].toUpperCase() + name.slice(1));
  // The vote and the wallets have come too: nothing below is still loading.
  await expect(profile.locator(".vote-loading, .share-bar.loading, .wallets-loading")).toHaveCount(0);
  return profile;
}

/** How the website's label is laid out. */
function layout(link: Locator) {
  return link.evaluate((a) => {
    const label = a.querySelector(".profile-website-label")!;
    const range = document.createRange();
    range.selectNodeContents(label);
    const style = getComputedStyle(label);
    const card = a.closest("section.profile")!;
    const padding = parseFloat(getComputedStyle(card).paddingRight);
    return {
      lines: new Set([...range.getClientRects()].filter((r) => r.width > 0).map((r) => Math.round(r.top))).size,
      cut: label.scrollWidth > label.clientWidth + 0.5,
      ellipsis: style.textOverflow === "ellipsis" && style.whiteSpace === "nowrap",
      inside: a.getBoundingClientRect().right <= card.getBoundingClientRect().right - padding + 0.5,
      pageWidth: document.documentElement.scrollWidth,
    };
  });
}

for (const width of [1280, 390]) {
  const phone = width < 600;

  test(`at ${width}, the website is a link out under the bio, on one line`, async ({ browser }, info) => {
    const { context, page } = await at(browser, info, width);
    await serve(page);
    const profile = await open(page, "dan");
    const link = profile.locator(".profile-website a");
    await expect(link).toHaveAttribute("href", "https://danromero.org");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(link).toHaveAttribute("rel", REL);
    await expect(link.locator(".profile-website-label")).toHaveText("danromero.org");
    await expect(link).toHaveAccessibleName("Website: danromero.org");
    await expect(link.locator("svg.icon")).toBeVisible();
    // No native tooltip.
    expect(await link.getAttribute("title")).toBeNull();

    // Under the bio, above the facts.
    const [bio, site, facts] = await Promise.all(
      [profile.locator(".bio"), profile.locator(".profile-website"), profile.locator(".facts")].map((l) => l.boundingBox()),
    );
    expect(site!.y).toBeGreaterThanOrEqual(bio!.y + bio!.height - 0.5);
    expect(facts!.y).toBeGreaterThanOrEqual(site!.y + site!.height - 0.5);

    // Its target: 24 px tall with a mouse, 44 px on a touch screen.
    const box = await size(link);
    expect(box.height).toBeGreaterThanOrEqual(phone ? 44 : 24);
    expect(box.width).toBeGreaterThanOrEqual(24);
    // A tap or click at its middle, and at its top and bottom, is its own.
    for (const dy of [0, -box.height / 2 + 1, box.height / 2 - 1]) {
      const own = await link.evaluate((a, dy) => {
        const r = a.getBoundingClientRect();
        const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2 + dy);
        return hit === a || a.contains(hit);
      }, dy);
      expect(own, `a press ${dy} px from its middle`).toBe(true);
    }

    // It came with the header, in the same render, and hasn't moved in it
    // since, the vote and the wallets having come.
    const first = await page.evaluate(() => (window as unknown as { __website: { atHeader: boolean; top: number } }).__website);
    expect(first.atHeader).toBe(true);
    expect(Math.abs((await topInHeader(link)) - first.top)).toBeLessThan(0.5);

    const laid = await layout(link);
    expect(laid.lines).toBe(1);
    expect(laid.cut).toBe(false);
    expect(laid.inside).toBe(true);
    expect(laid.pageWidth).toBeLessThanOrEqual(width);
    await profile.screenshot({ path: info.outputPath(`profile-website-${width}.png`) });

    if (!phone) {
      // It opens the site in a tab of its own, which can't reach back to
      // the profile, and the profile stays.
      await context.route("https://danromero.org/**", (route) =>
        route.fulfill({ contentType: "text/html", body: "<title>Dan Romero</title>" }),
      );
      const [tab] = await Promise.all([context.waitForEvent("page"), link.click()]);
      await tab.waitForLoadState();
      expect(tab.url()).toBe("https://danromero.org/");
      expect(await tab.evaluate(() => [window.opener === null, document.referrer])).toEqual([true, ""]);
      await expect(page).toHaveURL(/\/u\/dan$/);
    }
    await context.close();
  });

  test(`at ${width}, a long website is cut short with an ellipsis on one line`, async ({ browser }, info) => {
    const { context, page } = await at(browser, info, width);
    await serve(page);
    const profile = await open(page, "long");
    const link = profile.locator(".profile-website a");
    await expect(link).toHaveAttribute("href", longHref);
    await expect(link).toHaveAttribute("rel", REL);
    // Cut on screen only: the label is whole, and so is what it's read as.
    await expect(link.locator(".profile-website-label")).toHaveText(longLabel);
    await expect(link).toHaveAccessibleName(`Website: ${longLabel}`);
    const laid = await layout(link);
    expect(laid).toEqual({ lines: 1, cut: true, ellipsis: true, inside: true, pageWidth: laid.pageWidth });
    expect(laid.pageWidth).toBeLessThanOrEqual(width);
    expect((await size(link)).height).toBeGreaterThanOrEqual(phone ? 44 : 24);
    await profile.screenshot({ path: info.outputPath(`profile-website-long-${width}.png`) });
    await context.close();
  });

  test(`at ${width}, a profile with no bio shows its website under "No bio."`, async ({ browser }, info) => {
    const { context, page } = await at(browser, info, width);
    await serve(page);
    const profile = await open(page, "quiet");
    await expect(profile.locator(".bio")).toHaveText("No bio.");
    const link = profile.locator(".profile-website a");
    await expect(link).toHaveAttribute("href", "https://quiet.example/");
    await expect(link.locator(".profile-website-label")).toHaveText("quiet.example");
    await context.close();
  });

  test(`at ${width}, no link for a website cleared, never set, or not a web address`, async ({ browser }, info) => {
    const { context, page } = await at(browser, info, width);
    await serve(page);
    for (const name of ["cleared", "never", "hostile"]) {
      const profile = await open(page, name);
      await expect(profile.locator(".bio")).toHaveText(accounts[name].bio);
      await expect(page.locator(".profile-website")).toHaveCount(0);
      await expect(page.locator('a[href^="javascript:" i]')).toHaveCount(0);
      await expect(profile).not.toContainText("danromero.org");
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      if (name === "never") await profile.screenshot({ path: info.outputPath(`profile-no-website-${width}.png`) });
    }
    // The label names where the link goes, whatever the payload says.
    const profile = await open(page, "forged");
    const link = profile.locator(".profile-website a");
    await expect(link).toHaveAttribute("href", "https://evil.example/login");
    await expect(link.locator(".profile-website-label")).toHaveText("evil.example/login");
    await expect(profile).not.toContainText("danromero.org");
    await context.close();
  });
}
