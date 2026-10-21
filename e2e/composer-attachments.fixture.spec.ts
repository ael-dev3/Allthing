// SPDX-License-Identifier: AGPL-3.0-or-later
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, type Browser, type JSHandle, type Locator, type Page, type TestInfo } from "@playwright/test";
import { expectDraft } from "./helpers/composer";
import { approvedKey, cast as castOf, envelope, signIn, type Envelope } from "./helpers/farcaster";
import { standIn } from "./helpers/node";

/*
 * Images and videos with a cast (crates/allthing-web/src/components/
 * attach.rs): from the attach button (a phone's photo picker), a paste or
 * a drop; each uploaded at once, as the Worker takes it, with its progress,
 * and the cast's embeds are their URLs. `/api/v1/media` is a stand-in that
 * keeps what it's sent, as are the node and KLIPY; nothing leaves the
 * test. The browser's own XMLHttpRequest uploads, watched (`watchUploads`)
 * so a test can say how far one has gone and see it stopped. Where
 * `/api/v1/media` answers 404 (no R2 bucket bound), the composer has no
 * attach button and takes no files.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no sign-in");

const meta = { backend: "hypersnap", fetched_at: Date.now() };
const ts = 1792235681;
const hex = (n: number) => `0x${n.toString(16).padStart(40, "0")}`;
const alice = { fid: 42, username: "alice", display_name: "Alice" };
const me = { fid: 1234, username: "reader", display_name: "Reader" };
const post = { hash: hex(1), author: alice, text: "Show me your desk", ts, likes: 3, replies: 0 };
const NODE = "https://haatz.quilibrium.com/v1";
const KLIPY = "https://api.klipy.com/api/v1/fixture-key/gifs";
const cors = { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" };
const MB = 1024 * 1024;
const LIMITS = { max_image_bytes: 10 * MB, max_video_bytes: 50 * MB, max_embeds: 2 };
const FULL = "A cast takes two embeds: remove one to add another.";
const ACCEPT = "image/png,image/jpeg,image/gif,image/webp,video/mp4,video/quicktime,video/webm";
const EXT: Record<string, string> = {
  "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp",
  "video/mp4": "mp4", "video/quicktime": "mov", "video/webm": "webm",
};
const tinyPng = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==", "base64");

/** A KLIPY result, in its documented shape. */
const gif = (slug: string) => {
  const base = `https://static.klipy.com/ii/fixture/${slug}`;
  const format = (size: string, w: number) => ({
    gif: { url: `${base}/${size}.gif`, width: w, height: w, size: 1000 },
    webp: { url: `${base}/${size}.webp`, width: w, height: w, size: 500 },
  });
  return { id: slug.length, slug, title: slug.replace("-", " "), type: "gif", tags: [], file: { hd: format("hd", 498), md: format("md", 320), sm: format("sm", 200), xs: format("xs", 90) } };
};

type FileLike = { name: string; mimeType: string; buffer: Buffer };
type Upload = { body: Buffer; type: string; url: string; hls?: string };
type Kept = { id: number; at: number; hash: string; state: "waiting"; draft: string };

/**
 * The Worker's `/api/v1/media` and `/m/`, in memory: `held` keeps each upload's answer until `release`; `refuse` answers
 * the next ones so. With `limits` null, `/api/v1/media` answers as a Worker with no bucket bound: 404.
 */
class Media {
  uploads: Upload[] = [];
  asked = 0;
  posts = 0;
  held = false;
  refuse: [number, string][] = [];
  private waiting: (() => void)[] = [];
  constructor(public limits: object | null = LIMITS) {}

  release() {
    this.held = false;
    this.waiting.splice(0).forEach((go) => go());
  }

  async serve(page: Page) {
    await page.route("**/api/v1/media", async (route) => {
      const request = route.request();
      if (request.method() === "GET") {
        this.asked += 1;
        return this.limits
          ? route.fulfill({ json: this.limits })
          : route.fulfill({ status: 404, json: { error: "Attachments aren't set up on this server" } });
      }
      this.posts += 1;
      const body = request.postDataBuffer() ?? Buffer.alloc(0);
      const type = request.headers()["content-type"];
      if (this.held) await new Promise<void>((go) => this.waiting.push(go));
      const refused = this.refuse.shift();
      // The page may have stopped it meanwhile.
      if (refused) return route.fulfill({ status: refused[0], json: { error: refused[1] } }).catch(() => {});
      // A video remuxed for HLS comes with its index, and the cast embeds its playlist, as the Worker answers.
      const hls = request.headers()["x-allthing-hls"];
      const url = `${new URL(request.url()).origin}/m/${me.fid}/${"f".repeat(30)}${String(this.uploads.length + 1).padStart(2, "0")}.${hls ? "m3u8" : EXT[type]}`;
      this.uploads.push({ body, type, url, hls });
      return route.fulfill({ json: { url, type, bytes: body.length } }).catch(() => {});
    });
    await page.route("**/m/**", (route) => {
      const url = route.request().url();
      const upload = this.uploads.find((u) => u.url === url || (u.hls && u.url.replace(/\.m3u8$/, ".mp4") === url));
      return upload ? route.fulfill({ body: upload.body, contentType: upload.type }) : route.fulfill({ status: 404, body: "" });
    });
  }
}

/**
 * The feed, a post, the node, KLIPY and the reader's scheduled casts. `key`: whether the Worker endorses keys (writes
 * from here). `calls.feed` holds casts as Allthing's payloads carry them, newest first, above the feed's own.
 */
