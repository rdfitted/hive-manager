import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export const PLATFORM_KEYS = ['windows-x86_64', 'darwin-aarch64', 'darwin-x86_64'];
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

export function collectAssets(directory) {
  const assets = new Map();
  function walk(path) {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const file = join(path, entry.name);
      if (entry.isDirectory()) walk(file);
      else if (entry.isFile()) {
        if (assets.has(entry.name)) throw new Error(`Duplicate asset: ${entry.name}`);
        assets.set(entry.name, file);
      }
    }
  }
  walk(directory);
  return assets;
}

function decodePublicKey(encoded) {
  const outer = Buffer.from(encoded, 'base64').toString('utf8').trim();
  const line = outer.includes('\n') ? outer.split(/\r?\n/)[1] : encoded;
  const bytes = Buffer.from(line, 'base64');
  if (bytes.length !== 42 || !['Ed', 'ED'].includes(bytes.subarray(0, 2).toString())) {
    throw new Error('Invalid minisign public key');
  }
  return {
    id: bytes.subarray(2, 10),
    key: createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, bytes.subarray(10)]), format: 'der', type: 'spki' }),
  };
}

export function verifyMinisign(payload, signatureText, encodedPublicKey) {
  const lines = signatureText.trim().split(/\r?\n/);
  if (lines.length !== 4 || !lines[0].startsWith('untrusted comment: ') || !lines[2].startsWith('trusted comment: ')) {
    throw new Error('Invalid minisign signature format');
  }
  const signature = Buffer.from(lines[1], 'base64');
  const globalSignature = Buffer.from(lines[3], 'base64');
  if (signature.length !== 74 || globalSignature.length !== 64) throw new Error('Invalid minisign signature length');
  const algorithm = signature.subarray(0, 2).toString();
  if (algorithm !== 'ED' && algorithm !== 'Ed') throw new Error('Unsupported minisign algorithm');
  const publicKey = decodePublicKey(encodedPublicKey);
  if (!signature.subarray(2, 10).equals(publicKey.id)) throw new Error('Minisign key ID differs from configured pubkey');
  const message = algorithm === 'ED' ? createHash('blake2b512').update(payload).digest() : payload;
  if (!verify(null, message, publicKey.key, signature.subarray(10))) throw new Error('Payload signature does not verify');
  const trustedComment = lines[2].slice('trusted comment: '.length);
  if (!verify(null, Buffer.concat([signature.subarray(10), Buffer.from(trustedComment)]), publicKey.key, globalSignature)) {
    throw new Error('Trusted comment signature does not verify');
  }
}

export function validateLatest({ manifest, artifactsDir, tag, repository, publicKey }) {
  if (!/^v\d+\.\d+\.\d+$/.test(tag ?? '')) throw new Error('Invalid release tag');
  if (manifest.version !== tag.slice(1)) throw new Error('Manifest version differs from tag');
  const platforms = manifest.platforms;
  if (!platforms || Object.keys(platforms).sort().join(',') !== [...PLATFORM_KEYS].sort().join(',')) {
    throw new Error('Manifest platform set is incomplete or unexpected');
  }
  const assets = collectAssets(artifactsDir);
  for (const name of assets.keys()) {
    if (/\s|%20/i.test(name)) throw new Error(`Release asset name contains whitespace: ${name}`);
  }
  for (const platform of PLATFORM_KEYS) {
    const entry = platforms[platform];
    if (!entry?.signature) throw new Error(`${platform} has an empty signature`);
    const prefix = `https://github.com/${repository}/releases/download/${tag}/`;
    if (typeof entry.url !== 'string' || !entry.url.startsWith(prefix)) throw new Error(`${platform} URL is not tag-specific`);
    if (/\s|%20/i.test(entry.url)) throw new Error(`${platform} URL contains whitespace`);
    const filename = decodeURIComponent(entry.url.slice(prefix.length));
    if (basename(filename) !== filename || !filename) throw new Error(`${platform} URL has an invalid asset name`);
    if (/\s|%20/i.test(filename)) throw new Error(`${platform} URL has whitespace in its asset name`);
    if (entry.url !== prefix + encodeURIComponent(filename)) throw new Error(`${platform} URL is not canonical`);
    if (platform.startsWith('darwin') && !filename.endsWith('.app.tar.gz')) throw new Error('Darwin URL must name a .app.tar.gz updater');
    if (platform.startsWith('windows') && !filename.endsWith('-setup.exe')) throw new Error('Windows URL must name an NSIS setup.exe updater');
    const payloadPath = assets.get(filename);
    const signaturePath = assets.get(`${filename}.sig`);
    if (!payloadPath) throw new Error(`${platform} URL basename is missing from release assets: ${filename}`);
    if (!signaturePath) throw new Error(`${platform} updater .sig is missing`);
    const signature = readFileSync(signaturePath, 'utf8');
    if (entry.signature !== signature) throw new Error(`${platform} signature differs from .sig contents`);
    verifyMinisign(readFileSync(payloadPath), signature, publicKey);
  }
  if (platforms['darwin-aarch64'].url !== platforms['darwin-x86_64'].url) throw new Error('Darwin entries must use one universal artifact');
  return true;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  const value = (flag) => args[args.indexOf(flag) + 1];
  try {
    const config = JSON.parse(readFileSync(value('--config') ?? 'src-tauri/tauri.conf.json', 'utf8'));
    validateLatest({
      manifest: JSON.parse(readFileSync(value('--manifest'), 'utf8')),
      artifactsDir: value('--artifacts'),
      tag: value('--tag'),
      repository: value('--repository'),
      publicKey: config.plugins.updater.pubkey,
    });
    console.log('latest.json is complete and all updater signatures verify');
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
