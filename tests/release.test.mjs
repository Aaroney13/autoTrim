import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, copyFileSync } from 'node:fs';
import { execFileSync, spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { nextVersion, setVersion } from '../scripts/prepare-release.mjs';
import { updateAssets } from '../scripts/verify-update.mjs';

test('every main build upgrades the source version and all previous app releases', () => {
  assert.equal(nextVersion('0.2.0', []), '0.2.1');
  assert.equal(nextVersion('0.2.0', ['v0.2.9', 'v0.2.10', 'cli-v9.0.0', 'v1.0.0-beta.1']), '0.2.11');
  assert.equal(nextVersion('0.2.0', ['v0.3.0', 'v0.2.99']), '0.3.1');
  assert.equal(nextVersion('1.0.0', ['v0.2.99']), '1.0.1');
  assert.throws(() => nextVersion('0.2.0-beta', []));
});

for (const [label, newline] of [['LF', '\n'], ['CRLF', '\r\n']]) {
  test(`the app, daemon, and lockfile receive the same version without changing dependencies (${label})`, () => {
    const dir = mkdtempSync(join(tmpdir(), 'autotrim-release-'));
    try {
      mkdirSync(join(dir, 'tray'));
      for (const file of ['Cargo.toml', 'tray/Cargo.toml', 'Cargo.lock', 'tray/tauri.conf.json']) {
        const source = readFileSync(new URL(`../${file}`, import.meta.url), 'utf8');
        writeFileSync(join(dir, file), source.replace(/\r?\n/g, newline));
      }
      const before = readFileSync(join(dir, 'Cargo.lock'), 'utf8');
      setVersion(pathToFileURL(`${dir}/`), '0.2.42');
      for (const file of ['Cargo.toml', 'tray/Cargo.toml']) {
        assert.match(readFileSync(join(dir, file), 'utf8'), /^version = "0.2.42"/m);
      }
      assert.equal(JSON.parse(readFileSync(join(dir, 'tray/tauri.conf.json'))).version, '0.2.42');
      const normalize = s => s.replace(/(name = "autotrim(?:-tray)?"\r?\nversion = ")[^"]+/g, '$1VERSION');
      const after = readFileSync(join(dir, 'Cargo.lock'), 'utf8');
      assert.equal(normalize(after), normalize(before));
      assert.match(after, /name = "autotrim"\r?\nversion = "0.2.42"/);
      assert.match(after, /name = "autotrim-tray"\r?\nversion = "0.2.42"/);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });
}

function fixture() {
  const archive = { name: 'autoTrim.app.tar.gz', size: 100, url: 'https://api.github.com/repos/Aaroney13/autoTrim/releases/assets/123', browser_download_url: 'https://github.com/Aaroney13/autoTrim/releases/download/v0.2.1/autoTrim.app.tar.gz' };
  const release = { tag_name: 'v0.2.1', assets: [archive, { name: 'autoTrim.dmg', size: 100 }] };
  const manifest = { version: '0.2.1', platforms: Object.fromEntries(['darwin-aarch64', 'darwin-x86_64'].map(p => [p, { url: archive.url, signature: 'signature' }])) };
  return { manifest, release };
}

test('the feed covers both Macs and only references uploaded archives', () => {
  const { manifest, release } = fixture();
  manifest.platforms['darwin-x86_64'].url = release.assets[0].browser_download_url;
  assert.equal(updateAssets(manifest, release, '0.2.1').length, 2);
});

test('incomplete or mismatched releases cannot be published', () => {
  for (const breakRelease of [
    f => { f.manifest.version = '0.2.0'; },
    f => { f.release.tag_name = 'v0.2.0'; },
    f => { delete f.manifest.platforms['darwin-x86_64']; },
    f => { f.manifest.platforms['darwin-aarch64'].signature = ''; },
    f => { f.manifest.platforms['darwin-aarch64'].url = 'https://example.com/other.app.tar.gz'; },
    f => { f.release.assets[0].size = 0; },
    f => { f.release.assets.pop(); },
  ]) {
    const f = fixture();
    breakRelease(f);
    assert.throws(() => updateAssets(f.manifest, f.release, '0.2.1'));
  }
});

// The release job installs minisign. Developer/other CI hosts can still run
// the dependency-free version and feed tests without installing it.
test('the publication command verifies real signatures and rejects tampered archives', {
  skip: spawnSync('minisign', ['-v']).status !== 0,
}, () => {
  const dir = mkdtempSync(join(tmpdir(), 'autotrim-signature-test-'));
  try {
    mkdirSync(join(dir, 'scripts'));
    mkdirSync(join(dir, 'tray'));
    copyFileSync(new URL('../scripts/verify-update.mjs', import.meta.url), join(dir, 'scripts/verify-update.mjs'));
    execFileSync('minisign', ['-G', '-W', '-p', join(dir, 'test.pub'), '-s', join(dir, 'test.key')], { stdio: 'pipe' });
    writeFileSync(join(dir, 'tray/tauri.conf.json'), JSON.stringify({ plugins: { updater: { pubkey: readFileSync(join(dir, 'test.pub')).toString('base64') } } }));
    const archive = join(dir, 'autoTrim.app.tar.gz');
    writeFileSync(archive, 'signed archive fixture');
    const { manifest, release } = fixture();
    writeFileSync(join(dir, 'release.json'), JSON.stringify(release));
    const args = [join(dir, 'scripts/verify-update.mjs'), dir, '0.2.1'];
    for (const legacy of [false, true]) {
      execFileSync('minisign', ['-S', ...(legacy ? ['-l'] : []), '-s', join(dir, 'test.key'), '-m', archive], { stdio: 'pipe' });
      const signature = readFileSync(`${archive}.minisig`).toString('base64');
      for (const entry of Object.values(manifest.platforms)) entry.signature = signature;
      writeFileSync(join(dir, 'latest.json'), JSON.stringify(manifest));
      assert.match(execFileSync(process.execPath, args, { encoding: 'utf8' }), /Verified v0.2.1/);
    }
    writeFileSync(archive, 'tampered archive');
    assert.notEqual(spawnSync(process.execPath, args).status, 0);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