async function serve(page: Page, media: Media, { key = true } = {}) {
  // The node: it checks each message, and answers its read-backs.
  const node = await standIn(page, { fid: me.fid });
  const calls = { sent: node.sent as Envelope[], scheduled: [] as { message: string; draft: string; replaces?: number }[], feed: [] as { hash: string }[] };
  const kept: Kept[] = [];
  await page.route("**/api/v1/**", (route) => route.fulfill({ status: 404, json: { error: "Fixture endpoint" } }));
  await page.route("**/api/v1/home**", (route) => {
    const later = new URL(route.request().url()).searchParams.has("offset");
    return route.fulfill({ json: { pool: later ? [] : [...calls.feed, post], notes: {}, meta } });
  });
  await page.route("**/api/v1/cast?*", (route) => {
    const hash = new URL(route.request().url()).searchParams.get("hash");
    const cast = calls.feed.find((c) => c.hash === hash) ?? post;
    return route.fulfill({ json: { thread: { cast, children: [] }, notes: {}, meta } });
  });
  await page.route("**/api/v1/signer", (route) =>
    key ? route.fulfill({ json: { app_fid: 9152 } }) : route.fulfill({ status: 404, json: { error: "not set up" } }),
  );
  await page.route("**/api/v1/auth/signer", (route) => route.fulfill({ status: 404, json: { error: "not set up" } }));
  await page.route("**/api/v1/gifs/key", (route) => route.fulfill({ json: { key: "fixture-key" } }));
  await page.route(`${KLIPY}/**`, (route) =>
    route.fulfill({ headers: cors, json: { result: true, data: { data: [gif("trending-1"), gif("trending-2")], current_page: 1, per_page: 24, has_next: false } } }),
  );
  await page.route("https://static.klipy.com/**", (route) => route.fulfill({ body: tinyPng, contentType: "image/png" }));
  await page.route("https://farcaster.xyz/**", (route) => route.fulfill({ body: "<title>Farcaster</title>", contentType: "text/html" }));
  await page.route(`${NODE}/reactionsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [] } }));
  await page.route(`${NODE}/castsByFid?*`, (route) => route.fulfill({ headers: cors, json: { messages: [], nextPageToken: "" } }));
  await page.route("**/api/v1/scheduled", (route) => {
    const request = route.request();
    if (request.method() === "GET") return route.fulfill({ json: { casts: kept } });
    const asked = request.postDataJSON();
    calls.scheduled.push(asked);
    const signed = envelope(Buffer.from(asked.message, "base64"));
    const fields = { at: signed.at + 1_609_459_200, hash: signed.hash, draft: asked.draft, state: "waiting" as const };
    const one = asked.replaces ? Object.assign(kept.find((k) => k.id === asked.replaces)!, fields) : { id: 100 + kept.length, ...fields };
    if (!asked.replaces) kept.push(one);
    return route.fulfill({ json: one });
  });
  await media.serve(page);
  return calls;
}

/** Each upload the page sends, as the browser's XMLHttpRequest makes it, and whether it was stopped. */
async function watchUploads(page: Page) {
  await page.addInitScript(() => {
    const w = window as unknown as { uploads: { xhr: XMLHttpRequest; aborted: boolean }[] };
    w.uploads = [];
    const send = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.send = function (this: XMLHttpRequest, body?: Document | XMLHttpRequestBodyInit | null) {
      const entry = { xhr: this, aborted: false };
      this.addEventListener("abort", () => (entry.aborted = true));
      w.uploads.push(entry);
      return send.call(this, body);
    };
  });
}

/** Upload `n`'s progress, as the browser reports it: `loaded` bytes of `total`. */
const progress = (page: Page, n: number, loaded: number, total: number) =>
  page.evaluate(
    ([n, loaded, total]) => {
      const w = window as unknown as { uploads: { xhr: XMLHttpRequest }[] };
      w.uploads[n].xhr.upload.dispatchEvent(new ProgressEvent("progress", { lengthComputable: true, loaded, total }));
    },
    [n, loaded, total],
  );

/** Whether `src` shows as a picture in the page. */
const shows = (page: Page, src: string) =>
  page.evaluate(
    (src) =>
      new Promise<boolean>((resolve) => {
        const img = new Image();
        img.onload = () => resolve(img.naturalWidth > 0);
        img.onerror = () => resolve(false);
        img.src = src;
      }),
    src,
  );

/** Whether the bar's buttons and Post fit in it, on one line, none squeezed narrower than what it holds. */
const fits = (dialog: Locator) =>
  dialog.locator(".compose-bar").evaluate((bar) => {
    const whole = (el: Element) => {
      const box = el.getBoundingClientRect();
      const icon = el.querySelector("img, svg")?.getBoundingClientRect();
      return !icon || (icon.left >= box.left + 2 && icon.right <= box.right - 2);
    };
    const post = bar.querySelector(".compose-post")!;
    return (
      bar.scrollWidth <= bar.clientWidth &&
      bar.getBoundingClientRect().height < 48 &&
      post.scrollWidth <= post.clientWidth &&
      [...bar.querySelectorAll("button, a")].every(whole)
    );
  });

const aborted = (page: Page) => page.evaluate(() => (window as unknown as { uploads: { aborted: boolean }[] }).uploads.map((u) => u.aborted));

/** Signed in, with a key approved an hour ago (or none, where the Worker endorses none), on `path`. */
async function ready(page: Page, media: Media, { key = true, path = "/" } = {}) {
  await signIn(page, me);
  await watchUploads(page);
  const calls = await serve(page, media, { key });
  await page.goto(path);
  await expect(page.locator(".cast-card:not(.skeleton), .post").first()).toBeVisible();
  if (key) {
    await approvedKey(page, me.fid, 3_600_000);
    await page.reload();
    await expect(page.locator(".cast-card:not(.skeleton), .post").first()).toBeVisible();
  }
  return calls;
}

async function phone(browser: Browser, testInfo: TestInfo) {
  const context = await browser.newContext({ baseURL: testInfo.project.use.baseURL, viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  return { context, page: await context.newPage() };
}

/** A picture drawn in the page (a landscape at `hue`), as a file of `type`'s bytes, with no metadata. */
async function drawn(page: Page, name: string, type = "image/png", hue = 210, w = 640, h = 480): Promise<FileLike> {
  const url = await page.evaluate(
    ({ type, hue, w, h }) => {
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const g = canvas.getContext("2d")!;
      const sky = g.createLinearGradient(0, 0, 0, h);
      sky.addColorStop(0, `hsl(${hue} 75% 70%)`);
      sky.addColorStop(1, `hsl(${hue + 40} 70% 88%)`);
      g.fillStyle = sky;
      g.fillRect(0, 0, w, h);
      g.fillStyle = `hsl(${hue + 170} 90% 62%)`;
      g.beginPath();
      g.arc(w * 0.72, h * 0.3, h * 0.11, 0, Math.PI * 2);
      g.fill();
      g.fillStyle = `hsl(${hue + 110} 35% 34%)`;
      g.beginPath();
      g.moveTo(0, h);
      g.lineTo(w * 0.3, h * 0.48);
      g.lineTo(w * 0.52, h * 0.78);
      g.lineTo(w * 0.78, h * 0.55);
      g.lineTo(w, h * 0.82);
      g.lineTo(w, h);
      g.fill();
      return canvas.toDataURL(type, 0.9);
    },
    { type, hue, w, h },
  );
  return { name, mimeType: type, buffer: Buffer.from(url.split(",")[1], "base64") };
}

/** A short video of `type` (WebM, or MP4 with VP9, which this Chromium plays), recorded in the page from a canvas, as a phone's clip would be. */
async function recorded(page: Page, name: string, type = "video/webm"): Promise<FileLike> {
  const b64 = await page.evaluate(async (type) => {
    const canvas = document.createElement("canvas");
    canvas.width = 320;
    canvas.height = 240;
    const g = canvas.getContext("2d")!;
    const recorder = new MediaRecorder(canvas.captureStream(30), { mimeType: type });
    const parts: Blob[] = [];
    recorder.ondataavailable = (e) => parts.push(e.data);
    const done = new Promise((resolve) => (recorder.onstop = resolve));
    recorder.start();
    for (let i = 0; i < 12; i++) {
      g.fillStyle = `hsl(${20 + i * 6} 80% 55%)`;
      g.fillRect(0, 0, 320, 240);
      g.fillStyle = "#fff";
      g.fillRect(40 + i * 18, 90, 60, 60);
      await new Promise((r) => setTimeout(r, 40));
    }
    recorder.stop();
    await done;
    const bytes = new Uint8Array(await new Blob(parts, { type }).arrayBuffer());
    let s = "";
    bytes.forEach((b) => (s += String.fromCharCode(b)));
    return btoa(s);
  }, type);
  return { name, mimeType: type, buffer: Buffer.from(b64, "base64") };
}

/** A cast as Allthing's payloads carry it once the node has it: its embeds as `normalize::embed` reads their URLs. */
const listed = (hash: string, text: string, embeds: { kind: string; url: string }[]) => ({
  hash, author: me, text, ts: Math.floor(Date.now() / 1000) - 5, embeds, likes: 0, recasts: 0, replies: 0,
});

/** That `scope` (a card, a post) shows the PNG at `png` as a picture and the MP4 at `mp4` as a video that plays. */
async function showsMedia(scope: Locator, png: string, mp4: string) {
  const image = scope.locator(".tile-image img");
  await expect(image).toHaveAttribute("src", png);
  await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth)).toBeGreaterThan(0);
  const video = scope.locator(".tile-video video");
  await expect(video).toHaveAttribute("src", `${mp4}#t=0.1`);
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => (v.error ? -1 : v.readyState))).toBeGreaterThanOrEqual(2);
  await video.evaluate((v: HTMLVideoElement) => {
    v.muted = true;
    return v.play();
  });
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime)).toBeGreaterThan(0);
  expect(await video.evaluate((v: HTMLVideoElement) => v.error)).toBeNull();
}

