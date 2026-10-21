// SPDX-License-Identifier: AGPL-3.0-or-later
// The front page's first screen, drawn as soon as its data has come, before
// the app's WebAssembly has: the cards the app draws first, in its order and
// with its markup (the components in crates/allthing-web, which
// e2e/early-paint.fixture.spec.ts holds this to). It only shows: the boot
// screen holds it, inert and hidden from screen readers, until the app's own
// cards are drawn in the same places and take over (boot.rs). A card
// it can't draw as the app does (one that quotes a cast, or has a video or
// a sticker, or a single image it can't size) ends it there, as does
// anything it doesn't expect. A single image's tile takes its final box
// before the image comes, from Cloudflare Images' description of it (about
// 110 bytes), here and then in the app, so nothing moves when it comes. The
// images themselves wait for the app's WebAssembly: on a slow line they
// would share it, and hold the app back (by 1.4 s in a trial on the speed
// report's phone).

const BASE = "{{BASE}}";
/** The Pepe pack (`allthing_core::emoji::PACK`): code -> [width, height, animated]. */
const PACK = {{EMOJI}};
/** How many cards the app draws first (cast.rs, `DRAW_STEP`). */
const CARDS = 6;
/** The usage bar's apps and the casts it needs to show (components/clients.rs). */
const TOP_APPS = 4;
const MIN_KNOWN = 20;
const FARCASTER_APP = 9152;
const ALLTHING_APP = 3356318;
const MINUTE = 60;
const HOUR = 3600;
const DAY = 86400;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const HEART = "M8 13.5S2.5 10 2.5 6.2A2.8 2.8 0 0 1 8 5a2.8 2.8 0 0 1 5.5 1.2C13.5 10 8 13.5 8 13.5z";
const RECAST = "M4 5.5h7l-1.8-1.8M12 10.5H5l1.8 1.8M4 5.5v3M12 10.5v-3";
const BUBBLE = "M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z";
const STAR = "M8 1.9l1.85 3.8 4.15.6-3 2.95.7 4.15L8 11.45 4.3 13.4l.7-4.15-3-2.95 4.15-.6z";
const PEN = "M10.5 3.2l2.3 2.3M3 13l.6-2.9 7.5-7.5a1.2 1.2 0 0 1 1.7 0l.6.6a1.2 1.2 0 0 1 0 1.7l-7.5 7.5z";
const RANGES = [["all", "All time"], ["year", "Past year"], ["month", "Past month"], ["week", "Past week"], ["day", "Today"], ["hour", "Past hour"]];
const PERIODS = { hour: "the past hour's", day: "today's", week: "the past week's" };
const VIDEO_HOSTS = /^(?:www\.)?(?:m\.)?(?:youtube\.com|music\.youtube\.com|youtube-nocookie\.com|youtu\.be|vimeo\.com|player\.vimeo\.com|loom\.com|streamable\.com)$/;
/** GIF services' media hosts (embeds.rs, `is_gif_host`): Tenor's, KLIPY's and GIPHY's. */
const GIF_HOSTS = /^(?:(?:media\d*|c)\.tenor\.com|static\d*\.klipy\.com|(?:media\d*|i)\.giphy\.com)$/;

const early = window.__allthingEarly;
const boot = document.getElementById("boot");
/** The app's WebAssembly, preloaded by index.html. */
const module = document.querySelector('link[rel="preload"][type="application/wasm"]');
const moduleIn = () => !module || performance.getEntriesByName(module.href).length > 0;
/** What waits for the module: setting the images' sources. */
const waiting = [];
if (!moduleIn()) {
  new PerformanceObserver((_, observer) => {
    if (!moduleIn()) return;
    observer.disconnect();
    for (const start of waiting.splice(0)) start();
  }).observe({ type: "resource", buffered: true });
}
const plan = BASE === "" && early && boot ? front(early.path) : null;
if (plan) {
  // Before the app reads it: the app takes over the same response.
  early.response
    .then((response) => (response.ok && !response.headers.has("x-allthing-stale") ? response.clone().json() : null))
    .then((payload) => payload && paint(payload, plan))
    .catch(() => {});
}

/** What the front page lists for its data's path, if it's one painted here. */
function front(path) {
  const query = new URLSearchParams(location.search);
  if (location.pathname !== "/" || query.has("client")) return null;
  const sort = query.get("sort") === "top" ? "top" : query.get("sort") === "new" ? "new" : "hot";
  const range = RANGES.some(([value]) => value === query.get("t")) ? query.get("t") : "day";
  // "new" is the event stream's; the index's ranges say what they were ranked from.
  if (sort === "new" || !(range in PERIODS)) return null;
  return path.startsWith("/api/v1/home") ? { sort, range } : null;
}

