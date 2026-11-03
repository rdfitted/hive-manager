import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import path from 'node:path';

// Snapshot working bytes, including existing edits, rather than index contents.
// No exclusions: this includes every tracked file below tools/render too.
export async function trackedSnapshot(repo) {
  const names = execFileSync('git', ['ls-files', '-z'], { cwd: repo, windowsHide: true }).toString('utf8').split('\0').filter(Boolean);
  return new Map(await Promise.all(names.map(async name => {
    try { return [name, createHash('sha256').update(await readFile(path.join(repo, name))).digest('hex')]; }
    catch (error) { if (error.code === 'ENOENT') return [name, null]; throw error; }
  })));
}

export async function trackedChanges(repo, before) {
  const after = await trackedSnapshot(repo);
  return [...new Set([...before.keys(), ...after.keys()])].filter(name => !before.has(name) || !after.has(name) || before.get(name) !== after.get(name));
}