/** Pastes into `box` what a clipboard holds: `files`, and `text` by type. */
async function paste(box: Locator, files: FileLike[], text: Record<string, string> = {}) {
  await box.evaluate(
    (el, { files, text }) => {
      const data = new DataTransfer();
      for (const [type, value] of Object.entries(text)) data.setData(type, value);
      for (const f of files) data.items.add(new File([Uint8Array.from(atob(f.b64), (c) => c.charCodeAt(0))], f.name, { type: f.type }));
      el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
    },
    { files: files.map((f) => ({ name: f.name, type: f.mimeType, b64: f.buffer.toString("base64") })), text },
  );
}

/** What a drag from the desktop carries: `files`. */
const dragged = (page: Page, files: FileLike[]) =>
  page.evaluateHandle((files) => {
    const data = new DataTransfer();
    for (const f of files) data.items.add(new File([Uint8Array.from(atob(f.b64), (c) => c.charCodeAt(0))], f.name, { type: f.type }));
    return data;
  }, files.map((f) => ({ name: f.name, type: f.mimeType, b64: f.buffer.toString("base64") })));

/** Picks `files` with the attach button, as its file picker would. */
async function attach(page: Page, dialog: Locator, files: FileLike[], tap = false) {
  const chooser = page.waitForEvent("filechooser");
  const button = dialog.getByRole("button", { name: "Attach images or videos" });
  await (tap ? button.tap() : button.click());
  await (await chooser).setFiles(files);
}

const tiles = (dialog: Locator) => dialog.locator(".compose-tile");
const note = (dialog: Locator) => dialog.locator(".attach-note");

/** An Exif APP1 segment as a phone writes one: its make, and where it was taken (52°31'12.34"N 13°24'56.78"E). */
function exifWithGps(): Buffer {
  const tiff = Buffer.alloc(198);
  tiff.write("II", 0, "latin1");
  tiff.writeUInt16LE(42, 2);
  tiff.writeUInt32LE(8, 4);
  const entry = (at: number, tag: number, type: number, count: number, value: number) => {
    tiff.writeUInt16LE(tag, at);
    tiff.writeUInt16LE(type, at + 2);
    tiff.writeUInt32LE(count, at + 4);
    tiff.writeUInt32LE(value, at + 8);
  };
  // IFD0 at 8: Make, Orientation (1: as it is), and the GPS IFD's offset.
  tiff.writeUInt16LE(3, 8);
  entry(10, 0x010f, 2, 13, 50);
  entry(22, 0x0112, 3, 1, 1);
  entry(34, 0x8825, 4, 1, 64);
  tiff.writeUInt32LE(0, 46);
  tiff.write("FixturePhone\0", 50, "latin1");
  // The GPS IFD at 64: latitude and longitude, and how they were fixed.
  tiff.writeUInt16LE(5, 64);
  entry(66, 0x0001, 2, 2, 0x4e);
  entry(78, 0x0002, 5, 3, 130);
  entry(90, 0x0003, 2, 2, 0x45);
  entry(102, 0x0004, 5, 3, 154);
  entry(114, 0x001b, 7, 19, 178);
  tiff.writeUInt32LE(0, 126);
  [52, 1, 31, 1, 1234, 100, 13, 1, 24, 1, 5678, 100].forEach((n, i) => tiff.writeUInt32LE(n, 130 + i * 4));
  tiff.write("ASCII\0\0\0GPS-FIXTURE", 178, "latin1");
  return segment(0xe1, Buffer.concat([Buffer.from("Exif\0\0", "latin1"), tiff]));
}

