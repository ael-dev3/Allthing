// SPDX-License-Identifier: AGPL-3.0-or-later
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { createHmac } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { deflateSync } from "node:zlib";
import { expect, test } from "@playwright/test";

/*
 * Readers' attachments on the real Worker, with a local R2: a signed-in
 * reader uploads an image and a video, the Worker keeps each in R2 without
 * reading it and records it in D1, and anyone gets it back from /m/, byte
 * for byte, in ranges, and only ever as the type it was declared as. This
 * spec runs a Worker of its own, from wrangler.toml and its R2 binding (a
 * config in a temporary folder), with an empty local R2 and D1 and a
 * session secret; and one more from wrangler.toml without the binding, as
 * a server that keeps no attachments has it.
 */

test.skip(Boolean(process.env.E2E_PAGES), "the static copy has no Worker");
test.skip(Boolean(process.env.E2E_BASE_URL), "runs a Worker of its own");

const PORT = Number(process.env.E2E_MEDIA_PORT ?? 8793);
const ORIGIN = `http://127.0.0.1:${PORT}`;
const SECRET = "media-upload-end-to-end-session-secret-for-tests-only";
const ROOT = path.join(__dirname, "..");
const MIB = 1024 * 1024;
const now = () => Math.floor(Date.now() / 1000);

let worker: ChildProcess | undefined;
let folder = "";
let config = "";

/** A session cookie as the Worker's sign-in sets it: `<fid>.<expiry>.<HMAC-SHA256>`. */
function session(fid: number) {
  const value = `${fid}.${now() + 3_600}`;
  const tag = createHmac("sha256", SECRET).update(`allthing session\n${value}`).digest("hex");
  return `__Host-allthing-session=${value}.${tag}`;
}

