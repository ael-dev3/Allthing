// SPDX-License-Identifier: AGPL-3.0-or-later
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import test from "node:test";

const source = readFileSync(new URL("../crates/allthing-worker/src/index.rs", import.meta.url), "utf8");
const names = [
  "ACTIVITY_STATE", "CLAIM_ACTIVITY", "ACTIVITY_SETS", "STORE_SETS", "DROP_SETS",
  "STORE_KEYS", "STORE_ACTIVITY_KEYS", "KEY_APPS", "COUNT_APPS", "APPS_OF",
  "SAVE_ACTIVITY_STATE", "SAVE_ACTIVITY_SNAPSHOT", "STORE_REACTIONS", "DROP_REACTIONS", "OLD_REACTIONS",
];
const sql = Object.fromEntries(names.map(name => {
  const statement = source.match(new RegExp(`const ${name}: &str = "([^"]+)";`))?.[1].replace(/\\\n\s*/g, "");
  assert(statement, `missing Worker SQL constant ${name}`);
  return [name, statement];
}));
const schema = [
  "0001_index.sql", "0002_snapshots.sql", "0003_activity.sql", "0004_signer_apps.sql",
  "0006_reactions.sql", "0011_verified_activity.sql",
].map(name => readFileSync(new URL(`../migrations/${name}`, import.meta.url), "utf8")).join("\n");