const segment = (marker: number, payload: Buffer) => {
  const head = Buffer.from([0xff, marker, 0, 0]);
  head.writeUInt16BE(payload.length + 2, 2);
  return Buffer.concat([head, payload]);
};

test("at 1280, files attach from the button and a paste, show their uploads' progress, and go out as the cast's embeds", async ({ page }, info) => {
  const media = new Media();
  const calls = await ready(page, media);
  media.held = true;
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  // Asked once the composer opens, as the reader is signed in.
  const button = dialog.getByRole("button", { name: "Attach images or videos" });
  await expect(button).toBeVisible();
  expect(media.asked).toBe(1);
  await expect(button).toHaveAccessibleDescription("Images up to 10 MB, videos up to 50 MB");
  const input = dialog.locator("input.attach-input");
  await expect(input).toHaveAttribute("type", "file");
  await expect(input).toHaveAttribute("accept", ACCEPT);
  await expect(input).toHaveAttribute("multiple", "");
  // From the button: a tile at once, with its picture and its upload's progress.
  const desk = await drawn(page, "desk.png", "image/png", 200);
  await attach(page, dialog, [desk]);
  await expect(tiles(dialog)).toHaveCount(1);
  const preview = tiles(dialog).first().getByRole("img", { name: "desk.png" });
  await expect(preview).toHaveAttribute("src", /^blob:/);
  await expect.poll(() => preview.evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth)).toBe(640);
  const first = tiles(dialog).first().getByRole("progressbar", { name: "Uploading desk.png" });
  await expect(first).toHaveAttribute("aria-valuenow", "0");
  await expect(dialog.getByRole("button", { name: "Uploading…" })).toBeDisabled();
  await expect.poll(() => media.posts).toBe(1);
  await progress(page, 0, 4_500, 10_000);
  await expect(first).toHaveAttribute("aria-valuenow", "45");
  await expect(first.locator("span")).toHaveAttribute("style", "width: 45%");
  // A screenshot pasted: attached, not pasted as text.
  const box = dialog.getByRole("textbox", { name: "What's happening?" });
  await box.fill("My desk, and the view");
  const shot = await drawn(page, "screenshot.png", "image/png", 30);
  await paste(box, [shot]);
  await expect(tiles(dialog)).toHaveCount(2);
  await expectDraft(box, "My desk, and the view");
  await expect.poll(() => media.posts).toBe(2);
  await progress(page, 1, 1_800, 10_000);
  await expect(tiles(dialog).nth(1).getByRole("progressbar")).toHaveAttribute("aria-valuenow", "18");
  // Text copied with a picture of itself (a spreadsheet's cells) is pasted as text.
  await paste(box, [shot], { "text/plain": " (Q3)", "text/html": "<table><tr><td>Q3</td></tr></table>" });
  await expectDraft(box, "My desk, and the view (Q3)");
  await expect(tiles(dialog)).toHaveCount(2);
  await page.mouse.move(0, 0);
  await page.screenshot({ path: info.outputPath("attach-1280-uploading.png") });
  // Once both are kept, Cast; the cast embeds their URLs, in order.
  media.release();
  await expect(dialog.getByRole("progressbar")).toHaveCount(0);
  const cast = dialog.getByRole("button", { name: "Cast", exact: true });
  await expect(cast).toBeEnabled();
  expect(media.uploads.map((u) => u.type)).toEqual(["image/png", "image/png"]);
  // A PNG with no metadata goes as it is.
  expect(media.uploads[0].body.equals(desk.buffer)).toBe(true);
  expect(media.uploads[1].body.equals(shot.buffer)).toBe(true);
  await cast.click();
  await expect(dialog).toBeHidden();
  expect(calls.sent).toHaveLength(1);
  expect(castOf(calls.sent[0])).toEqual({
    text: "My desk, and the view (Q3)",
    mentions: [],
    positions: [],
    embeds: media.uploads.map((u) => ({ url: u.url })),
    parent: null,
    type: 0,
  });
  // The reader's cast shows at once above the feed, with its first picture.
  const outgoing = page.locator(".cast-card.outgoing");
  await expect(outgoing).toContainText("My desk, and the view (Q3)");
  await expect(outgoing.locator("img.outgoing-image")).toHaveAttribute("src", media.uploads[0].url);
  // The next draft starts with none, and its pictures were let go.
  await page.getByRole("button", { name: "Write a cast" }).click();
  await expect(tiles(page.getByRole("dialog", { name: "New cast" }))).toHaveCount(0);
});

test("a PNG and an MP4 from the attach button upload, go out as the cast's embeds, and its card and post show a picture and a video that plays", async ({ page }, info) => {
  const media = new Media();
  const calls = await ready(page, media);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const png = await drawn(page, "sunset.png", "image/png", 25);
  const mp4 = await recorded(page, "clip.mp4", "video/mp4");
  expect(mp4.buffer.subarray(4, 8).toString("latin1")).toBe("ftyp");
  await attach(page, dialog, [png, mp4]);
  await expect(tiles(dialog)).toHaveCount(2);
  await expect(tiles(dialog).nth(1).locator("video")).toHaveAttribute("aria-label", "clip.mp4");
  await dialog.getByRole("textbox", { name: "What's happening?" }).fill("A picture and a clip");
  const cast = dialog.getByRole("button", { name: "Cast", exact: true });
  await expect(cast).toBeEnabled();
  await cast.click();
  await expect(dialog).toBeHidden();
  // Both went to /api/v1/media as they were picked (a PNG with no metadata is as it was).
  const of = (type: string) => media.uploads.find((u) => u.type === type)!;
  expect(media.uploads.map((u) => u.type).sort()).toEqual(["image/png", "video/mp4"]);
  expect(of("image/png").body.equals(png.buffer)).toBe(true);
  expect(of("video/mp4").body.equals(mp4.buffer)).toBe(true);
  // The cast embeds their URLs, in the order they were picked.
  const [sent] = calls.sent;
  const urls = [of("image/png").url, of("video/mp4").url];
  expect(castOf(sent).embeds).toEqual(urls.map((url) => ({ url })));
  expect(urls[1]).toMatch(/\/m\/1234\/[0-9a-f]{32}\.mp4$/);
  // Once the node has it, the feed's card and its post show the picture, and the video, which plays.
  calls.feed.push(listed(sent.hash, "A picture and a clip", [{ kind: "image", url: urls[0] }, { kind: "video", url: urls[1] }]));
  await page.reload();
  const card = page.locator(".cast-card:not(.outgoing)").filter({ hasText: "A picture and a clip" });
  await expect(card).toHaveCount(1);
  await showsMedia(card, urls[0], urls[1]);
  await page.screenshot({ path: info.outputPath("attach-1280-posted.png") });
  await page.goto(`/p/${sent.hash}`);
  await showsMedia(page.locator(".post"), urls[0], urls[1]);
});