/** Not once the app, or boot.js's panel saying it couldn't load, is there. */
const appCame = () => !boot.isConnected || document.getElementById("app")?.hasChildNodes();

async function paint(payload, { sort, range }) {
  if (appCame() || !Array.isArray(payload.pool)) return;
  const notes = payload.notes || {};
  const lean = leanings();
  const casts = sorted(payload.pool.filter((c) => !c.parent_hash), sort, lean).slice(0, CARDS);
  // Each card is drawn once it and those above it can be, in order: one
  // with a single image once its size has come, a round trip away.
  const ready = casts.map((cast) => (drawable(cast) ? sized(cast) : Promise.reject(new Error("drawn otherwise"))));
  ready.forEach((card) => card.catch(() => {}));
  let list = null;
  for (const [i, cast] of casts.entries()) {
    try {
      await ready[i];
    } catch {
      return;
    }
    if (appCame()) return;
    const card = castCard(cast, notes, i, lean.favorites.has(cast.author.fid));
    if (!card) return;
    if (!list) {
      list = el("div", { class: "cast-list" });
      const content = el("div", { class: "content" }, listingHead(sort, range), shareBar(payload, notes, range), list);
      const main = el("main", { class: "page" }, el("div", { class: "columns single" }, content));
      main.inert = true;
      boot.querySelector(".boot-glow")?.remove();
      boot.append(main);
      boot.setAttribute("data-painted", "");
    }
    list.append(card);
  }
}

// The order the app lists them in (allthing_core::ranking::sorted_for).

const points = (c) => (c.likes || 0) + 2 * (c.recasts || 0) + (c.replies || 0);

function hotScore(p, ts) {
  const score = Math.log10(Math.max(p, 1)) + (ts - 1134028003) / 45000;
  return Math.round(score * 1e7) / 1e7;
}

const newestFirst = (a, b) => b.ts - a.ts || (a.hash < b.hash ? -1 : a.hash > b.hash ? 1 : 0);

/** A favorite's casts weigh 25 times their points, and those of an account the reader sees less of a 25th (`FAVORITE_WEIGHT`, `SHOW_LESS_WEIGHT`). */
const FAVORITE_WEIGHT = 25;
const SHOW_LESS_WEIGHT = 1 / FAVORITE_WEIGHT;

/** How many days the casts span, from the oldest to the newest, and one at least (`span_days`). */
function spanDays(casts) {
  if (!casts.length) return 1;
  const times = casts.map((c) => c.ts);
  return Math.max(1, (Math.max(...times) - Math.min(...times)) / DAY);
}

function sorted(casts, sort, lean) {
  const weight = (c) => (lean.favorites.has(c.author.fid) ? FAVORITE_WEIGHT : lean.less.has(c.author.fid) ? SHOW_LESS_WEIGHT : 1);
  const days = spanDays(casts);
  // In top, the points plus one times the weight; in hot, the weight's log added once for each day the casts span. On a tie, the heavier first.
  const key = sort === "top" ? (c) => (points(c) + 1) * weight(c) : (c) => hotScore(points(c), c.ts) + (weight(c) === 1 ? 0 : Math.log10(weight(c)) * days);
  return [...casts].sort((a, b) => key(b) - key(a) || weight(b) - weight(a) || newestFirst(a, b));
}

/** The reader's favorites and those they see less of (leanings.rs). */
function leanings() {
  const fids = (key) => {
    try {
      return new Set((JSON.parse(localStorage.getItem(key) || "[]") || []).map((a) => a.fid));
    } catch {
      return new Set();
    }
  };
  return { favorites: fids("allthing.favorites"), less: fids("allthing.show-less") };
}

// Markup.

function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs || {})) {
    if (value !== null && value !== undefined && value !== false) node.setAttribute(name, value === true ? "" : String(value));
  }
  for (const child of children.flat(Infinity)) {
    if (child !== null && child !== undefined && child !== false) node.append(child);
  }
  return node;
}

/** An image, given its source once the app's module is in. Until then it
 * shows a transparent stand-in of its size (`width` × `height`, or one the
 * page sizes), not its alt text, so nothing moves when it comes. */
function image(attrs, src, [width, height] = [1, 1]) {
  const img = el("img", { ...attrs, src: `data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='${width}' height='${height}'/%3E` });
  later(() => img.setAttribute("src", src));
  return img;
}

function later(start) {
  if (moduleIn()) start();
  else waiting.push(start);
}