function scenario(body) {
  // Match the supported Node22.12 setup, where SQLite needs the explicit flag.
  const run = spawnSync(process.execPath, ["--experimental-sqlite", "--input-type=module", "-e", `
    import assert from 'node:assert/strict';
    import { DatabaseSync } from 'node:sqlite';
    const db = new DatabaseSync(':memory:');
    db.exec(${JSON.stringify(schema)});
    const sql = ${JSON.stringify(sql)};
    const json = JSON.stringify;
    const rows = (table, order) => db.prepare('SELECT * FROM '+table+' ORDER BY '+order).all().map(row => ({...row}));
    const dump = () => ({
      state: rows('state','key'), snapshots: rows('snapshots','name'), activity: rows('activity','name'),
      keys: rows('verified_signer_apps','identity'), reactions: rows('reactions','hash'), legacyKeys: rows('signer_apps','pair'),
    });
    const identity = (fid, suffix) => String(fid).padStart(10,'0')+':0x12345678'+suffix.toString(16).padStart(56,'0');
    const seed = () => {
      db.prepare('INSERT INTO state VALUES (?,?)').run('activity_v2','before');
      db.prepare('INSERT INTO state VALUES (?,?)').run('activity','legacy-state');
      db.prepare('INSERT INTO snapshots VALUES (?,?,?)').run('activity_v2',10,'before-snapshot');
      db.prepare('INSERT INTO snapshots VALUES (?,?,?)').run('activity',5,'legacy-snapshot');
      for (const name of ['h1','k1','d1','w1','v2h1','v2k1','v2d1','v2w1','v2h20','v2d20']) {
        db.prepare('INSERT INTO activity VALUES (?,?)').run(name,'original-'+name);
      }
      db.prepare('INSERT INTO signer_apps VALUES (?,?)').run(123,9152);
      db.prepare('INSERT INTO reactions VALUES (?,?,?,?,?)').run('remove-me','parent','pepe',42,200);
      db.prepare('INSERT INTO reactions VALUES (?,?,?,?,?)').run('expire-me','parent','pepe',42,50);
      db.prepare('INSERT INTO reactions VALUES (?,?,?,?,?)').run('keep-me','parent','pepe',42,500);
    };
    // D1 batch is transactional. Execute every statement after a failed claim
    // too: each write and delete must enforce its own winning token.
    const save = ({expected, token, state='after', sets=[['v2h20','winner-hour'],['v2d20','winner-day']],
      keys=[[identity(4294967295,1),9152]], reactions=[['parent','winner-reaction','pepe',4294967295,300]],
      removed=['remove-me'], keepHour=10, keepDay=10, reactionsFrom=100, built=20, body='winner-snapshot', fail=false}) => {
      db.exec('BEGIN');
      try {
        const claim = db.prepare(sql.CLAIM_ACTIVITY).get(expected,token);
        db.prepare(sql.STORE_SETS).run(json(sets),token);
        db.prepare(sql.STORE_ACTIVITY_KEYS).run(json(keys),token);
        db.prepare(sql.DROP_SETS).run(keepHour,keepDay,token);
        db.prepare(sql.STORE_REACTIONS).run(json(reactions),token);
        db.prepare(sql.DROP_REACTIONS).run(json(removed),token);
        db.prepare(sql.OLD_REACTIONS).run(reactionsFrom,token);
        db.prepare(sql.SAVE_ACTIVITY_STATE).run(state,token);
        db.prepare(sql.SAVE_ACTIVITY_SNAPSHOT).run('activity_v2',built,body,token);
        if (fail) db.prepare(sql.SAVE_ACTIVITY_SNAPSHOT).run('activity_v2',built+1,null,token);
        db.exec('COMMIT');
        return claim?.value === token;
      } catch (error) {
        db.exec('ROLLBACK');
        throw error;
      }
    };
    ${body}
    db.close();
  `], { encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr || run.stdout);
}

test("an activity winner writes its batch and stale tokens cannot write or delete anything", () => scenario(`
  seed();
  assert.equal(save({expected:'before',token:'winner-token'}),true);
  assert.equal(db.prepare(sql.ACTIVITY_STATE).get().value,'after');
  assert.deepEqual(rows('snapshots','name'),[
    {name:'activity',built:5,body:'legacy-snapshot'},
    {name:'activity_v2',built:20,body:'winner-snapshot'},
  ]);
  assert.deepEqual(db.prepare(sql.ACTIVITY_SETS).all(json(['v2h20','v2d20'])).map(r=>({...r})).sort((a,b)=>a.name.localeCompare(b.name)),[
    {name:'v2d20',body:'winner-day'},{name:'v2h20',body:'winner-hour'},
  ]);
  assert.deepEqual(rows('verified_signer_apps','identity'),[{identity:identity(4294967295,1),fid:4294967295,app:9152}]);
  assert.deepEqual(rows('reactions','hash').map(r=>r.hash),['keep-me','winner-reaction']);
  assert(!rows('activity','name').some(r=>['v2h1','v2k1','v2d1','v2w1'].includes(r.name)));
  const won=dump();
  for (const [token,candidate] of [['stale-different','stale-state'],['stale-identical','after']]) {
    assert.equal(save({expected:'before',token,state:candidate,
      sets:[['v2h20','corrupt-existing'],['v2h21','stale-only']],
      keys:[[identity(4294967295,2),309857]],reactions:[['parent','stale-only','pepe',7,400]],
      removed:['keep-me','winner-reaction'],keepHour:999,keepDay:999,reactionsFrom:999,
      built:999,body:'stale-snapshot'}),false);
    assert.deepEqual(dump(),won,'a stale batch changed persisted data');
  }
`));

test("the first activity batch claims a null state and rejects another null-state reader", () => scenario(`
  assert.equal(db.prepare(sql.ACTIVITY_STATE).get(),undefined);
  assert.equal(save({expected:null,token:'first-token'}),true);
  const first=dump();
  assert.equal(save({expected:null,token:'other-token',state:'after',sets:[['v2h20','loser']],
    removed:['winner-reaction'],reactions:[],keepHour:999,keepDay:999,reactionsFrom:999}),false);
  assert.deepEqual(dump(),first);
`));

test("a late failing activity statement rolls back its claim and every earlier mutation", () => scenario(`
  seed();
  const before=dump();
  assert.throws(()=>save({expected:'before',token:'failing-token',fail:true}),/NOT NULL/);
  assert.deepEqual(dump(),before);
  assert.equal(save({expected:'before',token:'retry-after-rollback'}),true);
`));

test("verified activity retention expires only v2 rows and preserves legacy sets", () => scenario(`
  const names=['h1','k1','d1','w1','v3h1','v2h9','v2k9','v2d4','v2w4','v2h10','v2k10','v2d5','v2w5'];
  for(const name of names) db.prepare('INSERT INTO activity VALUES (?,?)').run(name,'body-'+name);
  assert.equal(db.prepare(sql.CLAIM_ACTIVITY).get(null,'retention-token').value,'retention-token');
  db.prepare(sql.DROP_SETS).run(10,5,'wrong-token');
  assert.equal(rows('activity','name').length,names.length);
  db.prepare(sql.DROP_SETS).run(10,5,'retention-token');
  assert.deepEqual(rows('activity','name').map(r=>r.name),['d1','h1','k1','v2d5','v2h10','v2k10','v2w5','v3h1','w1']);
`));

test("full signer keys sharing a prefix stay distinct and app queries count high FIDs exactly", () => scenario(`
  const high=4294967295, other=2147483648;
  const a=identity(high,1), b=identity(high,2), c=identity(other,3), extra=identity(high,4), missing=identity(high,5);
  assert.equal(a.slice(0,21),b.slice(0,21));
  db.prepare(sql.STORE_KEYS).run(json([[a,9152],[b,309857],[c,9152],[extra,9152]]));
  db.prepare(sql.STORE_KEYS).run(json([[a,999]]));
  assert.equal(rows('verified_signer_apps','identity').length,4);
  assert.deepEqual(db.prepare(sql.KEY_APPS).all(json([a,b,missing])).map(r=>({...r})).sort((x,y)=>x.identity.localeCompare(y.identity)),[
    {identity:a,app:9152},{identity:b,app:309857},
  ]);
  const hours=Array.from({length:24},(_,i)=>'v2k'+(100+i));
  for(const name of hours) db.prepare('INSERT INTO activity VALUES (?,?)').run(name,json([a,b,c,extra,missing].sort()));
  const raw=db.prepare(sql.COUNT_APPS).all(json(hours),json([]));
  assert(raw.every(r=>r.invalid===0&&r.accounts===2),'global cardinality must deduplicate every account across all24 arrays');
  assert.deepEqual(raw.filter(r=>r.active===0).map(r=>({...r})),[{app:null,active:0,unknown:'[]',invalid:0,accounts:2}]);
  const counts=raw.filter(r=>r.active>0).map(r=>({app:r.app,active:r.active,unknown:JSON.parse(r.unknown)}));
  assert.deepEqual(counts.sort((x,y)=>(x.app??-1)-(y.app??-1)),[
    {app:null,active:1,unknown:[missing]}, {app:9152,active:2,unknown:[]}, {app:309857,active:1,unknown:[]},
  ]);
  assert.deepEqual(db.prepare(sql.APPS_OF).all(json([high,other])).map(r=>({...r})).sort((x,y)=>x.fid-y.fid||x.app-y.app),[
    {fid:other,app:9152},{fid:high,app:9152},{fid:high,app:309857},
  ]);
  assert.throws(()=>db.prepare('INSERT INTO verified_signer_apps VALUES (?,?,?)').run(identity(high,99),42,9152),/CHECK constraint/i);
`));

test("stored activity key windows validate canonical arrays, replace the current batch and bound only the unknown backlog", () => scenario(`
  const hours=Array.from({length:24},(_,i)=>'v2k'+(100+i));
  const count=(overrides=[])=>db.prepare(sql.COUNT_APPS).all(json(hours),json(overrides)).map(r=>({...r}));
  const put=(name,body)=>db.prepare('INSERT INTO activity VALUES (?,?) ON CONFLICT(name) DO UPDATE SET body=excluded.body').run(name,body);
  const clear=()=>db.exec('DELETE FROM activity');
  const old=identity(42,1),fresh=identity(7,2);
  assert.deepEqual(count(),[{app:null,active:0,unknown:'[]',invalid:0,accounts:0}],
    'an empty window still returns a validation/cardinality sentinel');
  db.prepare(sql.STORE_KEYS).run(json([[old,9152],[fresh,309857]]));
  put(hours[0],json([old]));
  put('v2k99',json([identity(99,1)])); // outside the requested24-hour window
  let answer=count([[hours[0],json([fresh])],[hours[1],json([fresh])]]);
  assert(answer.every(r=>r.invalid===0&&r.accounts===1));
  assert.deepEqual(answer.filter(r=>r.active>0).map(({app,active,unknown})=>({app,active,unknown})),
    [{app:309857,active:1,unknown:'[]'}],'an override replaces old keys; it never appends them');
  put(hours[0],'not JSON');
  assert(count([[hours[0],json([fresh])]]).every(r=>r.invalid===0&&r.accounts===1),
    'a valid current-run replacement must exclude the old corrupt body entirely');

  const malformed=[
    ['invalid JSON','not JSON'],['JSON object',json({})],['JSON null','null'],['JSON string',json('text')],
    ['JSON number','42'],['null entry',json([null])],['number entry',json([7])],['boolean entry',json([true])],
    ['object entry',json([{}])],['nested array',json([[fresh]])],
    ['zero FID',json([identity(0,1)])],['FID aboveu32',json([identity(4294967296,1)])],
    ['noncanonical FID',json(['000000000x'+fresh.slice(10)])],
    ['short key',json([fresh.slice(0,-2)])],['long key',json([fresh+'ab'])],
    ['nonhex key',json([fresh.slice(0,-1)+'g'])],['uppercase key',json([fresh.slice(0,-1)+'A'])],
    ['NUL suffix',json([fresh+String.fromCharCode(0)+'hidden'])],
    ['unsorted identities',json([old,fresh])],['duplicate identity',json([fresh,fresh])],
  ];
  for(const [label,body] of malformed){
    clear();put(hours[0],body);
    let rejected=false;
    try { const rows=count();rejected=rows.length>0&&rows.every(r=>r.invalid>0); }
    catch { rejected=true; }
    assert(rejected,'corrupt key window was accepted: '+label);
  }
  clear();
  assert(count([[hours[0],json([fresh])],[hours[0],json([fresh])]]).every(r=>r.invalid>0),
    'duplicate replacement rows cannot masquerade as canonical hourly sets');

  const unknown=[];
  for(let fid=1000;fid<1450;fid++) unknown.push(identity(fid,1));
  for(let fid=1000;fid<1100;fid++) unknown.push(identity(fid,2));
  unknown.sort();
  for(let i=0;i<hours.length;i++) put(hours[i],json(unknown.filter((_,j)=>j%24===i||j%17===0)));
  answer=count();
  assert(answer.every(r=>r.invalid===0&&r.accounts===450));
  const unknownRow=answer.find(r=>r.app===null&&r.active>0);
  assert.equal(unknownRow.active,450,'unknown account cardinality is not capped by the lookup backlog');
  const backlog=JSON.parse(unknownRow.unknown);
  assert.equal(backlog.length,400);
  assert.deepEqual(backlog,unknown.slice(0,400));
  assert.equal(new Set(backlog).size,400,'the bounded backlog still contains distinct complete keys');
`));