test("an MP4 is remuxed for the Farcaster app: it goes up as a fragmented MP4 with its HLS index, the cast embeds its playlist, and Allthing plays its MP4", async ({ page }) => {
  const media = new Media();
  const calls = await ready(page, media);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  // An MP4 as a camera or an editor writes it: progressive, H.264 and AAC, with where it was recorded.
  const original = readFileSync(path.join(__dirname, "../crates/allthing-core/tests/fixtures/video-h264-aac.mp4"));
  await attach(page, dialog, [{ name: "reading.mp4", mimeType: "video/mp4", buffer: original }]);
  await expect(tiles(dialog)).toHaveCount(1);
  await dialog.getByRole("textbox", { name: "What's happening?" }).fill("Chapter 1, Part 02");
  const cast = dialog.getByRole("button", { name: "Cast", exact: true });
  await expect(cast).toBeEnabled();
  await cast.click();
  await expect(dialog).toBeHidden();
  const [up] = media.uploads;
  expect(up.type).toBe("video/mp4");
  // The same samples as fragments: a different file, with a moof per segment and no recorded place.
  expect(up.body.equals(original)).toBe(false);
  expect(up.body.subarray(4, 8).toString("latin1")).toBe("ftyp");
  expect(up.body.includes(Buffer.from("moof"))).toBe(true);
  expect(up.body.includes(Buffer.from("+40.0000-074.0000"))).toBe(false);
  // Its index: its size and codecs, its init and three 4 s segments, which add up to the file.
  const match = up.hls?.match(/^1;64x36;avc1\.[0-9a-f]{6},mp4a\.40\.2;(\d+);(\d+):(\d+),(\d+):(\d+),(\d+):(\d+)$/);
  expect(match, up.hls).toBeTruthy();
  const [init, ...segments] = match!.slice(1).map(Number);
  expect(segments.filter((_, i) => i % 2 === 0)).toEqual([4000, 4000, 4000]);
  expect(init + segments.filter((_, i) => i % 2 === 1).reduce((a, b) => a + b)).toBe(up.body.length);
  // The cast embeds the playlist, which the Farcaster app plays.
  const [sent] = calls.sent;
  expect(up.url).toMatch(/\/m\/1234\/[0-9a-f]{32}\.m3u8$/);
  expect(castOf(sent).embeds).toEqual([{ url: up.url }]);
  // Allthing's own card plays the MP4 beside it, as a plain file.
  calls.feed.push(listed(sent.hash, "Chapter 1, Part 02", [{ kind: "video", url: up.url }]));
  await page.reload();
  const card = page.locator(".cast-card:not(.outgoing)").filter({ hasText: "Chapter 1, Part 02" });
  await expect(card.locator(".tile-video video")).toHaveAttribute("src", `${up.url.replace(/\.m3u8$/, ".mp4")}#t=0.1`);
});

test("files dropped on the composer attach, with a hint while they're over it; a cast takes two embeds, a GIF and a quote among them", async ({ page }, info) => {
  const media = new Media();
  const calls = await ready(page, media);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("button", { name: "Attach images or videos" })).toBeVisible();
  // Dragged over: the hint; dragged away: gone; dropped: attached.
  const photo = await drawn(page, "photo.jpg", "image/jpeg", 150);
  const data: JSHandle = await dragged(page, [photo]);
  const hint = dialog.locator(".compose-drop");
  await dialog.locator(".compose-text").dispatchEvent("dragenter", { dataTransfer: data });
  await dialog.locator(".compose-text").dispatchEvent("dragover", { dataTransfer: data });
  await expect(hint).toBeVisible();
  await expect(hint).toHaveText("Drop images or videos to attach them");
  await page.screenshot({ path: info.outputPath("attach-1280-drop.png") });
  await dialog.locator(".compose-text").dispatchEvent("dragleave", { dataTransfer: data });
  await expect(hint).toHaveCount(0);
  await dialog.locator(".modal-head").dispatchEvent("dragenter", { dataTransfer: data });
  await expect(hint).toBeVisible();
  await dialog.locator(".modal-head").dispatchEvent("drop", { dataTransfer: data });
  await expect(hint).toHaveCount(0);
  await expect(tiles(dialog)).toHaveCount(1);
  await expect.poll(() => media.uploads.length).toBe(1);
  expect(media.uploads[0].type).toBe("image/jpeg");
  // With a GIF too, the cast has two embeds: no room for another file.
  await dialog.getByRole("button", { name: "Add a GIF" }).click();
  await dialog.getByRole("button", { name: "trending 1" }).click();
  const button = dialog.getByRole("button", { name: "Attach images or videos" });
  await expect(button).toHaveAttribute("aria-disabled", "true");
  await expect(button).toHaveAccessibleDescription(FULL);
  // Pressed anyway, it says why.
  await button.click({ force: true });
  await expect(note(dialog)).toHaveText(FULL);
  await expect(note(dialog)).toHaveAttribute("role", "status");
  await paste(dialog.getByRole("textbox"), [await drawn(page, "more.png")]);
  await expect(note(dialog)).toHaveText(FULL);
  await expect(tiles(dialog)).toHaveCount(1);
  // The GIF can still be changed for another, in its place.
  await expect(dialog.getByRole("button", { name: "Add a GIF" })).not.toHaveAttribute("aria-disabled", "true");
  // Without the GIF, two files: the second of three picked fits, the third doesn't.
  await dialog.getByRole("button", { name: "Remove the GIF" }).click();
  await expect(button).not.toHaveAttribute("aria-disabled", "true");
  await attach(page, dialog, [await drawn(page, "two.png", "image/png", 90), await drawn(page, "three.png", "image/png", 300)]);
  await expect(tiles(dialog)).toHaveCount(2);
  await expect(note(dialog)).toHaveText(FULL);
  await expect(tiles(dialog).nth(1).getByRole("img")).toHaveAccessibleName("two.png");
  // Now neither a GIF nor a sticker fits: their buttons say so.
  for (const name of ["Add a GIF", "Add a sticker"]) {
    const other = dialog.getByRole("button", { name });
    await expect(other).toHaveAttribute("aria-disabled", "true");
    await expect(other).toHaveAccessibleDescription(FULL);
  }
  await dialog.getByRole("button", { name: "Add a GIF" }).click({ force: true });
  await expect(dialog.locator(".gif-picker")).toHaveCount(0);
  await expect(note(dialog)).toHaveText(FULL);
  // Removing one makes room again, and says nothing more.
  await dialog.getByRole("button", { name: "Remove two.png" }).click();
  await expect(note(dialog)).toHaveText("");
  await expect(dialog.getByRole("button", { name: "Add a sticker" })).not.toHaveAttribute("aria-disabled", "true");
  await dialog.getByRole("button", { name: "Close" }).click();

  // A quote is one of the two: one file goes with it, and no GIF.
  await page.goto(`/p/${post.hash}`);
  await page.locator(".post").getByRole("button", { name: "Quote @alice's cast" }).click();
  const quote = page.getByRole("dialog", { name: "Quote" });
  await attach(page, quote, [await drawn(page, "a.png", "image/png", 10), await drawn(page, "b.png", "image/png", 60)]);
  await expect(tiles(quote)).toHaveCount(1);
  await expect(note(quote)).toHaveText(FULL);
  await expect(quote.getByRole("button", { name: "Add a GIF" })).toHaveAttribute("aria-disabled", "true");
  await expect(quote.getByRole("button", { name: "Attach images or videos" })).toHaveAttribute("aria-disabled", "true");
  await expect(quote.getByRole("button", { name: "Quote", exact: true })).toBeEnabled();
  await quote.getByRole("button", { name: "Quote", exact: true }).click();
  await expect(quote).toBeHidden();
  const sent = castOf(calls.sent[0]);
  expect(sent.embeds).toEqual([{ url: media.uploads.at(-1)!.url }, { cast: { fid: alice.fid, hash: post.hash } }]);
});

