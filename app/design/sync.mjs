#!/usr/bin/env node
/**
 * Prepares a re-sync of the published design system (design/README.md › Re-sync): compares the
 * fresh build in design/out/project with the published files, checks that the build still holds
 * everything the published system was built from, and writes the index to publish last.
 *
 *   npm run design:sync -- --live <dir> --note "<what changed>" [--drop <sha>]…
 *
 * <dir> is the folder the Artifact tool saved the published files into (it holds project/…).
 * Read every published file into it first: a file missing there counts as new.
 * Prints what to publish; changes nothing that is published.
 */
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(here, 'out');
const built = path.join(out, 'project');

/* ------------------------------------------------------------ args */

const args = process.argv.slice(2);
const opt = name => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const drops = args.flatMap((a, i) => (a === '--drop' ? [args[i + 1]] : []));
const liveDir = opt('--live');
const note = opt('--note');
if (!liveDir || !note) {
  console.error('usage: npm run design:sync -- --live <dir> --note "<what changed>" [--drop <sha>]…');
  process.exit(2);
}
const live = path.join(path.resolve(liveDir), 'project');
if (!existsSync(path.join(live, 'design-system.json'))) {
  console.error(`no project/design-system.json under ${liveDir}: read the published files into it first`);
  process.exit(2);
}
if (!existsSync(path.join(built, 'components', 'bundle.js'))) {
  console.error('no build: run `npm run design` first');
  process.exit(2);
}

function git(...a) {
  const r = spawnSync('git', a, { cwd: here, encoding: 'utf8' });
  return { ok: r.status === 0, out: (r.stdout || '').trim() };
}

/* ------------------------------------------------------------ sources */

// design/out must be a build of HEAD (build.mjs stamps tokens.json with branch@sha)
const stamp = String(JSON.parse(readFileSync(path.join(built, 'tokens.json'), 'utf8')).meta?.ref ?? '');
const headSha = git('rev-parse', '--short', 'HEAD').out;
if (!stamp.endsWith(`@${headSha}`)) {
  console.error(`design/out is a build of ${stamp || 'something else'}, not of HEAD (${headSha}): run \`npm run design\` again`);
  process.exit(1);
}

// The published index names the commits its files were built from ("GitHub · owner/repo@a + b").
// A build that leaves one of them out would take its changes off the canvas again.
const index = JSON.parse(readFileSync(path.join(live, 'design-system.json'), 'utf8'));
const via = String(index.lastChange?.via ?? '');
const shas = via.includes('@') ? [...via.slice(via.indexOf('@')).matchAll(/\b[0-9a-f]{7,40}\b/g)].map(m => m[0]) : [];
let blocked = false;
for (const s of shas) {
  if (drops.some(d => s.startsWith(d) || d.startsWith(s))) {
    console.log(`  dropped on purpose: ${s}`);
    continue;
  }
  if (!git('cat-file', '-e', `${s}^{commit}`).ok) {
    console.log(`✗ the published system was built from ${s}, which this clone does not have: git fetch origin <its branch> (or main), then run this again`);
    blocked = true;
  } else if (!git('merge-base', '--is-ancestor', s, 'HEAD').ok) {
    const branches = git('branch', '-r', '--contains', s).out.split('\n').map(b => b.trim()).filter(Boolean);
    console.log(`✗ the published system was built from ${s} (${branches.join(', ') || 'no remote branch'}), which this build leaves out.`);
    console.log(`  Merge it into the commit you build from (a local branch is fine), rebuild, and run this again;`);
    console.log(`  or, if that work was dropped, pass --drop ${s}.`);
    blocked = true;
  }
}
if (blocked) process.exit(1);
if (git('status', '--porcelain', '--', '..').out) console.log('! app/ has uncommitted changes: the build is not from a commit, so `via` will not name it');

