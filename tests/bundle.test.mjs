import test from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Replace only the expensive Rust compilation with tiny native executables.
// Run the real staging script, copies, and lipo against a clean checkout.
const fixtureBuild = `
  cargo() {
    local target="\${@: -1}" arch
    case "$target" in
      aarch64-apple-darwin) arch=arm64 ;;
      x86_64-apple-darwin) arch=x86_64 ;;
      *) return 1 ;;
    esac
    mkdir -p "target/$target/release"
    printf 'int main(void) { return 0; }\\n' |
      clang -arch "$arch" -x c - -o "target/$target/release/autotrim"
  }
  export -f cargo
  exec bash "$@"
`;

for (const target of ['universal-apple-darwin', 'aarch64-apple-darwin']) {
  test(`bundle preparation stages every sidecar needed for ${target}`, {
    skip: process.platform !== 'darwin',
  }, () => {
    const dir = mkdtempSync(join(tmpdir(), 'autotrim-bundle-'));
    try {
      mkdirSync(join(dir, 'scripts'));
      const script = join(dir, 'scripts/prepare-bundle.sh');
      copyFileSync(new URL('../scripts/prepare-bundle.sh', import.meta.url), script);
      execFileSync('bash', ['-c', fixtureBuild, 'bundle-test', script, target], { stdio: 'pipe' });

      const targets = target === 'universal-apple-darwin'
        ? ['aarch64-apple-darwin', 'x86_64-apple-darwin'] : [target];
      for (const triple of targets) {
        const sidecar = join(dir, `tray/binaries/autotrim-${triple}`);
        assert.deepEqual(readFileSync(sidecar), readFileSync(join(dir, `target/${triple}/release/autotrim`)));
        assert.ok(statSync(sidecar).mode & 0o111, `${triple} sidecar must remain executable`);
      }
      const architectures = target === 'universal-apple-darwin' ? ['arm64', 'x86_64'] : ['arm64'];
      const bundled = join(dir, `tray/binaries/autotrim-${target}`);
      execFileSync('lipo', [bundled, '-verify_arch', ...architectures]);
      assert.ok(statSync(bundled).mode & 0o111, 'Bundled daemon must remain executable');

      if (target === 'universal-apple-darwin') {
        // Exercise the release job's actual architecture checks, including
        // argument ordering, before relying on them to gate publication.
        const workflow = readFileSync(new URL('../.github/workflows/release.yml', import.meta.url), 'utf8');
        const checks = workflow.match(/^\s+lipo .+$/gm);
        assert.ok(checks?.length, 'The release must verify bundled architectures');
        const macos = join(dir, 'release-check/autoTrim.app/Contents/MacOS');
        mkdirSync(macos, { recursive: true });
        for (const name of ['autotrim-tray', 'autotrim']) copyFileSync(bundled, join(macos, name));
        const verify = () => execFileSync('bash', ['-e', '-c', checks.join('\n')], { cwd: dir, stdio: 'pipe' });
        verify();
        for (const name of ['autotrim-tray', 'autotrim']) {
          copyFileSync(join(dir, 'tray/binaries/autotrim-aarch64-apple-darwin'), join(macos, name));
          assert.throws(verify, `The release must reject a non-universal ${name}`);
          copyFileSync(bundled, join(macos, name));
        }
      }
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });
}