test("Remove stops an upload on its way, and lets its picture go", async ({ page }) => {
  const media = new Media();
  await ready(page, media);
  media.held = true;
  const failed: string[] = [];
  page.on("requestfailed", (request) => {
    if (request.url().endsWith("/api/v1/media")) failed.push(request.failure()?.errorText ?? "");
  });
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await attach(page, dialog, [await drawn(page, "oops.png")]);
  await expect(dialog.getByRole("progressbar")).toBeVisible();
  await expect.poll(() => media.posts).toBe(1);
  const src = (await tiles(dialog).first().getByRole("img").getAttribute("src"))!;
  expect(await shows(page, src)).toBe(true);
  await dialog.getByRole("button", { name: "Remove oops.png" }).click();
  await expect(tiles(dialog)).toHaveCount(0);
  expect(await aborted(page)).toEqual([true]);
  await expect.poll(() => failed).toEqual(["net::ERR_ABORTED"]);
  expect(await shows(page, src)).toBe(false);
  // Its answer, coming after all, changes nothing.
  media.release();
  await page.waitForTimeout(300);
  await expect(tiles(dialog)).toHaveCount(0);
  await expect(note(dialog)).toHaveText("");
  await expect(dialog.getByRole("button", { name: "Cast", exact: true })).toBeDisabled();
});

test("a file the Worker refuses says why, with Retry and Remove; files that can't go are refused before they're sent", async ({ page }, info) => {
  const media = new Media({ max_image_bytes: 300 * 1024, max_video_bytes: 2 * MB, max_embeds: 2 });
  await ready(page, media);
  media.refuse.push([413, "Images can be up to 10 MB"]);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox").fill("Look at this");
  await attach(page, dialog, [await drawn(page, "big.png", "image/png", 280)]);
  const error = dialog.getByRole("alert").filter({ hasText: "big.png" });
  await expect(error).toHaveText(/^big\.png: Images can be up to 10 MBRetryRemove$/);
  await expect(tiles(dialog).first()).toHaveClass(/failed/);
  await expect(dialog.getByRole("progressbar")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: "Cast", exact: true })).toBeDisabled();
  await page.mouse.move(0, 0);
  await page.screenshot({ path: info.outputPath("attach-1280-error.png") });
  // Retry: it goes this time.
  await error.getByRole("button", { name: "Retry" }).click();
  await expect(error).toHaveCount(0);
  await expect(tiles(dialog).first()).not.toHaveClass(/failed/);
  await expect(dialog.getByRole("button", { name: "Cast", exact: true })).toBeEnabled();
  expect(media.posts).toBe(2);
  // Too large, not a picture or a video, or empty: said at once, never sent.
  await dialog.getByRole("button", { name: "Remove big.png" }).click();
  const huge = { name: "huge.png", mimeType: "image/png", buffer: Buffer.alloc(300 * 1024 + 1, 1) };
  const notes = { name: "notes.pdf", mimeType: "application/pdf", buffer: Buffer.from("%PDF-1.7") };
  const empty = { name: "empty.webp", mimeType: "image/webp", buffer: Buffer.alloc(0) };
  await attach(page, dialog, [huge, notes, empty]);
  await expect(note(dialog)).toHaveText(
    "huge.png is too large: images can be up to 300 KB. " +
      "notes.pdf can't go with a cast: attach PNG, JPEG, GIF or WebP images, or MP4, MOV or WebM videos. " +
      "empty.webp is empty.",
  );
  await expect(tiles(dialog)).toHaveCount(0);
  expect(media.posts).toBe(2);
  // A video too large for this server's limit.
  await attach(page, dialog, [{ name: "clip.mov", mimeType: "video/quicktime", buffer: Buffer.alloc(2 * MB + 1) }]);
  await expect(note(dialog)).toHaveText("clip.mov is too large: videos can be up to 2 MB.");
  expect(media.posts).toBe(2);
});

