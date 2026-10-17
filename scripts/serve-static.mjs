#!/usr/bin/env node
// Serve the static export the way GitHub Pages does: under a base path,
// directories resolve to index.html (redirecting to add a trailing slash),
// and unknown paths get 404.html with a 404 status.
//
//   node scripts/serve-static.mjs [dir=out] [basePath=] [port=3200]

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";

const [dir = "out", basePath = "", port = "3200"] = process.argv.slice(2);
const root = resolve(dir);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".txt": "text/plain; charset=utf-8",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".png": "image/png",
  ".woff2": "font/woff2",
};

async function kind(path) {
  try {
    return (await stat(path)).isDirectory() ? "dir" : "file";
  } catch {
    return null;
  }
}

async function send(res, status, path) {
  res.writeHead(status, { "content-type": TYPES[extname(path)] ?? "application/octet-stream" });
  res.end(await readFile(path));
}

createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");
  let pathname;
  try {
    pathname = decodeURIComponent(url.pathname);
  } catch {
    pathname = url.pathname;
  }

  if (basePath && pathname !== basePath && !pathname.startsWith(`${basePath}/`)) {
    return send(res, 404, join(root, "404.html"));
  }
  const relative = pathname.slice(basePath.length) || "/";
  const target = resolve(root, `.${relative}`);
  if (target !== root && !target.startsWith(root + sep)) {
    res.writeHead(403).end();
    return;
  }

  const found = await kind(target);
  if (found === "dir") {
    if (!url.pathname.endsWith("/")) {
      res.writeHead(301, { location: `${url.pathname}/${url.search}` }).end();
      return;
    }
    const index = join(target, "index.html");
    if ((await kind(index)) === "file") return send(res, 200, index);
  } else if (found === "file") {
    return send(res, 200, target);
  }
  return send(res, 404, join(root, "404.html"));
}).listen(Number(port), () => {
  console.log(`serving ${root} at http://localhost:${port}${basePath}/`);
});
