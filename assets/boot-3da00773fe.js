// SPDX-License-Identifier: AGPL-3.0-or-later
// Loads the WebAssembly app. www.allthing.social is an alias: send readers
// to the canonical host first.
if (location.hostname === "www.allthing.social") {
  location.replace("https://allthing.social" + location.pathname + location.search + location.hash);
} else {
  const { default: init } = await import("./allthing-7fce5bee08.js");
  await init({ module_or_path: "/Allthing/assets/allthing-7c252673d2.wasm" });
}