test("a photo's Exif, GPS position, XMP and IPTC are taken out before it's uploaded, and its picture kept byte for byte", async ({ page }) => {
  const media = new Media();
  const calls = await ready(page, media);
  const clean = await drawn(page, "IMG_0042.jpg", "image/jpeg", 40);
  const xmp = segment(0xe1, Buffer.from('http://ns.adobe.com/xap/1.0/\0<x:xmpmeta><exif:GPSLatitude>52,31.2N GPS-FIXTURE-XMP</exif:GPSLatitude></x:xmpmeta>', "latin1"));
  const iptc = segment(0xed, Buffer.from("Photoshop 3.0\x008BIM\x04\x04\0\0\0\0\0\x10IPTC-FIXTURE-CITY", "latin1"));
  const photo = Buffer.concat([clean.buffer.subarray(0, 2), exifWithGps(), xmp, iptc, clean.buffer.subarray(2)]);
  expect(photo.includes("GPS-FIXTURE")).toBe(true);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await attach(page, dialog, [{ name: "IMG_0042.jpg", mimeType: "image/jpeg", buffer: photo }]);
  await expect.poll(() => media.uploads.length).toBe(1);
  const [sent] = media.uploads;
  expect(sent.type).toBe("image/jpeg");
  for (const word of ["Exif", "GPS-FIXTURE", "FixturePhone", "ns.adobe.com/xap", "Photoshop 3.0", "IPTC-FIXTURE"]) {
    expect(sent.body.includes(word), word).toBe(false);
  }
  expect(sent.body.equals(clean.buffer)).toBe(true);
  // And it goes out as the cast's one embed.
  await expect(dialog.getByRole("button", { name: "Cast", exact: true })).toBeEnabled();
  await dialog.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(castOf(calls.sent[0]).embeds).toEqual([{ url: sent.url }]);
});

test("where Allthing doesn't post, the draft goes to the Farcaster app with its files as embeds", async ({ page }) => {
  const media = new Media();
  await ready(page, media, { key: false });
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox").fill("From my phone's camera roll");
  media.held = true;
  await attach(page, dialog, [await drawn(page, "one.png", "image/png", 0), await drawn(page, "two.jpg", "image/jpeg", 120)]);
  await expect(dialog.getByRole("button", { name: "Uploading…" })).toBeDisabled();
  media.release();
  const post = dialog.getByRole("button", { name: "Post in Farcaster" });
  await expect(post).toBeEnabled();
  await post.click();
  const handoff = page.getByRole("dialog", { name: "Post in Farcaster" });
  const href = (await handoff.getByRole("link", { name: /Open on farcaster\.xyz/ }).getAttribute("href"))!;
  const url = new URL(href);
  expect(url.searchParams.get("text")).toBe("From my phone's camera roll");
  expect(url.searchParams.getAll("embeds[]")).toEqual(media.uploads.map((u) => u.url));
});

test("a scheduled cast keeps its files, on /scheduled and when it's changed", async ({ page }) => {
  const media = new Media();
  const calls = await ready(page, media);
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await dialog.getByRole("textbox").fill("Tomorrow's sunrise");
  await attach(page, dialog, [await drawn(page, "sunrise.jpg", "image/jpeg", 20)]);
  await expect.poll(() => media.uploads.length).toBe(1);
  await dialog.getByRole("button", { name: "Schedule for later" }).click();
  await dialog.getByRole("button", { name: "Schedule", exact: true }).click();
  await expect(dialog).toBeHidden();
  const [url] = media.uploads.map((u) => u.url);
  expect(JSON.parse(calls.scheduled[0].draft)).toEqual({ text: "Tomorrow's sunrise", media: [{ url, type: "image/jpeg" }] });
  expect(castOf(envelope(Buffer.from(calls.scheduled[0].message, "base64"))).embeds).toEqual([{ url }]);
  // Listed with its picture; opened again, the composer has it.
  await page.goto("/scheduled");
  const item = page.locator(".scheduled-item");
  await expect(item.locator(".scheduled-media img")).toHaveAttribute("src", url);
  await item.getByRole("button", { name: "Edit" }).click();
  const edit = page.getByRole("dialog", { name: "Scheduled cast" });
  await expect(tiles(edit).getByRole("img")).toHaveAttribute("src", url);
  await expect(edit.getByRole("progressbar")).toHaveCount(0);
  await edit.getByRole("textbox", { name: "What's happening?" }).fill("Tomorrow's sunrise, again");
  await edit.getByRole("button", { name: "Save" }).click();
  await expect(edit).toBeHidden();
  expect(JSON.parse(calls.scheduled[1].draft)).toEqual({ text: "Tomorrow's sunrise, again", media: [{ url, type: "image/jpeg" }] });
  expect(calls.scheduled[1].replaces).toBe(100);
});

/** Dispatches a drag event of `type` carrying `data` on `target`; whether the page took it (prevented its default). */
const drag = (target: Locator, type: string, data: JSHandle) =>
  target.evaluate(
    (el, { type, data }) => {
      const event = new DragEvent(type, { dataTransfer: data as DataTransfer, bubbles: true, cancelable: true });
      el.dispatchEvent(event);
      return event.defaultPrevented;
    },
    { type, data },
  );

