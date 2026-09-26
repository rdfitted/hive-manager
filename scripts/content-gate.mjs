import { createHash } from 'node:crypto';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const DEFAULT_DIRS = ['workflow-pack', 'docs/wiki-starter', '.claude/skills', '.agents/skills'];
const DENYLIST_RELATIVE = 'workflow-pack/content-denylist.sha256';
const PLAIN_PATTERNS = [
  /c:\\users/i,
  /rduff/i,
  /mainbrain/i,
  /fitted-automation/i,
  /@fitted/i,
];
const POSIX_ALTERNATIVE = /(?:\.sh\b|\b(?:bash|sh)\s+(?:-c\b|(?:\.{0,2}\/|\/|scripts\/)\S+)|\bnode\s+(?:\.\/)?scripts\/\S+)/i;

export function normalize(value) {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim();
}

function sha256(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

export function findDeniedNgram(line, hashes) {
  const words = normalize(line).split(' ').filter(Boolean);
  for (let start = 0; start < words.length; start += 1) {
    for (let length = 1; length <= 4 && start + length <= words.length; length += 1) {
      if (hashes.has(sha256(words.slice(start, start + length).join(' ')))) {
        return { start: start + 1, length };
      }
    }
  }
  return null;
}

export function parseDenylist(contents) {
  const hashes = new Set();
  for (const [index, raw] of contents.split(/\r?\n/).entries()) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    if (!/^[a-f0-9]{64}$/i.test(line)) {
      throw new Error(`Invalid denylist hash at line ${index + 1}`);
    }
    hashes.add(line.toLowerCase());
  }
  if (hashes.size === 0) throw new Error('Denylist has no hashes');
  return hashes;
}

async function collectFiles(target) {
  let entries;
  try {
    entries = await readdir(target, { withFileTypes: true });
  } catch (error) {
    if (error.code === 'ENOENT') return [];
    if (error.code === 'ENOTDIR') return [target];
    throw error;
  }
  const files = [];
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    const child = path.join(target, entry.name);
    if (entry.isDirectory()) files.push(...await collectFiles(child));
    else if (entry.isFile()) files.push(child);
    else if (entry.isSymbolicLink()) throw new Error(`Symlink in scanned tree: ${child}`);
  }
  return files;
}

function fencedBlockIds(lines) {
  const ids = [];
  let fence = null;
  let nextId = 0;
  for (const line of lines) {
    const marker = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (marker && !fence) {
      fence = { character: marker[1][0], length: marker[1].length, id: ++nextId };
      ids.push(null);
    } else if (marker && fence && marker[1][0] === fence.character
        && marker[1].length >= fence.length && !marker[2].trim()) {
      ids.push(null);
      fence = null;
    } else {
      ids.push(fence?.id ?? null);
    }
  }
  return ids;
}

function hasLocalPosixAlternative(lines, blockIds, index) {
  return lines.some((line, candidate) =>
    (Math.abs(candidate - index) <= 3
      || (blockIds[index] !== null && blockIds[index] === blockIds[candidate]))
    && POSIX_ALTERNATIVE.test(line));
}

export async function scan({ root, denylist, targets = DEFAULT_DIRS.map((dir) => path.join(root, dir)) }) {
  const hashes = parseDenylist(await readFile(denylist, 'utf8'));
  const findings = [];
  const excluded = path.resolve(denylist);
  const files = (await Promise.all(targets.map(collectFiles))).flat().sort();
  for (const file of new Set(files)) {
    if (path.resolve(file) === excluded) continue;
    const contents = await readFile(file, 'utf8');
    if (contents.includes('\0')) throw new Error(`Binary file in scanned tree: ${file}`);
    const lines = contents.split(/\r?\n/);
    const blockIds = fencedBlockIds(lines);
    for (const [index, line] of lines.entries()) {
      const relative = path.relative(root, file);
      const displayPath = relative.startsWith('..') || path.isAbsolute(relative) ? path.basename(file) : relative;
      const location = `${displayPath}:${index + 1}`;
      if (PLAIN_PATTERNS.some((pattern) => pattern.test(line))) {
        findings.push(`${location}: restricted plaintext pattern`);
      }
      if (/\.ps1\b/i.test(line) && !hasLocalPosixAlternative(lines, blockIds, index)) {
        findings.push(`${location}: PowerShell script without POSIX alternative`);
      }
      const match = findDeniedNgram(line, hashes);
      if (match) {
        findings.push(`${location}: denied n-gram at word ${match.start}, length ${match.length}`);
      }
    }
  }
  return findings;
}

function parseArgs(args) {
  const options = { root: path.resolve(fileURLToPath(new URL('..', import.meta.url))), targets: [] };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!['--root', '--denylist', '--scan'].includes(arg) || !args[index + 1]) {
      throw new Error('Usage: node scripts/content-gate.mjs [--root DIR] [--denylist FILE] [--scan FILE_OR_DIR ...]');
    }
    const value = args[++index];
    if (arg === '--root') options.root = path.resolve(value);
    else if (arg === '--denylist') options.denylist = path.resolve(value);
    else options.targets.push(path.resolve(value));
  }
  options.denylist ??= path.join(options.root, DENYLIST_RELATIVE);
  if (options.targets.length === 0) delete options.targets;
  return options;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const findings = await scan(parseArgs(process.argv.slice(2)));
    if (findings.length) {
      for (const finding of findings) console.error(finding);
      process.exitCode = 1;
    } else {
      console.log('Content gate passed');
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
