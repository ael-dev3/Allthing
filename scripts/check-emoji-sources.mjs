// SPDX-License-Identifier: AGPL-3.0-or-later
// Checks that every Pepe credited in web/emoji/CREDITS.md and
// web/stickers/CREDITS.md is still where its credit says, still shared as
// the credit's terms say, and by whom it says:
//
//   node scripts/check-emoji-sources.mjs
//
// - 7TV: the emote is listed, with no flags, and its owner is the maker.
// - FrankerFaceZ: the emote is public, not hidden, approved, and its owner
//   is the maker.
// - emoji.gg: the emoji's page is there and still under "Do whatever you
//   want with this emoji, have fun!".
//
// It only reads, one source at a time, and exits 1 if any isn't so, naming
// it, so it can be taken out (see the credits' last paragraph).

import { readFileSync } from "node:fs";

const FILES = ["web/emoji/CREDITS.md", "web/stickers/CREDITS.md"];
const FREE = "Do whatever you want with this emoji, have fun!";
const AGENT = "allthing-credits-check (+https://github.com/ael-dev3/Allthing)";

/** Each credit's maker and source, by source: one emote can be both an emoji and a sticker. */
function credits() {
  const found = new Map();
  for (const file of FILES) {
    for (const line of readFileSync(file, "utf8").split("\n")) {
      const cells = line.split("|").map((c) => c.trim());
      const source = cells.find((c) => c.startsWith("https://"));
      if (!line.startsWith("| `") || !source) continue;
      const code = cells[1].replaceAll("`", "").replaceAll(":", "");
      const by = cells[cells.indexOf(source) - 1];
      const seen = found.get(source);
      if (seen && seen.by !== by) throw new Error(`${source} is credited to ${seen.by} and to ${by}`);
      found.set(source, { by, codes: [...(seen?.codes ?? []), code] });
    }
  }
  return found;
}

const same = (a, b) => a.normalize("NFKC").toLowerCase() === b.normalize("NFKC").toLowerCase();

async function get(url, json = true) {
  const res = await fetch(url, { headers: { "user-agent": AGENT, accept: json ? "application/json" : "text/html" } });
  if (!res.ok) return { status: res.status };
  return { status: res.status, body: json ? await res.json() : await res.text() };
}

/** Why `source` isn't as its credit says, or nothing. */
async function check(source, by) {
  const seventv = source.match(/^https:\/\/7tv\.app\/emotes\/([0-9A-Z]+)$/);
  const ffz = source.match(/^https:\/\/www\.frankerfacez\.com\/emoticon\/(\d+)-/);
  if (seventv) {
    const { status, body } = await get(`https://7tv.io/v3/emotes/${seventv[1]}`);
    if (!body) return `7TV answers ${status}`;
    if (!body.listed) return "no longer listed on 7TV";
    if (body.flags) return `flagged on 7TV (${body.flags})`;
    const owner = body.owner ?? {};
    // A maker who has deleted their account: 7TV keeps the emote, with no
    // owner, and its credit says so, as 7TV did.
    const gone = !body.owner && by === "*DeletedUser";
    if (!gone && ![owner.username, owner.display_name].some((name) => name && same(name, by))) return `7TV names ${owner.username ?? "no one"} as its owner`;
  } else if (ffz) {
    const { status, body } = await get(`https://api.frankerfacez.com/v1/emote/${ffz[1]}`);
    const emote = body?.emote;
    if (!emote) return `FrankerFaceZ answers ${status}`;
    if (!emote.public || emote.hidden || emote.status !== 1) return "no longer public and approved on FrankerFaceZ";
    const owner = emote.owner ?? {};
    if (![owner.name, owner.display_name].some((name) => name && same(name, by))) return `FrankerFaceZ names ${owner.name ?? "no one"} as its owner`;
  } else if (source.startsWith("https://emoji.gg/emoji/")) {
    const { status, body } = await get(source, false);
    if (!body) return `emoji.gg answers ${status}`;
    if (!body.includes(FREE)) return "no longer under emoji.gg's free license";
  } else {
    return "a source this doesn't know";
  }
  return null;
}

const all = credits();
const wrong = [];
for (const [source, { by, codes }] of all) {
  const why = await check(source, by).catch((error) => `couldn't be read (${error.message})`);
  if (why) wrong.push(`${codes.join(", ")}: ${why} (${source})`);
}
console.log(`${all.size - wrong.length} of ${all.size} sources are as credited.`);
for (const line of wrong) console.log(`  ${line}`);
process.exit(wrong.length ? 1 : 0);
