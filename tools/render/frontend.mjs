import { spawnSync } from 'node:child_process';
import { cp, mkdir, rm, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { htmlProbeName, htmlProbeSource } from './html-probe.mjs';

// This hook is selected only by build.mjs's render-specific Tauri override.
// Keep the ordinary build directory free of the privileged test fixture.
const repo = fileURLToPath(new URL('../../', import.meta.url));
const result = spawnSync(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['run', 'build'], {
  cwd: repo, stdio: 'inherit', shell: process.platform === 'win32',
});
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);
const frontend = path.join(repo, 'tools/render/out/frontend');
await rm(frontend, { recursive: true, force: true });
await mkdir(frontend, { recursive: true });
await cp(path.join(repo, 'build'), frontend, { recursive: true });
await writeFile(path.join(frontend, htmlProbeName), htmlProbeSource);
console.log(`render:check packaged ${htmlProbeName} in disposable frontend`);