/** wrangler.toml with its R2 block (uncommented, if it is commented out), and its paths made absolute. */
function withBucket(toml: string) {
  const lines = toml.split("\n");
  const start = lines.indexOf("# [[r2_buckets]]");
  if (start < 0) {
    if (!/^\[\[r2_buckets\]\]$/m.test(toml)) throw new Error("wrangler.toml has no R2 block, commented or not");
  } else {
    for (let at = start; at === start || /^# \w+ = /.test(lines[at] ?? ""); at += 1) lines[at] = lines[at].slice(2);
  }
  return absolute(lines.join("\n"));
}

/** wrangler.toml without its R2 block, as a server that keeps no attachments has it. */
function withoutBucket(toml: string) {
  const bare = toml.replace(/^(?:# )?\[\[r2_buckets\]\]\n(?:(?:# )?\w+ = .*\n)+/m, "");
  if (/r2_buckets/.test(bare.replace(/^#.*$/gm, ""))) throw new Error("wrangler.toml's R2 block wasn't removed");
  return absolute(bare);
}

/** `toml`'s paths, made absolute for a config written elsewhere. */
function absolute(toml: string) {
  return toml
    .replace(/^main = "(.+)"$/m, (_, file) => `main = ${JSON.stringify(path.join(ROOT, file))}`)
    .replace(/^directory = "(.+)"$/m, (_, dir) => `directory = ${JSON.stringify(path.join(ROOT, dir))}`)
    .replace(/^migrations_dir = "(.+)"$/m, (_, dir) => `migrations_dir = ${JSON.stringify(path.join(ROOT, dir))}`);
}

/** A query on the spec's local D1, as the Worker sees it. */
function d1(sql: string): Record<string, number>[] {
  const out = execFileSync(
    "npx",
    ["wrangler", "d1", "execute", "allthing-index", "--local", "--config", config, "--persist-to", path.join(folder, "state"), "--json", "--command", sql],
    { cwd: ROOT, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
  );
  return JSON.parse(out)[0].results;
}

/** A PNG of `width` × 1 pixels, real enough for any viewer. */
function png(width: number) {
  const crc = (bytes: Buffer) => {
    let c = ~0;
    for (const byte of bytes) {
      c ^= byte;
      for (let k = 0; k < 8; k += 1) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
    }
    return ~c >>> 0;
  };
  const chunk = (type: string, data: Buffer) => {
    const body = Buffer.concat([Buffer.from(type, "latin1"), data]);
    const out = Buffer.alloc(body.length + 8);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc(body), body.length + 4);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(1, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(width * 3, 0x5a)]);
  return Buffer.concat([Buffer.from("89504e470d0a1a0a", "hex"), chunk("IHDR", header), chunk("IDAT", deflateSync(row)), chunk("IEND", Buffer.alloc(0))]);
}

type Upload = { status: number; json: { url?: string; type?: string; bytes?: number; error?: string; retry_after?: number }; headers: Headers };

/** POST `body` to the spec's Worker as `type`, from Allthing itself unless told otherwise. */
async function upload(body: Buffer | string, type: string | null, fid: number | null, origin: string | null = ORIGIN, hls?: string): Promise<Upload> {
  const headers: Record<string, string> = {};
  if (hls !== undefined) headers["x-allthing-hls"] = hls;
  if (type !== null) headers["content-type"] = type;
  if (fid !== null) headers.cookie = session(fid);
  if (origin !== null) headers.origin = origin;
  const res = await fetch(`${ORIGIN}/api/v1/media`, { method: "POST", headers, body });
  return { status: res.status, json: await res.json(), headers: res.headers };
}

/** Where the spec's Worker serves an uploaded file: the path of its URL, here. */
const local = (url: string) => `${ORIGIN}${new URL(url).pathname}`;

test.beforeAll(async () => {
  test.setTimeout(150_000);
  folder = mkdtempSync(path.join(tmpdir(), "allthing-media-"));
  config = path.join(folder, "wrangler.toml");
  writeFileSync(config, withBucket(readFileSync(path.join(ROOT, "wrangler.toml"), "utf8")));
  worker = spawn(
    "npx",
    ["wrangler", "dev", "--config", config, "--port", String(PORT), "--ip", "127.0.0.1", "--persist-to", path.join(folder, "state"), "--var", `SESSION_SECRET:${SECRET}`],
    { cwd: ROOT, stdio: "ignore", detached: true },
  );
  await expect
    .poll(async () => (await fetch(`${ORIGIN}/api/v1/status`).catch(() => null))?.status ?? 0, { timeout: 120_000, intervals: [1_000] })
    .toBe(200);
});

test.afterAll(async () => {
  if (worker?.pid) process.kill(-worker.pid, "SIGTERM");
  if (folder) rmSync(folder, { recursive: true, force: true });
});

test("a reader uploads an image and a video, and anyone gets them back byte for byte, in ranges too", async () => {
  const status = await (await fetch(`${ORIGIN}/api/v1/status`)).json();
  expect(status.media).toEqual({ ready: true });
  // The composer asks whether it may attach: yes, for a session, kept 5 minutes.
  const asked = await fetch(`${ORIGIN}/api/v1/media`, { headers: { cookie: session(1234) } });
  expect(asked.status).toBe(200);
  expect(asked.headers.get("cache-control")).toBe("private, max-age=300");
  expect(await asked.json()).toEqual({ max_image_bytes: 10 * MIB, max_video_bytes: 50 * MIB, max_embeds: 2 });
  expect((await fetch(`${ORIGIN}/api/v1/media`)).status).toBe(401);

  // The URL is on the origin the Worker was asked at (wrangler dev presents
  // requests at the route's host), as its sign-in names it.
  const started = await (await fetch(`${ORIGIN}/api/v1/auth/start`, { method: "POST", headers: { origin: ORIGIN } })).json();
  const image = png(40);
  const video = readFileSync(path.join(__dirname, "fixtures/hls/init_0.mp4"));
  const files = [
    { body: image, type: "image/png", extension: "png" },
    { body: video, type: "video/mp4", extension: "mp4" },
  ];
  for (const file of files) {
    const sent = await upload(file.body, file.type, 1234);
    expect(sent.status).toBe(200);
    expect(sent.headers.get("cache-control")).toBe("no-store");
    expect(sent.json.type).toBe(file.type);
    expect(sent.json.bytes).toBe(file.body.length);
    const url = sent.json.url!;
    const at = started.uri.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    expect(url).toMatch(new RegExp(`^${at}/m/1234/[0-9a-f]{32}\\.${file.extension}$`));
    expect(Buffer.byteLength(url)).toBeLessThanOrEqual(256);

    // Anyone, without a session, gets it back as it was sent.
    const got = await fetch(local(url));
    expect(got.status).toBe(200);
    expect(Buffer.from(await got.arrayBuffer()).equals(file.body)).toBe(true);
    const headers = Object.fromEntries(got.headers);
    expect(headers).toMatchObject({
      "content-type": file.type,
      "content-length": String(file.body.length),
      "cache-control": "public, max-age=31536000, immutable",
      "accept-ranges": "bytes",
      "x-content-type-options": "nosniff",
      "cross-origin-resource-policy": "cross-origin",
      "access-control-allow-origin": "*",
    });
    expect(headers["content-security-policy"]).toMatch(/^default-src 'none'; .*; sandbox$/);
    expect(headers.etag).toMatch(/^"[0-9a-f]+"$/);

    // HEAD: the same, without the body.
    const head = await fetch(local(url), { method: "HEAD" });
    expect(head.status).toBe(200);
    expect(head.headers.get("content-length")).toBe(String(file.body.length));
    expect(head.headers.get("etag")).toBe(headers.etag);
    // A copy the browser already has: 304, nothing sent.
    const again = await fetch(local(url), { headers: { "if-none-match": headers.etag } });
    expect(again.status).toBe(304);
    expect((await again.arrayBuffer()).byteLength).toBe(0);
    expect((await fetch(local(url), { headers: { "if-none-match": '"other"' } })).status).toBe(200);
  }

  // Ranges, as Safari asks for video: each the bytes it names, and only them.
  const url = local((await upload(video, "video/mp4", 1234)).json.url!);
  const size = video.length;
  for (const [range, first, last] of [
    ["bytes=0-1", 0, 1],
    ["bytes=100-199", 100, 199],
    ["bytes=500-", 500, size - 1],
    ["bytes=-64", size - 64, size - 1],
    ["bytes=-100000", 0, size - 1],
    ["bytes=700-99999999", 700, size - 1],
  ] as const) {
    const part = await fetch(url, { headers: { range } });
    expect(part.status, range).toBe(206);
    expect(part.headers.get("content-range"), range).toBe(`bytes ${first}-${last}/${size}`);
    expect(part.headers.get("content-length"), range).toBe(String(last - first + 1));
    expect(Buffer.from(await part.arrayBuffer()).equals(video.subarray(first, last + 1)), range).toBe(true);
  }
  for (const range of [`bytes=${size}-`, `bytes=${size + 10}-${size + 20}`, "bytes=-0"]) {
    const none = await fetch(url, { headers: { range } });
    expect(none.status, range).toBe(416);
    expect(none.headers.get("content-range"), range).toBe(`bytes */${size}`);
  }
  // Several ranges, or another unit: the whole file, as HTTP allows.
  for (const range of ["bytes=0-1,4-5", "items=0-1"]) {
    const whole = await fetch(url, { headers: { range } });
    expect(whole.status, range).toBe(200);
    expect((await whole.arrayBuffer()).byteLength, range).toBe(size);
  }

  // D1 counts each file once R2 has it: three, and their bytes, in the
  // reader's rows, the total and the day's count.
  const bytes = image.length + 2 * video.length;
  expect(d1("SELECT COUNT(*) AS files, SUM(bytes) AS bytes FROM media WHERE fid = 1234")).toEqual([{ files: 3, bytes }]);
  expect(d1("SELECT bytes, files FROM media_total WHERE name = 'all'")).toEqual([{ bytes, files: 3 }]);
  expect(d1("SELECT SUM(bytes) AS bytes, SUM(files) AS files FROM media_total WHERE name LIKE 'd%'")).toEqual([{ bytes, files: 3 }]);
});

test("a video remuxed for HLS is embedded as its playlist, which the Worker writes from R2's metadata, for the Farcaster app", async () => {
  // A fragmented MP4 as the composer remuxes one (allthing_core::fmp4), and its index.
  const video = readFileSync(path.join(__dirname, "fixtures/remuxed/video.mp4"));
  const index = "1;64x36;avc1.64000a,mp4a.40.2;1258;4000:19463,4000:18474,4000:18667";
  const sent = await upload(video, "video/mp4", 4321, ORIGIN, index);
  expect(sent.status, sent.json.error).toBe(200);
  expect(sent.json.type).toBe("video/mp4");
  expect(sent.json.bytes).toBe(video.length);
  const url = sent.json.url!;
  expect(url).toMatch(/\/m\/4321\/[0-9a-f]{32}\.m3u8$/);
  const id = url.match(/([0-9a-f]{32})\.m3u8$/)![1];

  // The master playlist: one rendition, with its size and codecs, as the Farcaster app reads them.
  const master = await fetch(local(url));
  expect(master.status).toBe(200);
  const body = await master.text();
  expect(body).toBe(
    "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-INDEPENDENT-SEGMENTS\n" +
      `#EXT-X-STREAM-INF:BANDWIDTH=38926,AVERAGE-BANDWIDTH=38575,RESOLUTION=64x36,CODECS="avc1.64000a,mp4a.40.2"\n${id}.v.m3u8\n`,
  );
  const headers = Object.fromEntries(master.headers);
  expect(headers).toMatchObject({
    "content-type": "application/vnd.apple.mpegurl",
    "content-length": String(Buffer.byteLength(body)),
    "cache-control": "public, max-age=31536000, immutable",
    "access-control-allow-origin": "*",
    "x-content-type-options": "nosniff",
  });
  expect(headers.etag).toMatch(/^"[0-9a-f]+-m3u8"$/);
  expect(headers["accept-ranges"]).toBeUndefined();
  const head = await fetch(local(url), { method: "HEAD" });
  expect(head.status).toBe(200);
  expect(head.headers.get("content-type")).toBe("application/vnd.apple.mpegurl");
  expect(head.headers.get("content-length")).toBe(String(Buffer.byteLength(body)));
  expect((await fetch(local(url), { headers: { "if-none-match": headers.etag } })).status).toBe(304);

  // The media playlist: each segment a byte range of the MP4, and those ranges are its fragments.
  const media = await (await fetch(local(url.replace(/\.m3u8$/, ".v.m3u8")))).text();
  expect(media).toContain(`#EXT-X-MAP:URI="${id}.mp4",BYTERANGE="1258@0"`);
  const ranges = [...media.matchAll(/#EXTINF:([\d.]+),\n#EXT-X-BYTERANGE:(\d+)@(\d+)\n(\S+)/g)].map((m) => [Number(m[1]), Number(m[2]), Number(m[3]), m[4]]);
  expect(ranges).toEqual([
    [4, 19463, 1258, `${id}.mp4`],
    [4, 18474, 20721, `${id}.mp4`],
    [4, 18667, 39195, `${id}.mp4`],
  ]);
  expect(media.endsWith("#EXT-X-ENDLIST\n")).toBe(true);
  const mp4 = local(url.replace(/\.m3u8$/, ".mp4"));
  for (const [, length, offset] of ranges as number[][]) {
    const part = await fetch(mp4, { headers: { range: `bytes=${offset}-${offset + length - 1}` } });
    expect(part.status).toBe(206);
    const bytes = Buffer.from(await part.arrayBuffer());
    expect(bytes.subarray(4, 8).toString("latin1"), "each segment starts with its moof").toBe("moof");
    expect(bytes.equals(video.subarray(offset, offset + length))).toBe(true);
  }
  // A player on another site asks first: any may, with a Range.
  const preflight = await fetch(mp4, { method: "OPTIONS", headers: { origin: "https://farcaster.xyz", "access-control-request-method": "GET", "access-control-request-headers": "range" } });
  expect(preflight.status).toBe(204);
  expect(preflight.headers.get("access-control-allow-origin")).toBe("*");
  expect(preflight.headers.get("access-control-allow-headers")).toContain("range");
  expect((await fetch(mp4)).headers.get("access-control-expose-headers")).toContain("content-range");

  // A video kept without an index (one that couldn't be remuxed, or uploaded before) has no playlist.
  const plain = await upload(video, "video/mp4", 4321);
  expect(plain.json.url).toMatch(/\.mp4$/);
  expect((await fetch(local(plain.json.url!.replace(/\.mp4$/, ".m3u8")))).status).toBe(404);
  expect((await fetch(local(plain.json.url!.replace(/\.mp4$/, ".v.m3u8")))).status).toBe(404);
  expect((await fetch(`${ORIGIN}/m/4321/${"0".repeat(32)}.m3u8`)).status).toBe(404);

  // An index that isn't the file's, or with anything but an MP4, is refused, and nothing is kept.
  const [{ files }] = d1("SELECT COUNT(*) AS files FROM media WHERE fid = 4321");
  for (const [body, type, line] of [
    [video, "video/mp4", "1;64x36;avc1.64000a;1258;4000:19463"],
    [video, "video/mp4", "1;64x36;avc1\",x=\"y;1258;4000:19463,4000:18474,4000:18667"],
    [video, "video/mp4", "nonsense"],
    [video, "video/quicktime", index],
    [png(4), "image/png", "1;4x1;avc1.64000a;10;4000:" + (png(4).length - 10)],
  ] as const) {
    const refused = await upload(body, type, 4321, ORIGIN, line);
    expect(refused.status, line).toBe(400);
    expect(refused.json.error).toBe("That video's stream index doesn't match it; try again");
  }
  expect(d1("SELECT COUNT(*) AS files FROM media WHERE fid = 4321")).toEqual([{ files }]);
});

test("a file is served only as the type it was declared as, and never runs as a page", async ({ page }) => {
  const html = '<!doctype html><title>ran as a page</title><script>document.title = "script ran"; fetch("/leak")</script><h1>Hi</h1>';
  const sent = await upload(html, "image/png", 99);
  expect(sent.status).toBe(200);
  const requests: string[] = [];
  page.on("request", (request) => requests.push(new URL(request.url()).pathname));
  const res = await page.goto(local(sent.json.url!));
  expect(res!.headers()["content-type"]).toBe("image/png");
  // The browser shows it as a (broken) image: neither its title nor its
  // heading, and its script neither ran nor fetched anything.
  await expect(page.locator("img")).toHaveCount(1);
  await expect(page.locator("h1")).toHaveCount(0);
  expect(await page.title()).not.toMatch(/ran/);
  expect(requests).toEqual([new URL(sent.json.url!).pathname]);

  // A real image opened in a tab of its own shows in the browser's viewer,
  // and a video plays there: the policy lets the browser's own viewer load
  // the file (with default-src 'none' alone, the video wouldn't load).
  const image = await upload(png(3), "image/png", 99);
  await page.goto(local(image.json.url!));
  await expect.poll(() => page.locator("img").evaluate((img: HTMLImageElement) => [img.naturalWidth, img.naturalHeight])).toEqual([3, 1]);
  const parts = ["init_0.mp4", "seg_0_0.m4s", "seg_0_1.m4s", "seg_0_2.m4s"];
  const clip = Buffer.concat(parts.map((part) => readFileSync(path.join(__dirname, "fixtures/hls", part))));
  const video = await upload(clip, "video/mp4", 99);
  const errors: string[] = [];
  page.on("console", (message) => message.type() === "error" && errors.push(message.text()));
  await page.goto(local(video.json.url!));
  await expect
    .poll(() => page.locator("video").evaluate((v: HTMLVideoElement) => [v.readyState >= 1, v.videoWidth, v.error?.code ?? null]))
    .toEqual([true, 320, null]);
  expect(errors).toEqual([]);
});

test("uploads are refused clearly: their type, size, session, origin and pace", async () => {
  const image = png(4);
  const refused = async (sent: Promise<Upload>, status: number, error: RegExp) => {
    const { status: got, json, headers } = await sent;
    expect(got, json.error).toBe(status);
    expect(json.error).toMatch(error);
    expect(headers.get("cache-control")).toBe("no-store");
    return { json, headers };
  };
  await refused(upload(image, "text/html", 7), 400, /^Attach a PNG, JPEG, GIF or WebP image, or an MP4, MOV or WebM video$/);
  await refused(upload(image, "image/svg+xml", 7), 400, /^Attach a PNG/);
  await refused(upload(image, null, 7), 400, /^Attach a PNG/);
  await refused(upload(Buffer.alloc(0), "image/png", 7), 400, /^That file is empty$/);
  await refused(upload(Buffer.alloc(10 * MIB + 1), "image/jpeg", 7), 413, /^Images can be up to 10 MB$/);
  await refused(upload(Buffer.alloc(50 * MIB + 1), "video/webm", 7), 413, /^Videos can be up to 50 MB$/);
  await refused(upload(image, "image/png", null), 401, /^Sign in to attach images and videos$/);
  await refused(upload(image, "image/png", 7, "https://elsewhere.example"), 403, /^Uploads must come from Allthing itself$/);
  await refused(upload(image, "image/png", 7, null), 403, /^Uploads must come from Allthing itself$/);
  expect((await fetch(`${ORIGIN}/api/v1/media`, { method: "PUT", headers: { origin: ORIGIN } })).status).toBe(405);

  // Twenty uploads a minute per reader, then a wait.
  for (let i = 0; i < 20; i += 1) expect((await upload(image, "image/png", 8)).status).toBe(200);
  const { headers } = await refused(upload(image, "image/png", 8), 429, /^That's a lot of uploads at once; try again in a moment$/);
  expect(Number(headers.get("retry-after"))).toBeGreaterThan(0);
  expect((await upload(image, "image/png", 9)).status, "another reader has their own").toBe(200);

  // Nothing refused was kept.
  expect(d1("SELECT COUNT(*) AS files FROM media WHERE fid = 7")).toEqual([{ files: 0 }]);
});

test("quotas: 60 files a reader a day, 2,000 uploads a day in all, and 9 GiB stored", async () => {
  const image = png(2);
  // A reader with 60 files in the past day may add no more until the
  // oldest is a day old.
  const values = Array.from({ length: 60 }, (_, i) => `('11/${i.toString(16).padStart(32, "0")}.png', 11, 10, 'image/png', ${now() - 3_600 + i})`);
  d1(`INSERT INTO media (key, fid, bytes, type, created_at) VALUES ${values.join(", ")}`);
  const full = await upload(image, "image/png", 11);
  expect(full.status).toBe(507);
  expect(full.json.error).toBe("You've attached 60 files in the past 24 hours, the most a day takes");
  expect(Number(full.headers.get("retry-after"))).toBeGreaterThan(82_000);
  // Files older than a day don't count.
  d1(`UPDATE media SET created_at = created_at - 86400 WHERE fid = 11 AND created_at < ${now() - 3_000}`);
  expect((await upload(image, "image/png", 11)).status).toBe(200);

  // 2,000 uploads a UTC day from everyone, then none until midnight.
  const day = Math.floor(now() / 86_400);
  const days = `('d${day}', 0, 2000), ('d${day + 1}', 0, 2000)`;
  const [{ files }] = d1(`SELECT COALESCE(SUM(files), 0) AS files FROM media_total WHERE name = 'd${day}'`);
  d1(`INSERT INTO media_total (name, bytes, files) VALUES ${days} ON CONFLICT (name) DO UPDATE SET files = 2000`);
  const busy = await upload(image, "image/png", 13);
  expect(busy.status).toBe(507);
  expect(busy.json.error).toBe("Allthing has taken all the attachments it can today; try again tomorrow");
  expect(Number(busy.headers.get("retry-after"))).toBeLessThanOrEqual(86_400);
  d1(`UPDATE media_total SET files = ${files} WHERE name = 'd${day}'`);
  d1(`DELETE FROM media_total WHERE name = 'd${day + 1}'`);
  expect((await upload(image, "image/png", 13)).status).toBe(200);

  // Everyone's files together stay under R2's free 10 GB-month: 9 GiB.
  const [{ bytes }] = d1("SELECT bytes FROM media_total WHERE name = 'all'");
  d1(`UPDATE media_total SET bytes = ${9 * 1024 * MIB - image.length + 1} WHERE name = 'all'`);
  const over = await upload(image, "image/png", 12);
  expect(over.status).toBe(507);
  expect(over.json.error).toBe("Allthing has no room for more attachments just now");
  d1(`UPDATE media_total SET bytes = ${9 * 1024 * MIB - image.length} WHERE name = 'all'`);
  expect((await upload(image, "image/png", 12)).status, "a file that fills it exactly").toBe(200);
  d1(`UPDATE media_total SET bytes = ${bytes} WHERE name = 'all'`);
});

test("without the R2 binding, uploads and /m/ answer 404", async () => {
  test.setTimeout(150_000);
  // A Worker of its own, from wrangler.toml without its R2 block.
  const bare = Number(process.env.E2E_MEDIA_BARE_PORT ?? 8799);
  const origin = `http://127.0.0.1:${bare}`;
  const toml = path.join(folder, "bare.toml");
  writeFileSync(toml, withoutBucket(readFileSync(path.join(ROOT, "wrangler.toml"), "utf8")));
  const server = spawn(
    "npx",
    ["wrangler", "dev", "--config", toml, "--port", String(bare), "--ip", "127.0.0.1", "--persist-to", path.join(folder, "bare-state"), "--var", `SESSION_SECRET:${SECRET}`],
    { cwd: ROOT, stdio: "ignore", detached: true },
  );
  try {
    await expect
      .poll(async () => (await fetch(`${origin}/api/v1/status`).catch(() => null))?.status ?? 0, { timeout: 120_000, intervals: [1_000] })
      .toBe(200);
    const status = await (await fetch(`${origin}/api/v1/status`)).json();
    expect(status.media).toEqual({ ready: false });
    const sent = await fetch(`${origin}/api/v1/media`, { method: "POST", headers: { origin, "content-type": "image/png", cookie: session(1234) }, body: png(2) });
    expect(sent.status).toBe(404);
    expect((await sent.json()).error).toBe("Attachments aren't set up on this server");
    expect((await fetch(`${origin}/api/v1/media`)).status).toBe(404);
    const file = await fetch(`${origin}/m/1234/${"0".repeat(32)}.png`);
    expect(file.status).toBe(404);
    expect(file.headers.get("content-type")).toMatch(/^application\/json/);
  } finally {
    if (server.pid) process.kill(-server.pid, "SIGTERM");
  }
});
