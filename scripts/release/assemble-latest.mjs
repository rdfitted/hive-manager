import { copyFileSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { collectAssets, validateLatest } from './validate-latest.mjs';

function exactlyOne(assets, suffix) {
  const matches = [...assets.keys()].filter((name) => name.endsWith(suffix));
  if (matches.length !== 1) throw new Error(`Expected one ${suffix} asset, found ${matches.length}`);
  return matches[0];
}

export function assembleLatest({ artifactsDir, releaseAssetsDir, tag, repository, publicKey }) {
  if (!releaseAssetsDir || resolve(releaseAssetsDir) === resolve(artifactsDir)) {
    throw new Error('A separate release asset directory is required');
  }
  const downloaded = collectAssets(artifactsDir);
  const normalized = new Map();
  for (const [name, path] of downloaded) {
    // GitHub changes spaces in uploaded names. Use our own stable names instead.
    const uploadName = name.replace(/\s+/g, '.');
    if (normalized.has(uploadName)) throw new Error(`Asset names collide after normalization: ${uploadName}`);
    normalized.set(uploadName, path);
  }
  mkdirSync(releaseAssetsDir, { recursive: true });
  if (readdirSync(releaseAssetsDir).length) throw new Error('Release asset directory must be empty');
  for (const [name, path] of normalized) copyFileSync(path, join(releaseAssetsDir, name));
  const assets = collectAssets(releaseAssetsDir);
  const windows = exactlyOne(assets, '-setup.exe');
  const macos = exactlyOne(assets, '.app.tar.gz');
  const entry = (name) => ({
    url: `https://github.com/${repository}/releases/download/${tag}/${encodeURIComponent(name)}`,
    signature: readFileSync(assets.get(`${name}.sig`), 'utf8'),
  });
  const darwin = entry(macos);
  const manifest = {
    version: tag.slice(1),
    platforms: {
      'windows-x86_64': entry(windows),
      'darwin-aarch64': darwin,
      'darwin-x86_64': darwin,
    },
  };
  validateLatest({ manifest, artifactsDir: releaseAssetsDir, tag, repository, publicKey });
  return manifest;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  const value = (flag) => args[args.indexOf(flag) + 1];
  try {
    const config = JSON.parse(readFileSync(value('--config') ?? 'src-tauri/tauri.conf.json', 'utf8'));
    const manifest = assembleLatest({
      artifactsDir: value('--artifacts'),
      releaseAssetsDir: value('--release-assets'),
      tag: value('--tag'),
      repository: value('--repository'),
      publicKey: config.plugins.updater.pubkey,
    });
    writeFileSync(value('--output') ?? 'latest.json', `${JSON.stringify(manifest, null, 2)}\n`);
    console.log('Assembled one validated latest.json');
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