function icon(path) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("aria-hidden", "true");
  const p = document.createElementNS(ns, "path");
  p.setAttribute("d", path);
  svg.append(p);
  return svg;
}

const href = (path) => BASE + path;
const userHref = (name) => href("/u/" + encodeURIComponent(name));
const castHref = (hash) => href("/p/" + hash);
const channelHref = (id) => (/^[a-z0-9][a-z0-9-]{0,63}$/.test(id) ? href("/c/" + id) : href("/c?id=" + encodeURIComponent(id)));

function listingHead(sort, range) {
  // The range kept unless it's today's (home.rs, `SortTabs`' path).
  // What each does is read out, as the app's tabs say it: no native tooltip.
  const tab = (name, description) => {
    const query = [range === "day" ? null : "t=" + range, name === "hot" ? null : "sort=" + name].filter(Boolean).join("&");
    const to = href(query ? "/?" + query : "/");
    return el("a", { href: to, "data-noscroll": true, class: sort === name ? "selected" : null, "aria-current": sort === name ? "page" : null, "aria-description": description }, name);
  };
  const select = el("select", {}, RANGES.map(([value, label]) => el("option", { value, selected: value === range }, label)));
  return el(
    "div",
    { class: "listing-head" },
    el("h1", { class: "visually-hidden" }, "Trending on Farcaster"),
    el(
      "div",
      { class: "sort-row" },
      el("nav", { class: "sort-tabs", "aria-label": "Sort" }, tab("hot", "engagement, decaying with age"), tab("new", "newest first"), tab("top", "most engagement")),
      el("label", { class: "time-filter" }, el("span", { class: "visually-hidden" }, "Time range"), select),
    ),
    el("button", { class: "cast-pill", type: "button", "aria-label": "Write a cast" }, icon(PEN), "Cast"),
  );
}

/** What to call an app, and its profile's key (allthing_core::clients). */
const clientName = (app) => (app.fid === FARCASTER_APP ? "Farcaster" : app.fid === ALLTHING_APP ? "Allthing" : app.display_name || app.username);
const clientProfile = (app) => (app.fid === FARCASTER_APP ? "fid:" + app.fid : app.username);

/** Whole percents that sum to 100, by the largest remainder. */
function percents(counts) {
  const total = counts.reduce((a, b) => a + b, 0);
  if (total === 0) return counts.map(() => 0);
  const out = counts.map((n) => Math.floor((n * 100) / total));
  const order = counts.map((_, i) => i).sort((a, b) => ((counts[b] * 100) % total) - ((counts[a] * 100) % total) || a - b);
  const short = 100 - out.reduce((a, b) => a + b, 0);
  for (const i of order.slice(0, short)) out[i] += 1;
  return out;
}

/** The apps the casts were posted from (ClientShareBar). */
function shareBar(payload, notes, range) {
  const origins = notes.origins || {};
  const apps = notes.apps || {};
  const counts = new Map();
  for (const cast of payload.pool) {
    const fid = origins[cast.hash]?.app_fid;
    if (fid !== undefined && fid !== null) counts.set(fid, (counts.get(fid) || 0) + 1);
  }
  const known = [...counts.values()].reduce((a, b) => a + b, 0);
  if (known === 0 || known < MIN_KNOWN) return null;
  const ranked = [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0] - b[0]);
  const shares = ranked.slice(0, TOP_APPS).map(([fid, casts]) => ({ fid, casts, name: apps[fid] ? clientName(apps[fid]) : "app fid " + fid }));
  const rest = ranked.slice(TOP_APPS).reduce((a, [, n]) => a + n, 0);
  if (rest > 0) shares.push({ fid: null, casts: rest, name: "Other apps" });
  percents(shares.map((s) => s.casts)).forEach((p, i) => (shares[i].percent = p));
  const counted = Boolean(payload.apps_24h);
  const period = PERIODS[range];
  const pct = (p) => (p === 0 ? "<1%" : p + "%");
  // The feed with the reader's sort and range kept, and the app filter set (clients.rs, `client_href`).
  const query = new URLSearchParams(location.search);
  const target = (fid) => {
    const pairs = [["sort", query.get("sort")], ["t", query.get("t")], ["client", String(fid)]].filter(([, value]) => value);
    return href("/?" + pairs.map(([key, value]) => key + "=" + encodeURIComponent(value)).join("&"));
  };
  const segments = shares
    .map((s, i) => [s, i])
    .filter(([s]) => s.percent > 0)
    .map(([s, i]) => {
      const grow = "flex-grow: " + s.percent;
      if (s.fid !== null && counted) {
        const seg = el("a", { class: "seg client-" + i, href: target(s.fid), tabindex: "-1", "aria-hidden": "true" });
        return el("span", { style: grow, class: "seg-slot" }, el("span", { class: "popover seg-pop" }, seg));
      }
      return el("span", { style: grow, class: "seg client-" + i });
    });
  const legend = shares.map((s, i) => {
    const swatch = el("span", { class: "swatch client-" + i, "aria-hidden": "true" });
    const share = [swatch, s.name + " ", el("strong", { class: "pct" }, pct(s.percent))];
    if (s.fid === null) {
      return el("li", {}, el("span", { class: "popover " }, el("button", { class: "popover-trigger client-other", type: "button", "aria-label": "Other apps", "aria-expanded": "false" }, share)));
    }
    const link = el("a", { href: target(s.fid) }, share);
    return el("li", {}, counted ? el("span", { class: "popover " }, link) : link);
  });
  return el(
    "section",
    {
      class: "client-share",
      "aria-label": `Apps posting ${period} trending casts`,
      "aria-description": `Apps that posted ${known} of ${period} trending casts, from the app that registered each cast's signing key.`,
    },
    el("div", { class: counted ? "share-bar counted" : "share-bar", role: "img", "aria-label": shares.map((s) => `${s.name} ${s.percent}%`).join(", ") }, segments),
    el("ul", { class: "client-legend" }, legend),
  );
}

