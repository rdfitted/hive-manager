import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const fixtures = new URL('./fixtures/', import.meta.url);
export const manifest = JSON.parse(await readFile(new URL('manifest.json', fixtures), 'utf8'));

// Check source text and generated text after replacing the explicitly allowed instance root.
// Never include the rejected value in diagnostics (it could be a credential).
export function assertSynthetic(text, { root, userNames = [] } = {}) {
  // Decode JSON strings before checking paths so escaped separators are not mistaken for UNC roots.
  let decoded = text;
  try {
    const strings = [];
    const collect = value => {
      if (typeof value === 'string') strings.push(value);
      else if (Array.isArray(value)) value.forEach(collect);
      else if (value && typeof value === 'object') Object.entries(value).forEach(([key, item]) => { strings.push(key); collect(item); });
    };
    collect(JSON.parse(text));
    decoded = strings.join('\n');
  } catch { /* Markdown and journal fragments are plain text. */ }
  // Canonical containment paths may carry a verbatim prefix. Only this known root alias is allowed.
  const canonicalRoot = root && /^[a-z]:[\\/]/i.test(root) ? `\\\\?\\${root}` : root;
  let value = decoded;
  if (root) for (const alias of [...new Set([canonicalRoot, root])]) {
    const escaped = alias.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    value = value.replace(new RegExp(`${escaped}(?=[\\\\/\\r\\n]|$)`, 'g'), '<instance>');
  }
  const forbidden = [
    /\b[a-z]:[\\/]/i, /\/(?:Users|home|mnt|var|tmp)\//i, /\\\\[^\s\\]+\\/,
    /\b(?:sk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[A-Z0-9]{16})\b/,
    /\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b/,
    ...userNames.filter(name => name && name.length >= 3).map(name => new RegExp(`\\b${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`, 'i')),
  ];
  if (forbidden.some(pattern => pattern.test(value))) throw new Error('Fixture contains a forbidden path, user name, or credential shape');
}

