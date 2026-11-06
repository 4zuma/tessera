#!/usr/bin/env node
// Sets the Tessera version everywhere it is written down.
//
//   node scripts/bump-version.mjs 0.3.1     set the version to 0.3.1
//   node scripts/bump-version.mjs --check   list the versions, fail if they differ
//
// Cargo, npm and Tauri each keep their own copy of the version:
//   Cargo.toml               [workspace.package] (tessera-core and tessera-cli inherit it)
//   app/src-tauri/Cargo.toml [package] (the app crate)
//   Cargo.lock               one entry per workspace crate
//   app/src-tauri/tauri.conf.json   the version the exe reports
//   app/package.json, app/package-lock.json (two root entries)
//   app/src/lib/backend/mock/index.ts   the browser-only preview backend
//
// Merging the bump to main releases it: the release workflow sees the new
// version in tauri.conf.json, tags the merge commit and publishes.
//
// Every file is read and checked before any is written: if one place cannot
// be found, nothing changes.

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (path) => readFileSync(join(root, path), 'utf8');

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

class Fail extends Error {}

/** The `version = "…"` line of a TOML table, as { line index, value }. */
function tomlVersion(text, table, path) {
  const lines = text.split('\n');
  const start = lines.findIndex((l) => l.trim() === `[${table}]`);
  if (start < 0) throw new Fail(`${path}: no [${table}] table`);
  for (let i = start + 1; i < lines.length && !lines[i].trimStart().startsWith('['); i++) {
    const m = /^version\s*=\s*"([^"]*)"\s*\r?$/.exec(lines[i]);
    if (m) return { lines, index: i, value: m[1] };
  }
  throw new Fail(`${path}: no version in [${table}]`);
}

function tomlPlace(path, table) {
  return {
    label: `${path} [${table}]`,
    path,
    get(text) {
      return [tomlVersion(text, table, path).value];
    },
    set(text, version) {
      const { lines, index } = tomlVersion(text, table, path);
      lines[index] = lines[index].replace(/"[^"]*"/, `"${version}"`);
      return lines.join('\n');
    },
  };
}

/** Workspace crate names, from the members listed in the root Cargo.toml. */
function workspaceCrates() {
  const members = /^members\s*=\s*\[([^\]]*)\]/m.exec(read('Cargo.toml'));
  if (!members) throw new Fail('Cargo.toml: no workspace members');
  return [...members[1].matchAll(/"([^"]+)"/g)].map(([, dir]) => {
    const path = `${dir}/Cargo.toml`;
    const text = read(path);
    const lines = text.split('\n');
    const start = lines.findIndex((l) => l.trim() === '[package]');
    for (let i = start + 1; start >= 0 && i < lines.length && !lines[i].trimStart().startsWith('['); i++) {
      const m = /^name\s*=\s*"([^"]+)"/.exec(lines[i]);
      if (m) return m[1];
    }
    throw new Fail(`${path}: no [package] name`);
  });
}

function lockPlace(crates) {
  const entry = (name) => new RegExp(`(\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = ")([^"]*)(")`, 'g');
  const find = (text, name) => {
    const found = [...text.matchAll(entry(name))];
    if (found.length !== 1) throw new Fail(`Cargo.lock: ${found.length} entries for ${name}, expected 1`);
    return found[0];
  };
  return {
    label: `Cargo.lock (${crates.join(', ')})`,
    path: 'Cargo.lock',
    get(text) {
      return crates.map((name) => find(text, name)[2]);
    },
    set(text, version) {
      for (const name of crates) {
        find(text, name);
        text = text.replace(entry(name), `$1${version}$3`);
      }
      return text;
    },
  };
}

/**
 * Version fields of a JSON file. The text is edited in place to keep its
 * formatting, then parsed again and compared with the expected object, so a
 * replacement that touched anything else is caught.
 */