// A card (cast.rs, CastCard), or nothing for one the app draws otherwise.

function castCard(cast, notes, i, favorite) {
  if (!cast || !cast.hash || !cast.author || typeof cast.ts !== "number" || !drawable(cast)) return null;
  const link = castHref(cast.hash);
  const text = typeof cast.text === "string" ? cast.text : "";
  const images = (cast.embeds || []).filter((e) => e.kind === "image");
  const card = el(
    "article",
    {
      style: `--i: ${Math.min(i, 16)}`,
      class: "cast-card",
      "data-hash": cast.hash,
      "data-fid": cast.author.fid,
      "data-ts": cast.ts,
      "data-points": points(cast),
      "data-likes": cast.likes || 0,
      "data-recasts": cast.recasts || 0,
      "data-replies": cast.replies || 0,
    },
    el("p", { class: "deleted-note" }, "You deleted this cast."),
    meta(cast, notes, link, favorite),
    text.trim() ? el("div", { class: "cast-text" }, castText(text, cast.mentions || [])) : null,
    images.length ? mediaGrid(cast, images, link) : null,
    linkCards(cast, text),
    el("footer", { class: "cast-foot" }, engagement(cast)),
    // Its Pepe reactions' row, as the app draws it until their chips come
    // (reactions.rs, `card_reactions`), as tall as they are.
    (notes.reactions || {})[cast.hash]?.length ? el("div", { class: "reaction-chips coming", "aria-hidden": "true" }) : null,
  );
  return card;
}

/** Whether the card is one drawn here as the app draws it. */
function drawable(cast) {
  for (const embed of cast.embeds || []) {
    if (embed.kind === "image") {
      if (typeof embed.url !== "string" || embed.url.startsWith("https://allthing.social/assets/stickers/")) return false;
    } else if (embed.kind === "link") {
      if (typeof embed.url !== "string" || isVideoLink(embed.url)) return false;
    } else {
      return false;
    }
  }
  return !tokenize(cast.text || "", []).some((t) => t.url && isVideoLink(t.url));
}