/** Seed only a fresh, absolute instance directory. No app or agent is started. */
export async function seed(root, now = new Date()) {
  if (!path.isAbsolute(root) || path.resolve(root) !== root || path.parse(root).root === root) {
    throw new Error('Fixture root must be an absolute normalized directory below a filesystem root');
  }
  const instant = new Date(now);
  if (!Number.isFinite(instant.getTime())) throw new Error('Invalid seed time');
  const at = seconds => new Date(instant.getTime() - seconds * 1000).toISOString();
  await mkdir(root, { recursive: true });
  if ((await readdir(root)).length) throw new Error('Fixture root must be empty');
  const dataDir = path.join(root, process.platform === 'win32' ? 'appdata/hive-manager' : 'home/.config/hive-manager');
  const userNames = [process.env.USERNAME, process.env.USER];
  const chat = await readFile(new URL(manifest.chat.file, fixtures), 'utf8');
  assertSynthetic(JSON.stringify(manifest), { userNames });
  assertSynthetic(chat, { userNames });
  const write = async (relative, content) => {
    const target = path.join(root, relative);
    if (!target.startsWith(root + path.sep)) throw new Error('Fixture output escapes root');
    assertSynthetic(content, { root, userNames });
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, content, 'utf8');
  };
  const json = (relative, value) => write(relative, JSON.stringify(value, null, 2) + '\n');
  await write('.hive-instance', 'Synthetic render fixture\n');
  for (const folder of ['home/.codex', 'home/.claude', 'temp', 'webview', 'localappdata', 'appdata']) {
    await mkdir(path.join(root, folder), { recursive: true });
  }
  await write('home/.gitconfig', '');
  await write('home/.gitconfig-system', '');
  // Explicit identity/config/date avoids reading or copying the operator's Git settings.
  const gitEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^GIT_/i.test(key)));
  Object.assign(gitEnv, {
    GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: path.join(root, 'home/.gitconfig'),
    GIT_CONFIG_SYSTEM: path.join(root, 'home/.gitconfig-system'),
    GIT_AUTHOR_NAME: 'Synthetic Fixture', GIT_COMMITTER_NAME: 'Synthetic Fixture',
    GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_EMAIL: 'fixture@example.invalid',
    GIT_AUTHOR_DATE: instant.toISOString(), GIT_COMMITTER_DATE: instant.toISOString(),
    HOME: path.join(root, 'home'), USERPROFILE: path.join(root, 'home'),
  });
  const gitAt = (cwd, args) => execFileSync('git', ['-c', 'core.hooksPath=disabled-hooks', ...args], { cwd, env: gitEnv, encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
  // The app's cwd is the instance root; Recent is scoped to that exact project.
  // Give the completed fixture a real synthetic base repo while its worktree stays nested.
  gitAt(root, ['init', '--initial-branch=main', '--template=']);
  await write('.gitignore', '*\n!.gitignore\n!synthetic-base.txt\n');
  await write('synthetic-base.txt', 'Synthetic render base project\n');
  gitAt(root, ['add', '--', '.gitignore', 'synthetic-base.txt']);
  gitAt(root, ['-c', 'commit.gpgsign=false', 'commit', '-m', 'Synthetic base project']);
  const overlay = { version: 1, sessions: [] };
  for (const session of manifest.sessions) {
    const project = path.join(root, 'projects', session.project);
    const projectPath = session.projectScope === 'instance-root' ? root : project;
    await mkdir(project, { recursive: true });
    const git = args => execFileSync('git', ['-c', 'core.hooksPath=disabled-hooks', ...args], { cwd: project, env: gitEnv, encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
    git(['init', '--initial-branch=main', '--template=']);
    await write(`projects/${session.project}/navigation.txt`, 'Synthetic navigation\nCourse: amber\n');
    git(['add', '--', 'navigation.txt']);
    git(['-c', 'commit.gpgsign=false', 'commit', '-m', 'Synthetic navigation baseline']);
    const base = git(['rev-parse', 'HEAD']);
    await write(`projects/${session.project}/navigation.txt`, 'Synthetic navigation\nCourse: violet\nReview: ready\n');
    const relative = path.relative(root, path.join(dataDir, 'sessions', session.id));
    const agents = session.agents.map(agent => ({
      id: agent.id, role: agent.role, parent_id: agent.role === 'Queen' ? null : session.agents[0].id,
      config: { cli: 'codex', flags: [], label: agent.label, name: agent.label, initial_prompt: 'Inspect the synthetic orbital report.' },
      transcript_cwd: project, transcript_spawned_at: at(manifest.timing.createdSeconds),
      transcript_session_id: null, base_commit_sha: base,
    }));
    await json(`${relative}/session.json`, {
      id: session.id, name: session.name, session_type: { Hive: { worker_count: agents.length - 1 } },
      project_path: projectPath, worktree_path: project, worktree_branch: 'main',
      created_at: at(manifest.timing.createdSeconds), last_activity_at: at(manifest.timing.recentActivitySeconds),
      agents, state: session.state, default_cli: 'codex', auth_strategy: 'none',
      execution_policy: { workspace_strategy: 'shared_cell' }, qa_workers: [],
    });
    await json(`${relative}/retirement.json`, { enabled: false, on_call_agent_id: null });
    // The UI polls the typed runtime graph even when no scheduling nodes are needed.
    await json(`${relative}/state/work-graph.json`, { nodes: [], edges: [], omissions: [] });
    const projected = [];
    for (const [index, agent] of session.agents.entries()) {
      const old = ['finished', 'stalled'].includes(agent.state);
      const activity = at(old ? manifest.timing.oldActivitySeconds : manifest.timing.recentActivitySeconds);
      const status = agent.state === 'finished' ? 'Completed'
        : agent.state === 'idle' ? 'Idle'
        : agent.state === 'exited_during_startup' ? { Error: 'exited_during_startup:17' }
        : agent.state === 'delivery_failed' ? { WaitingForInput: `injected_delivery_failed:synthetic:${activity}` } : 'Running';
      projected.push({ agent_id: agent.id, status, work_state: agent.state === 'finished' ? 'finished' : 'working',
        ...(['working', 'markdown', 'idle'].includes(agent.state) ? { refresh_heartbeat: true } : {}),
        heartbeat: { status: agent.state === 'finished' ? 'completed' : agent.state === 'idle' ? 'idle' : 'working', summary: agent.label, last_activity: activity } });
      const generation = `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`;
      const rows = [
        ['user_message', 'Please inspect this synthetic navigation report.'],
        ['assistant_message', agent.state === 'markdown' ? chat : `Synthetic report for ${agent.label}. State: ${agent.state}.`],
      ];
      let journal = '';
      for (const [rowIndex, [kind, text]] of rows.entries()) {
        const cursor = Buffer.from(JSON.stringify({ generation, byte_offset: Buffer.byteLength(journal) })).toString('base64url');
        journal += JSON.stringify({ agent_id: agent.id, cursor, ts: activity, kind, source_id: `synthetic-${rowIndex}`, truncated: false, payload: { text } }) + '\n';
      }
      await write(`${relative}/agent-feeds/${agent.id}.jsonl`, journal);
      await write(`${relative}/agent-feeds/${agent.id}.source`, 'protocol');
    }
    overlay.sessions.push({ session_id: session.id, agents: projected });
  }
  await json('fixture-state.json', overlay);
  return { root, dataDir, manifest, seededAt: instant.toISOString() };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = process.argv[2];
  if (!root) throw new Error('Usage: node tools/render/seed.mjs <absolute-empty-root> [seed-time]');
  await seed(root, process.argv[3] ?? new Date());
}