function jsonPlace(path, fields) {
  return {
    label: path,
    path,
    get(text) {
      const data = JSON.parse(text);
      return fields.map((f) => f.get(data));
    },
    set(text, version) {
      const expected = JSON.parse(text);
      for (const f of fields) {
        const old = f.get(expected);
        f.set(expected, version);
        const at = text.indexOf(`"version": "${old}"`);
        if (at < 0) throw new Fail(`${path}: cannot find "version": "${old}"`);
        text = text.slice(0, at) + `"version": "${version}"` + text.slice(at + `"version": "${old}"`.length);
      }
      if (JSON.stringify(JSON.parse(text)) !== JSON.stringify(expected)) {
        throw new Fail(`${path}: editing the version changed something else; edit it by hand`);
      }
      return text;
    },
  };
}

function mockPlace(path) {
  const re = /(\bversion: ')([^']*)(')/g;
  const find = (text) => {
    const found = [...text.matchAll(re)];
    if (found.length !== 1) throw new Fail(`${path}: ${found.length} "version: '…'" entries, expected 1`);
    return found[0];
  };
  return {
    label: path,
    path,
    get: (text) => [find(text)[2]],
    set(text, version) {
      find(text);
      return text.replace(re, `$1${version}$3`);
    },
  };
}

const top = { get: (d) => d.version, set: (d, v) => (d.version = v) };
const lockRoot = { get: (d) => d.packages?.['']?.version, set: (d, v) => (d.packages[''].version = v) };

function places() {
  return [
    tomlPlace('Cargo.toml', 'workspace.package'),
    tomlPlace('app/src-tauri/Cargo.toml', 'package'),
    lockPlace(workspaceCrates()),
    jsonPlace('app/src-tauri/tauri.conf.json', [top]),
    jsonPlace('app/package.json', [top]),
    jsonPlace('app/package-lock.json', [top, lockRoot]),
    mockPlace('app/src/lib/backend/mock/index.ts'),
  ];
}

function check() {
  const seen = new Set();
  for (const place of places()) {
    const versions = place.get(read(place.path));
    versions.forEach((v) => seen.add(v));
    console.log(`  ${[...new Set(versions)].join(' / ').padEnd(12)} ${place.label}`);
  }
  if (seen.size !== 1) {
    console.error(`\nThe versions differ (${[...seen].join(', ')}). Run: node scripts/bump-version.mjs <version>`);
    process.exit(1);
  }
  console.log(`\nAll at ${[...seen][0]}.`);
}

function bump(version) {
  const writes = [];
  const before = new Set();
  for (const place of places()) {
    const text = read(place.path);
    place.get(text).forEach((v) => before.add(v));
    const next = place.set(text, version);
    if (place.get(next).some((v) => v !== version)) throw new Fail(`${place.path}: the new version did not stick`);
    writes.push({ place, text, next });
  }
  for (const { place, text, next } of writes) {
    if (next !== text) writeFileSync(join(root, place.path), next);
    console.log(`  ${next === text ? 'unchanged' : 'updated  '} ${place.label}`);
  }
  console.log(`\n${[...before].join(', ')} -> ${version}`);
  console.log(
    'Next: commit, open a pull request and merge it once CI is green. Merging it to main\n' +
      `tags v${version} and publishes the release by itself (.github/workflows/release.yml).\n` +
      'Write the release notes in the pull request description between <!-- release-notes -->\n' +
      'and <!-- /release-notes -->; without them GitHub writes the notes.',
  );
}

try {
  const arg = process.argv[2];
  if (arg === '--check') {
    check();
  } else if (arg && SEMVER.test(arg.replace(/^v/, ''))) {
    bump(arg.replace(/^v/, ''));
  } else {
    console.error('Usage: node scripts/bump-version.mjs <version>   e.g. 0.3.1\n       node scripts/bump-version.mjs --check');
    process.exit(2);
  }
} catch (e) {
  if (!(e instanceof Fail)) throw e;
  console.error(`Nothing was changed. ${e.message}`);
  process.exit(1);
}