/** the commits to name in `via`: HEAD when a remote branch holds it, else the parents of a local merge */
function sources() {
  const head = git('rev-parse', '--short', 'HEAD').out;
  if (git('branch', '-r', '--contains', 'HEAD').out) return [head];
  const parents = git('rev-list', '--parents', '-n', '1', 'HEAD').out.split(' ').slice(1);
  if (parents.length > 1) {
    const unpushed = parents.filter(p => !git('branch', '-r', '--contains', p).out);
    if (unpushed.length) console.log(`! not pushed yet: ${unpushed.map(p => p.slice(0, 7)).join(', ')}. Push them first, so other threads can merge them.`);
    return parents.map(p => p.slice(0, 7));
  }
  console.log(`! HEAD ${head} is not pushed yet. Push it first, so other threads can merge it.`);
  return [head];
}
const repo = (() => {
  const m = /github\.com[/:]([^/]+\/[^/.]+)/.exec(git('remote', 'get-url', 'origin').out);
  return m ? m[1] : '4zuma/tessera';
})();
const newVia = `GitHub · ${repo}@${sources().join(' + ')}`;

/* ------------------------------------------------------------ files */

function walk(dir, base = dir) {
  return readdirSync(dir).flatMap(n => {
    const p = path.join(dir, n);
    return statSync(p).isDirectory() ? walk(p, base) : [path.relative(base, p).split(path.sep).join('/')];
  });
}
/** build stamps (branch@sha, date) are not changes */
function comparable(rel, buf) {
  if (rel !== 'tokens.json') return buf;
  const t = JSON.parse(buf.toString('utf8'));
  if (t.meta) {
    delete t.meta.ref;
    delete t.meta.synced;
  }
  return Buffer.from(JSON.stringify(t));
}
const SKIP = new Set(['design-system.json']);
// the logos are uploaded assets (blob records in the index), not files
const isAsset = rel => rel.startsWith('assets/') && rel.endsWith('.svg');

const changed = [];
const added = [];
for (const rel of walk(built).sort()) {
  if (SKIP.has(rel) || isAsset(rel)) continue;
  const mine = path.join(built, rel);
  const theirs = path.join(live, rel);
  if (!existsSync(theirs)) added.push(rel);
  else if (!comparable(rel, readFileSync(mine)).equals(comparable(rel, readFileSync(theirs)))) changed.push(rel);
}
const gone = existsSync(live)
  ? walk(live).filter(rel => !SKIP.has(rel) && !isAsset(rel) && !existsSync(path.join(built, rel)))
  : [];

/* ------------------------------------------------------------ report */

const send = [...changed, ...added];
if (!send.length && !gone.length) {
  console.log('Nothing to publish: the published system already matches this build.');
  process.exit(0);
}
const files = {};
for (const rel of send) {
  const key = `project/${rel}`;
  files[key] = rel.endsWith('.ts') ? { from: key, contentType: 'text/plain' } : key;
}
for (const rel of gone) files[`project/${rel}`] = null;

index.lastChange = { ...index.lastChange, at: new Date().toISOString().replace(/\.\d+Z$/, 'Z'), via: newVia, note };
if (index.editing !== 'code') console.log('! the published index is not "editing": "code"; kept as it is');
writeFileSync(path.join(built, 'design-system.json'), JSON.stringify(index, null, 2) + '\n');

console.log(`\nChanged: ${changed.join(', ') || 'none'}`);
if (added.length) console.log(`New (or not read: read it first if it is published): ${added.join(', ')}`);
if (gone.length) console.log(`Published but no longer built (removed with null): ${gone.join(', ')}`);
console.log(`\n1. Publish these with root = ${out}`);
console.log(`   (file_path = any one of them, files = the rest):`);
console.log(JSON.stringify(files, null, 2));
console.log(`\n2. Then publish the index last: file_path = ${path.join(built, 'design-system.json')}, same root.`);
console.log(`   lastChange.via = "${newVia}"`);
const COPIED = ['tokens.json', 'tokens.css', 'components/bundle.js', 'components/bundle.css'];
const recopy = COPIED.filter(rel => send.includes(rel));
console.log(`\n3. The canvas copies ${recopy.length ? recopy.join(', ') : 'none of the changed files'}${recopy.length ? ' again (README › The canvas):' : '; still set its designSystems version.'}`);
for (const rel of recopy) console.log(`   "project/ds/tessera/${rel}": {"artifact": "<system url>", "path": "project/${rel}"}`);
