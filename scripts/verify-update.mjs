// Validate the uploaded feed and cryptographically verify both Mac entries.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, realpathSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export function updateAssets(manifest, release, version) {
  assert.equal(manifest.version, version, 'Updater version differs from the app');
  assert.equal(release.tag_name, `v${version}`, 'Release tag differs from the app');
  assert.ok(release.assets.some(a => a.name.endsWith('.dmg') && a.size > 0), 'Missing DMG');
  return ['darwin-aarch64', 'darwin-x86_64'].map(platform => {
    const entry = manifest.platforms?.[platform];
    assert.ok(entry?.signature?.trim(), `Missing signature for ${platform}`);
    const asset = release.assets.find(a => entry.url === a.url || entry.url === a.browser_download_url);
    assert.ok(asset?.name.endsWith('.app.tar.gz') && asset.size > 0, `Missing update archive for ${platform}`);
    return { name: asset.name, signature: entry.signature };
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const [directory, version] = process.argv.slice(2);
  const manifest = JSON.parse(readFileSync(join(directory, 'latest.json')));
  const release = JSON.parse(readFileSync(join(directory, 'release.json')));
  const config = JSON.parse(readFileSync(new URL('../tray/tauri.conf.json', import.meta.url)));
  const publicKey = Buffer.from(config.plugins.updater.pubkey, 'base64').toString().trim().split('\n')[1];
  assert.ok(publicKey, 'Missing public verification key');
  for (const asset of updateAssets(manifest, release, version)) {
    const archive = join(directory, asset.name);
    const signature = `${archive}.minisig`;
    writeFileSync(signature, Buffer.from(asset.signature, 'base64'));
    execFileSync('minisign', ['-Vm', archive, '-x', signature, '-P', publicKey], { stdio: 'inherit' });
  }
  console.log(`Verified v${version} update signatures for Apple Silicon and Intel`);
}
