// SPDX-License-Identifier: AGPL-3.0-or-later
// Loads the WebAssembly app. www.allthing.social is an alias: send readers
// to the canonical host first.
bootHeader();
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
    // The front page's first cards, drawn while the app was on its way
    // (paint.js), go: they can't be used without it.
    boot?.querySelector("main")?.remove();
    boot?.removeAttribute("data-painted");
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

// The boot screen's header as the app's will be: Home marked on the front
// page, and in the account's place what the app will show there, Sign in or
// the signed-in reader's picture (the user cookie says which), or the static
// copy's read-only note. index.html has the rest.
function bootHeader() {
  const account = document.getElementById("boot-account");
  if (!account) return;
  const base = "{{BASE}}";
  if (location.pathname === base + "/" || location.pathname === base) {
    account.closest(".header-inner")?.querySelector(".boot-nav span")?.setAttribute("aria-current", "page");
  }
  if (base) {
    account.outerHTML = '<span class="boot-readonly">read-only</span>';
  } else if (/(?:^|;\s*)__Host-allthing-user=[^;]/.test(document.cookie)) {
    account.outerHTML = '<span class="boot-actions"><span class="boot-account"><span class="boot-avatar" style="width: 32px; height: 32px;"></span></span></span>';
  } else {
    account.outerHTML =
      '<span class="boot-actions"><span class="boot-sign-in"><svg class="icon" viewBox="0 0 16 16"><circle cx="8" cy="5.5" r="2.6"/><path d="M3 13.5c.6-2.6 2.6-4 5-4s4.4 1.4 5 4"/></svg><span class="boot-sign-in-label">Sign in</span></span></span>';
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