/** A URL's host, lowercase, without its port (embeds.rs, `host_of`). */
function hostOf(url) {
  const m = /^https?:\/\/([^/?#]*)/i.exec(url);
  if (!m) return null;
  let host = m[1].split("@").pop();
  const colon = host.lastIndexOf(":");
  if (colon >= 0 && /^\d*$/.test(host.slice(colon + 1))) host = host.slice(0, colon);
  return /^[A-Za-z0-9._-]+$/.test(host) ? host.toLowerCase() : null;
}

/** Whether a link plays as a video facade (allthing_core::video, `detect`): any on those hosts. */
const isVideoLink = (url) => VIDEO_HOSTS.test(hostOf(url) || "");

function meta(cast, notes, link, favorite) {
  const user = cast.author;
  const to = userHref(user.username);
  const avatar = user.pfp_url
    ? image({ class: "avatar", alt: "", width: 22, height: 22, loading: "lazy", decoding: "async", referrerpolicy: "no-referrer" }, resized(user.pfp_url, "anim=false,fit=cover,f=auto,w=64,h=64"), [22, 22])
    : el("span", { class: "avatar avatar-empty", style: "width: 22px; height: 22px;" });
  const named = user.display_name && user.display_name !== user.username;
  const channel = cast.channel ? el("span", { class: "in-channel" }, "in ", el("a", { class: "channel-name", href: channelHref(cast.channel.id) }, "/" + cast.channel.label)) : null;
  return el(
    "header",
    { class: "cast-meta" },
    // Who and when, on one line (components/cast.rs); the dot is the time's.
    el(
      "span",
      { class: "byline" },
      el(
        "span",
        { class: "popover user-chip" },
        el("a", { class: "user-link", href: to, tabindex: "-1", "aria-hidden": "true" }, avatar),
        named ? el("a", { class: "display-name", href: to }, user.display_name) : null,
        el("a", { class: "author", href: to }, "@" + user.username),
      ),
      favorite ? el("span", { class: "favorite-mark", role: "img", "aria-label": "One of your favorites" }, icon(STAR)) : null,
      el("a", { class: "when", href: link }, el("span", { class: "visually-hidden" }, "Open post, "), el("time", { datetime: isoUtc(cast.ts), "aria-description": dateTime(cast.ts) }, timeAgo(cast.ts))),
    ),
    channel,
    origin(cast.hash, notes),
  );
}

/** "0xc052…23fa" (allthing_core::format, `short_address`). */
function shortAddress(address) {
  const chars = Array.from(address);
  const head = address.startsWith("0x") ? 6 : 4;
  return chars.length <= head + 6 ? address : chars.slice(0, head).join("") + "…" + chars.slice(-4).join("");
}

/** "via Farcaster" (chips.rs, `OriginChip`). */
function origin(hash, notes) {
  const found = (notes.origins || {})[hash];
  if (!found) return null;
  const fid = found.app_fid;
  const attributed = fid !== undefined && fid !== null;
  const app = attributed ? (notes.apps || {})[fid] : undefined;
  const key = shortAddress(String(found.signer || ""));
  if (app) {
    const name = clientName(app);
    const told = `Signed with key ${key} requested by ${name} (app fid ${app.fid}).`;
    return el("span", { class: "origin", "aria-description": told }, "via ", el("a", { href: userHref(clientProfile(app)) }, name));
  }
  const told = attributed ? `Signed with key ${key}, requested by fid ${fid}.` : `Signed with key ${key}, registered without an app attribution.`;
  return el("span", { class: "origin origin-unknown", "aria-description": told }, attributed ? "via app fid " + fid : "via unattributed key");
}

// Text (allthing_core::text and components/text.rs).

function shortcodeAt(rest) {
  const m = /^:([A-Za-z0-9_]+):/.exec(rest);
  if (!m) return null;
  const code = m[1].toLowerCase();
  return Object.hasOwn(PACK, code) ? { emoji: code, length: m[0].length } : null;
}

function trimUrl(raw) {
  let url = raw;
  for (;;) {
    const c = url.slice(-1);
    if (c && ".,!?;:'\"".includes(c)) url = url.slice(0, -1);
    else if (c === ")") {
      if (url.split("(").length >= url.split(")").length) return url;
      url = url.slice(0, -1);
    } else return url;
  }
}

/** The link `rest` starts with, if any, as `tokenize` reads one. */
function urlAt(rest) {
  const scheme = /^https:\/\//i.test(rest) ? 8 : /^http:\/\//i.test(rest) ? 7 : 0;
  if (!scheme) return null;
  const end = rest.search(/[\s<>"']/u);
  const url = trimUrl(end < 0 ? rest : rest.slice(0, end));
  return url.length > scheme ? url : null;
}

/** Plain text, links, mentions the cast carries, /channels and pack emoji. */
function tokenize(text, mentions) {
  const known = mentions.map((m) => String(m.username || "").toLowerCase());
  const tokens = [];
  const pushText = (value) => {
    if (!value) return;
    const last = tokens[tokens.length - 1];
    if (last && last.text !== undefined) last.text += value;
    else tokens.push({ text: value });
  };
  let last = 0;
  let i = 0;
  while (i < text.length) {
    const rest = text.slice(i);
    const ch = String.fromCodePoint(text.codePointAt(i));
    const before = text.slice(0, i);
    const prev = before ? Array.from(before).pop() : undefined;
    const url = urlAt(rest);
    if (url) {
      pushText(text.slice(last, i));
      tokens.push({ url });
      i += url.length;
      last = i;
      continue;
    } else if (ch === "@") {
      const after = rest.slice(1);
      const stop = after.search(/[^A-Za-z0-9_.-]/);
      const name = (stop < 0 ? after : after.slice(0, stop)).replace(/[^A-Za-z0-9]+$/, "");
      if (/^[A-Za-z0-9]/.test(name)) {
        pushText(text.slice(last, i));
        if (known.includes(name.toLowerCase())) tokens.push({ mention: name });
        else pushText("@" + name);
        i += 1 + name.length;
        last = i;
        continue;
      }
    } else if (ch === ":") {
      const found = shortcodeAt(rest);
      if (found) {
        pushText(text.slice(last, i));
        tokens.push({ emoji: found.emoji });
        i += found.length;
        last = i;
        continue;
      }
    } else if (ch === "/" && (prev === undefined || /\s/u.test(prev) || prev === "(")) {
      const after = rest.slice(1);
      const stop = after.search(/[^A-Za-z0-9-]/);
      const runEnd = (stop < 0 ? after.length : stop) + 1;
      const name = rest.slice(1, runEnd);
      const next = rest.slice(runEnd, runEnd + 1);
      const blocked = /[A-Za-z0-9_/-]/.test(next);
      if (name && name.length <= 32 && /^[A-Za-z0-9]/.test(name) && !blocked) {
        pushText(text.slice(last, i));
        tokens.push({ channel: name.toLowerCase() });
        i += runEnd;
        last = i;
        continue;
      }
    }
    i += ch.length;
  }
  pushText(text.slice(last));
  return tokens;
}

/** Paragraphs on blank lines; single newlines stay inside. */
function paragraphs(text) {
  const out = [];
  let current = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) {
      if (current.length) out.push(current.join("\n"));
      current = [];
    } else current.push(line);
  }
  if (current.length) out.push(current.join("\n"));
  return out;
}

function displayUrl(url) {
  const at = url.indexOf("://");
  let bare = at < 0 ? url : url.slice(at + 3);
  if (bare.startsWith("www.")) bare = bare.slice(4);
  bare = bare.replace(/\/+$/, "");
  const chars = Array.from(bare);
  return chars.length > 48 ? chars.slice(0, 47).join("") + "…" : bare;
}

function emojiView(code) {
  const [width, height, animated] = PACK[code];
  const still = animated ? el("source", { media: "(prefers-reduced-motion: reduce)" }) : null;
  if (still) later(() => still.setAttribute("srcset", href(`/assets/emoji/v1/${code}.still.webp`)));
  return el("picture", { class: "emoji" }, still, image({ alt: `:${code}:`, width, height, loading: "lazy", decoding: "async" }, href(`/assets/emoji/v1/${code}.webp`), [width, height]));
}

function line(text, mentions) {
  return tokenize(text, mentions).map((t) => {
    if (t.url) return el("a", { class: "ext", href: t.url, target: "_blank", rel: "noopener noreferrer nofollow ugc" }, displayUrl(t.url));
    if (t.mention) return el("a", { class: "mention", href: userHref(t.mention) }, "@" + t.mention);
    if (t.channel) return el("a", { class: "channel-ref", href: channelHref(t.channel) }, "/" + t.channel);
    if (t.emoji) return emojiView(t.emoji);
    return t.text;
  });
}

function castText(text, mentions) {
  const tokens = tokenize(text, mentions);
  const emoji = tokens.filter((t) => t.emoji).length;
  const jumbo = emoji >= 1 && emoji <= 6 && tokens.every((t) => t.emoji || (t.text !== undefined && !t.text.trim()));
  return paragraphs(text).map((paragraph) =>
    el(
      "p",
      { class: jumbo ? "jumbo" : null },
      paragraph.split("\n").map((l, n) => [n > 0 ? el("br") : null, line(l, mentions)]),
    ),
  );
}

/** The characters a pack emoji counts as in an excerpt (`EMOJI_CHARS`). */
const EMOJI_CHARS = 3;

/** Where `text` has pack shortcodes, as [start, end]: those `tokenize` reads, so none in a link. */
function emojiSpans(text) {
  const spans = [];
  let i = 0;
  while (i < text.length) {
    const rest = text.slice(i);
    const url = urlAt(rest);
    const found = url ? null : shortcodeAt(rest);
    if (url) i += url.length;
    else if (found) {
      spans.push([i, i + found.length]);
      i += found.length;
    } else i += String.fromCodePoint(text.codePointAt(i)).length;
  }
  return spans;
}

/**
 * The first `max` characters of a cast, whitespace collapsed, cut at a word
 * (`truncate_title`). A pack shortcode counts as its image's room and is
 * never cut.
 */
function truncateTitle(text, max) {
  const flat = text.split(/\s+/u).filter(Boolean).join(" ");
  const spans = emojiSpans(flat);
  // Each character or whole shortcode, as where it ends and its room.
  const units = [];
  for (let i = 0, s = 0; i < flat.length; ) {
    if (spans[s]?.[0] === i) {
      i = spans[s++][1];
      units.push([i, EMOJI_CHARS]);
    } else {
      i += String.fromCodePoint(flat.codePointAt(i)).length;
      units.push([i, 1]);
    }
  }
  let room = 0;
  let fit = 0;
  while (fit < units.length && (room += units[fit][1]) <= max) fit++;
  if (fit === units.length) return flat;
  const end = fit ? units[fit - 1][0] : 0;
  const cut = flat.slice(0, end);
  // A cut before a space, or beside an emoji, already falls between words.
  const between = flat.startsWith(" ", end) || spans.some(([start, stop]) => start === end || stop === end);
  const space = cut.lastIndexOf(" ");
  const before = units.filter(([e]) => e <= space).reduce((sum, [, width]) => sum + width, 0);
  const base = space >= 0 && !between && before * 10 > max * 6 ? cut.slice(0, space) : cut;
  const trimmed = base.replace(/[\s.,;:!?…-]+$/u, "");
  // A shortcode at the end keeps its closing colon.
  const shortcode = spans.map(([, stop]) => stop).filter((stop) => stop <= base.length).pop() ?? 0;
  return base.slice(0, Math.max(trimmed.length, shortcode)) + "…";
}

// Media (components/media.rs).

/** An imagedelivery.net image at the size a place shows it (embeds.rs). */
function resized(url, variant) {
  const prefix = "https://imagedelivery.net/";
  if (!url.startsWith(prefix)) return url;
  const parts = url.slice(prefix.length).split("/");
  const plain = (s) => s && !/[?#]/.test(s);
  return parts.length === 3 && parts.every(plain) ? `${prefix}${parts[0]}/${parts[1]}/${variant}` : url;
}

/** Whether an image is a GIF (embeds.rs, `is_gif`): from a GIF service, or its path ends in .gif. */
function isGif(url) {
  const host = hostOf(url);
  if (!host) return false;
  const path = url.slice(url.indexOf("://") + 3).replace(/^[^/?#]*/, "").split(/[?#]/)[0];
  return GIF_HOSTS.test(host) || path.toLowerCase().endsWith(".gif");
}

/** Image sizes for the app (media.rs, `known_size`). */
const sizes = (window.__allthingSizes = window.__allthingSizes || {});
/** The variant a feed shows an image at (embeds.rs, `ImageSize::Inline`). */
const INLINE = "fit=scale-down,f=auto,w=720";

/** Once a card's single image, if it has one, has its size. */
function sized(cast) {
  const images = (cast.embeds || []).filter((e) => e.kind === "image");
  if (images.length !== 1) return Promise.resolve();
  const src = resized(images[0].url, INLINE);
  if (src === images[0].url) return Promise.reject(new Error("not on Cloudflare Images"));
  // The same options, described rather than drawn.
  return fetch(src.slice(0, src.lastIndexOf("/") + 1) + "fit=scale-down,w=720,format=json")
    .then((response) => (response.ok ? response.json() : null))
    .then((found) => {
      if (!(found?.width > 0 && found?.height > 0)) throw new Error("no size");
      sizes[src] = [found.width, found.height];
    });
}

function mediaGrid(cast, images, link) {
  const excerpt = truncateTitle(cast.text || "", 90);
  const tiles = images.map((embed, i) => {
    const position = images.length > 1 ? ` ${i + 1} of ${images.length}` : "";
    const alt = `Image${position} posted by @${cast.author.username}` + (excerpt ? ": " + excerpt : "");
    const src = resized(embed.url, INLINE);
    // As the app's tile with a size it knows (media.rs, `MediaGrid`).
    const [width, height] = sizes[src] || [];
    const img = image({ width, height, alt, loading: "lazy", decoding: "async", referrerpolicy: "no-referrer" }, src, width ? [width, height] : undefined);
    const box = width ? { style: `--w: ${width}; --h: ${height}`, "data-sized": true, "data-portrait": height > width } : {};
    const tile = el("a", { class: isGif(embed.url) ? "tile tile-image gif" : "tile tile-image", href: link, ...box }, img);
    img.addEventListener("load", () => {
      const [w, h] = [img.naturalWidth, img.naturalHeight];
      if (!w || !h || img.currentSrc.startsWith("data:")) return;
      sizes[src] = [w, h];
      img.setAttribute("width", w);
      img.setAttribute("height", h);
      tile.setAttribute("style", `--w: ${w}; --h: ${h}`);
      tile.setAttribute("data-sized", "");
      if (h > w) tile.setAttribute("data-portrait", "");
    });
    return tile;
  });
  return el("div", { class: "media-grid n" + Math.min(images.length, 4) }, tiles);
}

function linkCards(cast, text) {
  return (cast.embeds || [])
    .filter((e) => e.kind === "link" && !text.includes(e.url))
    .map((e) => {
      const host = hostOf(e.url);
      const domain = host ? host.replace(/^www\./, "") : e.url;
      return el(
        "a",
        { class: "link-card", href: e.url, target: "_blank", rel: "noopener noreferrer nofollow ugc" },
        e.image_url ? image({ alt: "", loading: "lazy", referrerpolicy: "no-referrer" }, resized(e.image_url, "anim=false,fit=cover,f=auto,w=140,h=140")) : null,
        el("span", { class: "link-text" }, el("span", { class: "link-title" }, e.title || e.url), el("span", { class: "link-domain" }, domain)),
      );
    });
}

// Engagement (components/chips.rs).

function fullNumber(n) {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** As Rust formats it: exact halves to even, where JS rounds them up. */
function compactNumber(n) {
  const trim = (s) => (s.endsWith(".0") ? s.slice(0, -2) : s);
  if (n < 10000) return fullNumber(n);
  if (n < 100000) return trim(n % 1000 === 250 ? (Math.floor(n / 100) / 10).toFixed(1) : (n / 1000).toFixed(1)) + "k";
  if (n < 1000000) {
    const whole = Math.floor(n / 1000);
    return (n % 1000 === 500 ? (whole % 2 ? whole + 1 : whole) : Math.round(n / 1000)) + "k";
  }
  return trim(n % 1000000 === 250000 ? (Math.floor(n / 100000) / 10).toFixed(1) : (n / 1000000).toFixed(1)) + "m";
}

function count(n, one, many) {
  return [el("span", { class: "n" }, compactNumber(n)), el("span", { class: "label" }, n === 1 ? one : many)];
}

function reaction(kind, n) {
  const [path, one, many, label] = kind === "likes" ? [HEART, " like", " likes", "Like"] : [RECAST, " recast", " recasts", "Recast"];
  const act = el("button", { class: "act", type: "button", "aria-label": label, "aria-haspopup": "dialog" }, icon(path));
  const shown =
    n > 0
      ? el("button", { class: "popover-trigger stat-count", type: "button", "aria-label": `${compactNumber(n)}${n === 1 ? one : many}: show who`, "aria-expanded": "false" }, count(n, one, many))
      : el("span", { class: "popover-idle stat-count" }, count(n, one, many));
  return el("span", { class: `stat stat-${kind} acting` }, el("span", { class: "popover " }, act, shown));
}

function engagement(cast) {
  const [likes, recasts, replies] = [cast.likes || 0, cast.recasts || 0, cast.replies || 0];
  const repliesView =
    replies === 0
      ? el("span", { class: "stat stat-replies" }, icon(BUBBLE), count(0, " reply", " replies"))
      : el(
          "span",
          { class: "popover " },
          el(
            "button",
            { class: "popover-trigger stat stat-replies", type: "button", "aria-label": `${compactNumber(replies)} ${replies === 1 ? "reply" : "replies"}: preview`, "aria-expanded": "false" },
            el("span", { class: "stat-body" }, icon(BUBBLE), count(replies, " reply", " replies")),
          ),
        );
  const summary = `${fullNumber(likes)} likes, ${fullNumber(recasts)} recasts, ${fullNumber(replies)} replies. Ranked by engagement points: likes + 2 × recasts + replies = ${fullNumber(points(cast))}.`;
  return el("div", { class: "engagement" }, reaction("likes", likes), reaction("recasts", recasts), repliesView, el("span", { class: "visually-hidden" }, summary));
}

// Time (allthing_core::time).

function timeAgo(ts) {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - ts);
  if (diff < MINUTE) return "just now";
  if (diff < HOUR) return Math.floor(diff / MINUTE) + "m ago";
  if (diff < DAY) return Math.floor(diff / HOUR) + "h ago";
  if (diff < 30 * DAY) return Math.floor(diff / DAY) + "d ago";
  if (diff < 365 * DAY) return Math.floor(diff / (30 * DAY)) + "mo ago";
  return Math.floor(diff / (365 * DAY)) + "y ago";
}

const isoUtc = (ts) => new Date(ts * 1000).toISOString().replace(/\.\d{3}Z$/, "Z");

function dateTime(ts) {
  const d = new Date(ts * 1000);
  const pad = (n) => String(n).padStart(2, "0");
  return `${MONTHS[d.getUTCMonth()]} ${d.getUTCDate()}, ${d.getUTCFullYear()}, ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())} UTC`;
}
