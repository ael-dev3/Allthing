// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";

test("the shared Home upsert keeps a newer copy, and a failed statement preserves it", () => {
  const source = readFileSync(new URL("../crates/allthing-worker/src/index.rs", import.meta.url), "utf8");
  const sql = source.match(/const SAVE_HOME_SNAPSHOT: &str = "([^"]+)";/)?.[1].replace(/\\\n\s*/g, "");
  assert(sql);
  const schema = readFileSync(new URL("../migrations/0002_snapshots.sql", import.meta.url), "utf8");
  // The package supports Node22.12, where SQLite still needs this flag.
  const run = spawnSync(process.execPath, ["--experimental-sqlite", "--input-type=module", "-e", `
    import assert from 'node:assert/strict';
    import { DatabaseSync } from 'node:sqlite';
    const db = new DatabaseSync(':memory:');
    db.exec(${JSON.stringify(schema)});
    const save = db.prepare(${JSON.stringify(sql)});
    const row = () => ({...db.prepare('SELECT built,body FROM snapshots WHERE name=?').get('home-day-v1')});
    save.run('home-day-v1',100,'first');
    save.run('home-day-v1',90,'late older build');
    save.run('home-day-v1',100,'equal time');
    assert.deepEqual(row(),{built:100,body:'first'});
    save.run('home-day-v1',110,'newer');
    assert.deepEqual(row(),{built:110,body:'newer'});
    assert.throws(()=>save.run('home-day-v1',120,null));
    assert.deepEqual(row(),{built:110,body:'newer'});
    save.run('latest',200,'other snapshot');
    assert.deepEqual(row(),{built:110,body:'newer'});
    const now = db.prepare("SELECT CAST(strftime('%s','now') AS INTEGER)*1000 + CAST(substr(strftime('%f','now'),4,3) AS INTEGER) AS ms").get().ms;
    save.run('home-day-v1',now-1000,'newer present');
    save.run('home-day-v1',now-2000,'late older present');
    assert.deepEqual(row(),{built:now-1000,body:'newer present'});
    // Seed the invalid existing row directly: a normal save never creates it.
    db.prepare('UPDATE snapshots SET built=?,body=? WHERE name=?').run(now+60000,'impossible future','home-day-v1');
    save.run('home-day-v1',now-500,'repaired');
    assert.deepEqual(row(),{built:now-500,body:'repaired'});
    save.run('home-day-v1',now-1500,'older after repair');
    assert.deepEqual(row(),{built:now-500,body:'repaired'});
    db.close();
  `], { encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr || run.stdout);
});