/** The composer as it was before files: no attach button or file input, and a file pasted or dropped not taken. */
async function takesNoFiles(page: Page, dialog: Locator) {
  await expect(dialog.getByRole("button", { name: "Add a sticker" })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Attach images or videos" })).toHaveCount(0);
  await expect(dialog.locator("input[type=file]")).toHaveCount(0);
  const data = await dragged(page, [await drawn(page, "photo.png")]);
  const head = dialog.locator(".modal-head");
  expect(await drag(head, "dragenter", data)).toBe(false);
  expect(await drag(head, "dragover", data)).toBe(false);
  await expect(dialog.locator(".compose-drop")).toHaveCount(0);
  expect(await drag(head, "drop", data)).toBe(false);
  const box = dialog.getByRole("textbox", { name: "What's happening?" });
  await paste(box, [await drawn(page, "shot.png")]);
  await expectDraft(box, "");
  await paste(box, [await drawn(page, "shot.png")], { "text/plain": "just text" });
  await expectDraft(box, "just text");
  await expect(tiles(dialog)).toHaveCount(0);
  await expect(note(dialog)).toHaveText("");
}

test("where /api/v1/media answers 404 (no R2 bucket bound), the composer has no attach button and takes no files", async ({ page }) => {
  const media = new Media(null);
  await ready(page, media);
  const asked = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/v1/media");
  await page.getByRole("button", { name: "Write a cast" }).click();
  expect((await asked).status()).toBe(404);
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await takesNoFiles(page, dialog);
  expect(media.asked).toBe(1);
  expect(media.posts).toBe(0);
  // It posts as it did.
  await dialog.getByRole("button", { name: "Cast", exact: true }).click();
  await expect(dialog).toBeHidden();
  // Opened again, it is the same.
  await page.getByRole("button", { name: "Write a cast" }).click();
  const again = page.getByRole("dialog", { name: "New cast" });
  await expect(again.getByRole("button", { name: "Add a sticker" })).toBeVisible();
  await expect(again.getByRole("button", { name: "Attach images or videos" })).toHaveCount(0);
});

test("signed out, the composer doesn't ask whether it may attach, and takes no files", async ({ page }) => {
  const media = new Media();
  await watchUploads(page);
  await serve(page, media, { key: false });
  await page.goto("/");
  await page.getByRole("button", { name: "Write a cast" }).click();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await takesNoFiles(page, dialog);
  expect(media.asked).toBe(0);
  expect(media.posts).toBe(0);
});

test("at 390, the attach button opens the phone's photos, takes a 44 px tap, and the composer fits; a PNG and an MP4 go out and show", async ({ browser }, info) => {
  const { context, page } = await phone(browser, info);
  const media = new Media();
  media.refuse.push([507, "You've attached 60 files in the past 24 hours, the most a day takes"]);
  const calls = await ready(page, media);
  expect(await page.evaluate(() => matchMedia("(pointer: coarse)").matches)).toBe(true);
  await page.getByRole("button", { name: "Write a cast" }).tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  const button = dialog.getByRole("button", { name: "Attach images or videos" });
  await expect(button).toBeVisible();
  const band = await button.evaluate((el) => {
    const after = getComputedStyle(el, "::after");
    return { width: parseFloat(after.width), height: parseFloat(after.height) };
  });
  expect(band.width).toBeGreaterThanOrEqual(44);
  expect(band.height).toBeGreaterThanOrEqual(44);
  // The bar's buttons and Cast on one line, in the dialog.
  expect(await fits(dialog)).toBe(true);
  // A file the Worker refuses: why, as it says, and the error tile.
  await dialog.getByRole("textbox").fill("Sunday");
  await attach(page, dialog, [await drawn(page, "IMG_0001.jpg", "image/jpeg", 330)], true);
  await expect(dialog.getByRole("alert")).toContainText("IMG_0001.jpg: You've attached 60 files in the past 24 hours, the most a day takes");
  await expect(dialog).toBeInViewport({ ratio: 1 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: info.outputPath("attach-390-error.png") });
  await dialog.getByRole("button", { name: "Remove" }).last().tap();
  await expect(tiles(dialog)).toHaveCount(0);
  // A picture and a video from the picker, on their way.
  media.held = true;
  const clip = await recorded(page, "clip.mp4", "video/mp4");
  await attach(page, dialog, [await drawn(page, "IMG_0002.png", "image/png", 190, 480, 640), clip], true);
  await expect(tiles(dialog)).toHaveCount(2);
  const video = tiles(dialog).nth(1).locator("video");
  await expect(video).toHaveAttribute("playsinline", "");
  await expect(video).toHaveAttribute("preload", "metadata");
  await expect(video).toHaveJSProperty("muted", true);
  await expect(tiles(dialog).nth(1)).toContainText("Video");
  await expect.poll(() => media.posts).toBe(3);
  await progress(page, 1, 6_200, 10_000);
  await progress(page, 2, 2_500, 10_000);
  await expect(dialog.getByRole("progressbar")).toHaveCount(2);
  await expect(dialog.getByRole("button", { name: "Uploading…" })).toBeDisabled();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await fits(dialog)).toBe(true);
  await page.waitForTimeout(300);
  await page.screenshot({ path: info.outputPath("attach-390-uploading.png") });
  media.release();
  await dialog.getByRole("button", { name: "Cast", exact: true }).tap();
  await expect(dialog).toBeHidden();
  // In the order they were picked, whichever was in first.
  const of = (type: string) => media.uploads.find((u) => u.type === type)!;
  const urls = [of("image/png").url, of("video/mp4").url];
  const [sent] = calls.sent;
  expect(castOf(sent).embeds).toEqual(urls.map((url) => ({ url })));
  // A video goes as it is.
  expect(of("video/mp4").body.equals(clip.buffer)).toBe(true);
  // Once the node has it, its card shows the picture, and the video, which plays.
  calls.feed.push(listed(sent.hash, "Sunday", [{ kind: "image", url: urls[0] }, { kind: "video", url: urls[1] }]));
  await page.reload();
  const card = page.locator(".cast-card:not(.outgoing)").filter({ hasText: "Sunday" });
  await showsMedia(card, urls[0], urls[1]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  await page.screenshot({ path: info.outputPath("attach-390-posted.png") });
  await context.close();
});

test("at 390, where Allthing doesn't post, the bar still fits with Post in Farcaster", async ({ browser }, info) => {
  const { context, page } = await phone(browser, info);
  const media = new Media();
  await ready(page, media, { key: false });
  await page.getByRole("button", { name: "Write a cast" }).tap();
  const dialog = page.getByRole("dialog", { name: "New cast" });
  await expect(dialog.getByRole("button", { name: "Attach images or videos" })).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Post in Farcaster" })).toBeVisible();
  expect(await fits(dialog)).toBe(true);
  media.held = true;
  await attach(page, dialog, [await drawn(page, "IMG_0003.jpg", "image/jpeg", 80)], true);
  await expect(dialog.getByRole("button", { name: "Uploading…" })).toBeDisabled();
  expect(await fits(dialog)).toBe(true);
  media.release();
  // On a phone, the draft's link opens the Farcaster app, with the file.
  const link = dialog.getByRole("link", { name: "Post in Farcaster" });
  await expect(link).toBeVisible();
  expect(await fits(dialog)).toBe(true);
  expect(new URL((await link.getAttribute("href"))!).searchParams.getAll("embeds[]")).toEqual(media.uploads.map((u) => u.url));
  await context.close();
});
