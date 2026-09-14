// Refuse a release whose tag, CLI, app, or lockfile versions disagree.
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
const read = name => readFileSync(new URL(`../${name}`, import.meta.url), 'utf8');
const version = JSON.parse(read('tray/tauri.conf.json')).version;
assert.match(version, /^\d+\.\d+\.\d+$/, 'Stable releases need an x.y.z version');
for (const file of ['Cargo.toml', 'tray/Cargo.toml']) {
  assert.equal(read(file).match(/^version = "([^"]+)"/m)?.[1], version, `${file} version differs`);
}
for (const pkg of ['autotrim', 'autotrim-tray']) {
  assert.equal(read('Cargo.lock').match(new RegExp(`name = "${pkg}"\nversion = "([^"]+)"`))?.[1], version, `${pkg} lockfile version differs`);
}
if (process.argv[2]) assert.equal(process.argv[2], `v${version}`, 'Tag must match the app version');
const updater = JSON.parse(read('tray/tauri.conf.json')).plugins.updater;
assert.ok(Buffer.from(updater.pubkey, 'base64').toString().startsWith('untrusted comment:'), 'Missing updater public key');
assert.deepEqual(updater.endpoints, ['https://github.com/Aaroney13/autoTrim/releases/latest/download/latest.json']);
console.log(`Release configuration valid: v${version}`);
