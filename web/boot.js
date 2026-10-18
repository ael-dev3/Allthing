// SPDX-License-Identifier: AGPL-3.0-or-later
// Loads the WebAssembly app. www.allthing.social is an alias: send readers
// to the canonical host first.
if (location.hostname === "www.allthing.social") {
  location.replace("https://allthing.social" + location.pathname + location.search + location.hash);
} else {
  try {
    const module = compiled();
    const { default: init } = await import("./{{JS}}");
    await init({ module_or_path: await module });
  } catch (error) {
    console.error("Allthing could not load", error);
    const boot = document.getElementById("boot");
    boot?.querySelector(".boot-glow")?.remove();
    boot?.removeAttribute("aria-hidden");

    const main = document.createElement("main");
    main.id = "main";
    main.className = "page";
    const panel = document.createElement("section");
    panel.className = "error-panel";
    panel.setAttribute("role", "alert");
    const title = document.createElement("h1");
    title.textContent = "Allthing couldn’t load";
    const message = document.createElement("p");
    message.textContent = "Reload to try again.";
    const reload = document.createElement("button");
    reload.type = "button";
    reload.className = "button";
    reload.textContent = "Reload";
    reload.addEventListener("click", () => location.reload());
    panel.append(title, message, reload);
    main.append(panel);
    const root = document.getElementById("app");
    if (root) root.replaceChildren(main);
    else document.body.append(main);
    if (document.activeElement === document.body) reload.focus();
  }
}

// The Worker's copy of the module, compressed at brotli's highest level, or
// the static copy if the Worker can't answer (its daily allowance spent,
// say). Only fetching and compiling fall back: an error while the app starts
// still shows the panel.
async function compiled() {
  const fallback = "{{BASE}}/assets/{{WASM}}";
  if ("{{WASM_URL}}" === fallback) return fallback;
  try {
    return await WebAssembly.compileStreaming(fetch("{{WASM_URL}}"));
  } catch (error) {
    console.warn("Allthing: loading the static copy of the app", error);
    return fallback;
  }
}
