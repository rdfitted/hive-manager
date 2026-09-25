import assert from 'node:assert/strict';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, it } from 'node:test';
import { checkVersion } from './check-version.mjs';
import { assembleLatest } from './assemble-latest.mjs';
import { validateLatest } from './validate-latest.mjs';

const temporary = [];
function temp() {
  const directory = mkdtempSync(join(tmpdir(), 'hive-release-test-'));
  temporary.push(directory);
  return directory;
}
afterEach(() => {
  for (const directory of temporary.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function versionTree() {
  const root = temp();
  mkdirSync(join(root, 'src-tauri'));
  writeFileSync(join(root, 'package.json'), JSON.stringify({ version: '0.55.0' }));
  writeFileSync(join(root, 'package-lock.json'), JSON.stringify({ version: '0.55.0', packages: { '': { version: '0.55.0' } } }));
  writeFileSync(join(root, 'src-tauri/tauri.conf.json'), JSON.stringify({ version: '0.55.0' }));
  writeFileSync(join(root, 'src-tauri/Cargo.toml'), '[package]\nname = "hive-manager"\nversion = "0.55.0"\n');
  writeFileSync(join(root, 'src-tauri/Cargo.lock'), '[[package]]\nname = "hive-manager"\nversion = "0.55.0"\n');
  return root;
}

describe('check-version', () => {
  it('accepts an exact tag and mutual agreement in dry-run mode', () => {
    const root = versionTree();
    assert.equal(checkVersion({ root, tag: 'v0.55.0' }), '0.55.0');
    assert.equal(checkVersion({ root, dryRun: true }), '0.55.0');
  });

  it('rejects a missing real tag', () => assert.throws(() => checkVersion({ root: versionTree() }), /tag is required/));

  for (const source of ['package.json', 'package-lock.json root', 'package-lock.json packages', 'tauri.conf.json', 'Cargo.toml', 'Cargo.lock']) {
    it(`rejects a mismatch in ${source}`, () => {
      const root = versionTree();
      const file = source.startsWith('package-lock') ? 'package-lock.json'
        : ['Cargo.toml', 'Cargo.lock', 'tauri.conf.json'].includes(source) ? `src-tauri/${source}` : source;
      const path = join(root, file);
      if (file.endsWith('.json')) {
        const data = JSON.parse(readFileSync(path, 'utf8'));
        if (source === 'package-lock.json packages') data.packages[''].version = '0.55.1';
        else data.version = '0.55.1';
        writeFileSync(path, JSON.stringify(data));
      } else {
        writeFileSync(path, readFileSync(path, 'utf8').replace('0.55.0', '0.55.1'));
      }
      assert.throws(() => checkVersion({ root, dryRun: true }), /differs/);
    });
  }
});

function signedArtifacts() {
  const artifactsDir = temp();
  const releaseAssetsDir = temp();
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const keyId = randomBytes(8);
  const rawPublic = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const keyText = `untrusted comment: minisign public key\n${Buffer.concat([Buffer.from('Ed'), keyId, rawPublic]).toString('base64')}\n`;
  const encodedPublicKey = Buffer.from(keyText).toString('base64');
  const signPayload = (filename) => {
    const payload = Buffer.from(`test payload for ${filename}`);
    const signature = sign(null, createHash('blake2b512').update(payload).digest(), privateKey);
    const comment = 'timestamp:0\tfile:fixture\tprehashed';
    const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), privateKey);
    const signatureText = `untrusted comment: test-only key\n${Buffer.concat([Buffer.from('ED'), keyId, signature]).toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`;
    writeFileSync(join(artifactsDir, filename), payload);
    writeFileSync(join(artifactsDir, `${filename}.sig`), signatureText);
  };
  signPayload('Hive Manager_0.55.0_x64-setup.exe');
  signPayload('Hive Manager.app.tar.gz');
  writeFileSync(join(artifactsDir, 'Hive Manager_0.55.0_universal.dmg'), 'installer');
  return { artifactsDir, releaseAssetsDir, publicKey: encodedPublicKey, tag: 'v0.55.0', repository: 'example/hive-manager' };
}

function validateAssembled(options, manifest) {
  return validateLatest({ ...options, artifactsDir: options.releaseAssetsDir, manifest });
}

describe('latest.json assembly and validation', () => {
  it('assembles all platforms with one universal Darwin updater', () => {
    const options = signedArtifacts();
    const manifest = assembleLatest(options);
    assert.deepEqual(Object.keys(manifest.platforms), ['windows-x86_64', 'darwin-aarch64', 'darwin-x86_64']);
    assert.equal(manifest.platforms['darwin-aarch64'].url, manifest.platforms['darwin-x86_64'].url);
    assert.match(manifest.platforms['windows-x86_64'].url, /Hive\.Manager_0\.55\.0_x64-setup\.exe$/);
    assert.match(manifest.platforms['darwin-aarch64'].url, /Hive\.Manager\.app\.tar\.gz$/);
    assert.equal(readFileSync(join(options.releaseAssetsDir, 'Hive.Manager.app.tar.gz'), 'utf8'), 'test payload for Hive Manager.app.tar.gz');
    assert.equal(readFileSync(join(options.releaseAssetsDir, 'Hive.Manager_0.55.0_universal.dmg'), 'utf8'), 'installer');
    assert.equal(validateAssembled(options, manifest), true);
  });

  for (const [name, mutate, error] of [
    ['missing platform', (manifest) => { delete manifest.platforms['darwin-x86_64']; }, /platform set/],
    ['empty signature', (manifest) => { manifest.platforms['windows-x86_64'].signature = ''; }, /empty signature/],
    ['wrong version', (manifest) => { manifest.version = '0.55.1'; }, /version differs/],
    ['DMG Darwin URL', (manifest) => { manifest.platforms['darwin-aarch64'].url = manifest.platforms['darwin-aarch64'].url.replace('.app.tar.gz', '.dmg'); }, /Darwin URL/],
  ]) {
    it(`rejects ${name}`, () => {
      const options = signedArtifacts();
      const manifest = structuredClone(assembleLatest(options));
      mutate(manifest);
      assert.throws(() => validateAssembled(options, manifest), error);
    });
  }

  it('rejects a signature that does not verify', () => {
    const options = signedArtifacts();
    const manifest = assembleLatest(options);
    writeFileSync(join(options.releaseAssetsDir, 'Hive.Manager_0.55.0_x64-setup.exe'), 'tampered');
    assert.throws(() => validateAssembled(options, manifest), /signature does not verify/);
  });

  it('rejects duplicate asset names across downloaded artifacts', () => {
    const options = signedArtifacts();
    mkdirSync(join(options.artifactsDir, 'duplicate'));
    writeFileSync(join(options.artifactsDir, 'duplicate/Hive Manager.app.tar.gz'), 'duplicate');
    assert.throws(() => assembleLatest(options), /Duplicate asset/);
  });

  it('rejects URLs containing spaces or %20', () => {
    const options = signedArtifacts();
    const manifest = assembleLatest(options);
    for (const name of ['Hive Manager.app.tar.gz', 'Hive%20Manager.app.tar.gz']) {
      const changed = structuredClone(manifest);
      changed.platforms['darwin-aarch64'].url = changed.platforms['darwin-aarch64'].url.replace('Hive.Manager.app.tar.gz', name);
      assert.throws(() => validateAssembled(options, changed), /URL contains whitespace/);
    }
  });

  it('rejects a manifest basename absent from assembled release assets', () => {
    const options = signedArtifacts();
    const manifest = assembleLatest(options);
    manifest.platforms['windows-x86_64'].url = manifest.platforms['windows-x86_64'].url.replace('Hive.Manager_', 'Missing.');
    assert.throws(() => validateAssembled(options, manifest), /URL basename is missing from release assets/);
  });

  it('rejects whitespace or %20 in any assembled asset name', () => {
    const options = signedArtifacts();
    const manifest = assembleLatest(options);
    for (const name of ['Hive Manager.dmg', 'Hive%20Manager.dmg']) {
      writeFileSync(join(options.releaseAssetsDir, name), 'installer');
      assert.throws(() => validateAssembled(options, manifest), /Release asset name contains whitespace/);
      rmSync(join(options.releaseAssetsDir, name));
    }
  });

  it('rejects collisions after normalizing spaces', () => {
    const options = signedArtifacts();
    writeFileSync(join(options.artifactsDir, 'Hive.Manager.app.tar.gz'), 'collision');
    assert.throws(() => assembleLatest(options), /collide after normalization/);
  });
});
