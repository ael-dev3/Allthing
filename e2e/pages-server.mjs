// SPDX-License-Identifier: AGPL-3.0-or-later
// Serves dist-pages/ the way GitHub Pages does, for the end-to-end tests:
// under a sub-path (/Allthing), "dir/" -> "dir/index.html", a redirect from
// "/Allthing" to "/Allthing/", and 404.html (with status 404) for anything else.
//
//   node e2e/pages-server.mjs [port] [base] [root]
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] ?? 4173);
const base = (process.argv[3] ?? "/Allthing").replace(/\/$/, "");
const root = process.argv[4] ? path.resolve(process.argv[4]) : path.resolve(import.meta.dirname, "../dist-pages");
const types = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".json": "application/json",
  ".txt": "text/plain; charset=utf-8",
};

async function file(relative) {
  const full = path.join(root, relative);
  const fromRoot = path.relative(root, full);
  if (fromRoot === ".." || fromRoot.startsWith(`..${path.sep}`) || path.isAbsolute(fromRoot)) return null;
  try {
    const info = await stat(full);
    return info.isDirectory() ? file(path.join(relative, "index.html")) : { full, body: await readFile(full) };
  } catch {
    return null;
  }
}

const server = createServer(async (req, res) => {
  let url, relative;
  try {
    url = new URL(req.url ?? "/", "http://localhost");
    relative = url.pathname.startsWith(`${base}/`) ? decodeURIComponent(url.pathname.slice(base.length)) : null;
  } catch {
    res.writeHead(400, { "content-type": "text/plain; charset=utf-8" }).end("Bad request");
    return;
  }
  if (url.pathname === base) {
    res.writeHead(301, { location: `${base}/${url.search}` }).end();
    return;
  }
  const found = relative === null ? null : await file(relative);
  const served = found ?? (await file("404.html"));
  const type = types[path.extname(served?.full ?? ".html")] ?? "application/octet-stream";
  res.writeHead(found ? 200 : 404, { "content-type": type, "cache-control": "no-cache" });
  res.end(served?.body ?? "Not found");
});
server.listen(port, "127.0.0.1", () => console.log(`dist-pages on http://127.0.0.1:${server.address().port}${base}/`));
