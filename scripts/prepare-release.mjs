// Assign a monotonically increasing app version in the CI checkout only.
import { readFileSync, writeFileSync, appendFileSync, realpathSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';

const stable = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

export function nextVersion(base, tags) {
  assert.match(base, stable, 'The source version must be stable');
  const versions = [base, ...tags.filter(t => t.startsWith('v')).map(t => t.slice(1))]
    .filter(v => stable.test(v)).map(v => v.split('.').map(Number));
  versions.sort((a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2]);
  const [major, minor, patch] = versions.at(-1);
  return `${major}.${minor}.${patch + 1}`;
}

export function setVersion(root, version) {
  assert.match(version, stable);
  const edits = [];
  for (const file of ['Cargo.toml', 'tray/Cargo.toml']) {
    const path = new URL(file, root);
    const source = readFileSync(path, 'utf8');
    assert.match(source, /^version = "[^"]+"/m, `Missing version in ${file}`);
    edits.push([path, source.replace(/^version = "[^"]+"/m, `version = "${version}"`)]);
  }
  const lockPath = new URL('Cargo.lock', root);
  let lock = readFileSync(lockPath, 'utf8');
  for (const name of ['autotrim', 'autotrim-tray']) {
    const pattern = new RegExp(`(name = "${name}"\nversion = ")[^"]+(")`);
    assert.match(lock, pattern, `Missing ${name} in Cargo.lock`);
    lock = lock.replace(pattern, (_, before, after) => `${before}${version}${after}`);
  }
  edits.push([lockPath, lock]);
  const configPath = new URL('tray/tauri.conf.json', root);
  const config = JSON.parse(readFileSync(configPath, 'utf8'));
  config.version = version;
  edits.push([configPath, `${JSON.stringify(config, null, 2)}\n`]);
  for (const [path, content] of edits) writeFileSync(path, content);
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const root = new URL('../', import.meta.url);
  const base = JSON.parse(readFileSync(new URL('tray/tauri.conf.json', root))).version;
  // Include unpublished drafts so a failed build cannot reuse its version.
  const releases = execFileSync('gh', ['release', 'list', '--limit', '1000', '--json', 'tagName'], { encoding: 'utf8' });
  const tags = execFileSync('git', ['tag', '--list', 'v*'], { encoding: 'utf8' }).trim().split('\n');
  const version = nextVersion(base, [...tags, ...JSON.parse(releases).map(r => r.tagName)]);
  setVersion(root, version);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\ntag=v${version}\n`);
  console.log(`Prepared autoTrim v${version}`);
}
