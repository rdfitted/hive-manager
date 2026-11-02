import { appendFile, mkdir, open, readFile, unlink } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { identity, sameProcess, sleep } from './windows.mjs';
export function pendingRequests(text) {
  const pending = [];
  for (const line of text.split(/\r?\n/)) {
    const match = /^(REQUEST|RELEASE) (\S+) (\S+)/.exec(line);
    if (!match) continue;
    const [, kind, lane, stamp] = match;
    if (kind === 'REQUEST') pending.push({ lane, stamp });
    else {
      // Existing session releases sometimes carry release-time rather than request-time.
      let index = pending.findIndex(item => item.lane === lane && item.stamp === stamp);
      if (index < 0) index = pending.findIndex(item => item.lane === lane);
      if (index >= 0) pending.splice(index, 1);
    }
  }
  return pending;
}
export async function acquireGate(repo, pool, signal) {
  const session = process.env.HIVE_SESSION_ID ?? /[\\/]worktrees[\\/]([a-f0-9-]{36})[\\/]/i.exec(repo)?.[1];
  const common = execFileSync('git', ['rev-parse', '--git-common-dir'], { cwd: repo, windowsHide: true, encoding: 'utf8' }).trim();
  const queue = session && path.join(path.dirname(path.resolve(repo, common)), '.hive-manager', session, 'gate-queue.log');
  if (queue && await readFile(queue, 'utf8').then(() => true, () => false)) {
    const lane = 'lane-c', stamp = new Date().toISOString();
    await appendFile(queue, `REQUEST ${lane} ${stamp}\n`);
    const release = () => appendFile(queue, `RELEASE ${lane} ${stamp}\n`);
    try {
      while (true) {
        if (signal.aborted) throw new Error('Render check interrupted while waiting for FIFO');
        const first = pendingRequests(await readFile(queue, 'utf8'))[0];
        if (first?.lane === lane && first.stamp === stamp) return { kind: 'fifo', path: queue, lane, stamp, release };
        console.log('render:check waiting for the FIFO gate');
        await sleep(60000);
      }
    } catch (error) { await release(); throw error; }
  }
  await mkdir(pool, { recursive: true });
  const lock = path.join(pool, 'render-check.lock');
  const owner = identity(process.pid);
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const handle = await open(lock, 'wx');
      await handle.writeFile(JSON.stringify({ marker: 'hm-render-lock-v1', owner }));
      await handle.close();
      return { kind: 'local', path: lock, owner, release: () => unlink(lock) };
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
      const previous = JSON.parse(await readFile(lock, 'utf8'));
      if (previous.marker !== 'hm-render-lock-v1' || sameProcess(previous.owner, identity(previous.owner.pid))) throw new Error('Another render check owns the local gate');
      await unlink(lock);
    }
  }
  throw new Error('Could not acquire render gate');
}
