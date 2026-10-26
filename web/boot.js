// SPDX-License-Identifier: AGPL-3.0-or-later
// Loads the WebAssembly app. www.allthing.social is an alias: send readers
// to the canonical host first.
const recoveryKey = "allthing-boot-recovery";
const recoveryQuery = "__allthing_boot";
const build = "{{WASM}}".match(/^allthing-([0-9a-f]{10})\.wasm$/)?.[1];
class AssetFailure extends Error {
  constructor(asset, absent, cause) {
    super(`Allthing ${asset} asset could not load`, { cause });
    this.asset = asset;
    this.absent = absent;
  }
}

bootHeader();
if (location.hostname === "www.allthing.social") {
  location.replace("https://allthing.social" + location.pathname + location.search + location.hash);
} else {
  let starting = false;
  try {
    // Settle both promises even when the other asset fails first. Errors
    // while init runs are app errors, never deployment-recovery signals.
    const [module, { default: init }] = await Promise.all([compiled(), javascript()]);
    cleanRecoveryURL();
    starting = true;
    await init({ module_or_path: module });
    try { sessionStorage.removeItem(recoveryKey); } catch { /* Storage may be disabled. */ }
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
    if (!starting) await recoverDeployment(error);
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
  if ("{{WASM_URL}}" !== fallback) {
    try {
      return await WebAssembly.compileStreaming(fetch("{{WASM_URL}}"));
    } catch (error) {
      console.warn("Allthing: loading the static copy of the app", error);
    }
  }
  try {
    const response = await fetch(fallback);
    if (missing(response)) {
      response.body?.cancel().catch(() => {});
      throw new AssetFailure("wasm", true);
    }
    return await WebAssembly.compileStreaming(response);
  } catch (error) {
    if (error instanceof AssetFailure) throw error;
    throw new AssetFailure("wasm", false, error);
  }
}

async function javascript() {
  try { return await import("./{{JS}}"); }
  catch (error) { throw new AssetFailure("js", false, error); }
}

// No extra requests on a successful boot. A failed asset can mean an old
// cached document survived a deployment. Revalidate that document only for
// a missing asset, never for corrupt Wasm, JS evaluation or app-start errors.
function missing(response) {
  return response.status === 404 || response.status === 410 ||
    (response.status === 200 && /^(?:text\/html|application\/xhtml\+xml)(?:;|$)/i.test(response.headers.get("content-type") || ""));
}

// Bound the whole operation, including a response body that stops arriving.
async function bounded(operation) {
  const controller = new AbortController();
  let timer;
  try {
    return await Promise.race([
      operation(controller.signal),
      new Promise((resolve) => { timer = setTimeout(() => { controller.abort(); resolve(null); }, 2000); }),
    ]);
  } catch { return null; }
  finally { clearTimeout(timer); }
}

async function recoverDeployment(error) {
  if (!build || !(error instanceof AssetFailure)) return;
  try {
    // A pending navigation gets only one chance, even if its response is
    // another stale build. Storage denial disables automatic recovery.
    if (sessionStorage.getItem(recoveryKey) !== null) return;
  } catch { return; }
  let absent = error.absent;
  if (error.asset === "js") {
    absent = await bounded(async (signal) => {
      const response = await fetch(new URL("./{{JS}}", import.meta.url), { cache: "reload", redirect: "error", signal });
      const result = missing(response);
      response.body?.cancel().catch(() => {});
      return result;
    });
  }
  if (!absent) return;
  const next = new URL(location.href);
  next.searchParams.set(recoveryQuery, build);
  const fresh = await bounded(async (signal) => {
    const response = await fetch(next, { cache: "reload", redirect: "error", headers: { "Cache-Control": "no-cache" }, signal });
    if (response.status !== 200 || !/^text\/html(?:;|$)/i.test(response.headers.get("content-type") || "")) {
      response.body?.cancel().catch(() => {});
      return false;
    }
    const reader = response.body?.getReader();
    if (!reader) return false;
    try {
      const chunks = [];
      let size = 0;
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > 256 * 1024) return false;
        chunks.push(value);
      }
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      return changedBuild(new TextDecoder().decode(bytes), next);
    } finally { await reader.cancel().catch(() => {}); }
  });
  if (!fresh) return;
  try {
    sessionStorage.setItem(recoveryKey, build);
    if (sessionStorage.getItem(recoveryKey) !== build) return;
    location.replace(next.href);
  } catch { /* The error panel and its manual Reload remain available. */ }
}

function changedBuild(html, url) {
  const document = new DOMParser().parseFromString(html, "text/html");
  const modules = document.querySelectorAll('link[rel~="preload"][as="fetch"][type="application/wasm"]');
  const scripts = Array.from(document.querySelectorAll('link[rel~="modulepreload"]'))
    .map((link) => new URL(link.getAttribute("href"), url))
    .filter((script) => script.pathname.endsWith("/allthing.js"));
  if (modules.length !== 1 || scripts.length !== 1) return false;
  const module = new URL(modules[0].getAttribute("href"), url);
  const hash = module.pathname.match(/\/allthing-([0-9a-f]{10})\.wasm$/)?.[1];
  const prefix = "{{WASM_URL}}" === "{{BASE}}/assets/{{WASM}}" ? "{{BASE}}/assets/" : "/wasm/";
  return hash && hash !== build && module.origin === location.origin && !module.search && !module.hash &&
    module.pathname === `${prefix}allthing-${hash}.wasm` && scripts[0].origin === location.origin &&
    !scripts[0].search && !scripts[0].hash && scripts[0].pathname === `{{BASE}}/assets/allthing-${hash}/allthing.js`;
}

function cleanRecoveryURL() {
  try {
    const url = new URL(location.href);
    if (url.searchParams.get(recoveryQuery) !== sessionStorage.getItem(recoveryKey)) return;
    if (!url.searchParams.has(recoveryQuery)) return;
    url.searchParams.delete(recoveryQuery);
    history.replaceState(history.state, "", url);
  } catch { /* Keep the original URL when history or storage is unavailable. */ }
}
