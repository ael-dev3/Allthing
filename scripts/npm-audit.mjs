// SPDX-License-Identifier: AGPL-3.0-or-later
// CI's npm audit: fails on a high or critical advisory, as
// `npm audit --audit-level=high` does, but only warns when the registry's
// audit service can't answer (it has returned 400 and 503 for whole days),
// so an outage there doesn't stop every pull request's browser tests.
import { spawnSync } from "node:child_process";

const run = spawnSync("npm", ["audit", "--json", "--audit-level=high"], { encoding: "utf8" });
let report;
try {
  report = JSON.parse(run.stdout);
} catch {
  report = null;
}
if (!report || report.error) {
  const why = report?.error?.summary ?? report?.error ?? run.stderr.trim().split("\n").pop() ?? "no answer";
  console.log(`::warning title=npm audit unavailable::${typeof why === "string" ? why : JSON.stringify(why)}`);
  process.exit(0);
}
const found = report.metadata?.vulnerabilities ?? {};
const serious = (found.high ?? 0) + (found.critical ?? 0);
console.log(`npm audit: ${JSON.stringify(found)}`);
if (serious > 0) {
  spawnSync("npm", ["audit", "--audit-level=high"], { stdio: "inherit" });
  process.exit(1);
}
