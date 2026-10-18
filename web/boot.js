// SPDX-License-Identifier: AGPL-3.0-or-later
// Loads the WebAssembly app. www.allthing.social is an alias: send readers
// to the canonical host first.
if (location.hostname === "www.allthing.social") {
  location.replace("https://allthing.social" + location.pathname + location.search + location.hash);
} else {
  // The front page's data does not depend on the app: start fetching it now,
  // alongside the WebAssembly download, and let the app pick the response up
  // (api.rs, early_response). Only on the Worker-backed site; the static copy
  // has no API.
  if ("{{BASE}}" === "" && location.pathname === "/") {
    window.__allthingEarly = { path: "/api/v1/home", response: fetch("/api/v1/home") };
  }
  const { default: init } = await import("./{{JS}}");
  await init({ module_or_path: "{{BASE}}/assets/{{WASM}}" });
}
