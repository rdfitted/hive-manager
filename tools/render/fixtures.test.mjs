import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { seed, manifest, assertSynthetic } from './seed.mjs';

test('fixture source contains only synthetic public data', async () => {
  for (const name of await readdir(new URL('./fixtures/', import.meta.url))) {
    assertSynthetic(await readFile(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'), {
      userNames: [process.env.USERNAME, process.env.USER],
    });
  }
});

test('denylist rejects path, user name, and credential mutations without exposing values', () => {
  for (const planted of [String.raw`Z:\Users\sample-person\private`, String.raw`\\synthetic-server\private`, '/home/sample-person/private',
    'sample-person', 'sk-' + 'a'.repeat(32), 'ghp_' + 'b'.repeat(36), 'AKIA' + 'A'.repeat(16)]) {
    assert.throws(() => assertSynthetic(planted, { userNames: ['sample-person'] }), /forbidden/);
    assert.throws(() => assertSynthetic(JSON.stringify({ value: planted }), { userNames: ['sample-person'] }), /forbidden/);
  }
  assertSynthetic('Synthetic Fixture https://example.com/synthetic-orbit');
  const ownRoot = String.raw`Z:\synthetic-fixture`;
  assertSynthetic(JSON.stringify({ path: `${ownRoot}\\project` }), { root: ownRoot });
  assertSynthetic(JSON.stringify({ path: `\\\\?\\${ownRoot}\\project` }), { root: ownRoot });
  for (const other of [`${ownRoot}-other\\private`, `\\\\?\\${ownRoot}-other\\private`, String.raw`\\?\Y:\other\private`]) assert.throws(() => assertSynthetic(JSON.stringify({ path: other }), { root: ownRoot }), /forbidden/);
});

test('seeded schema, feeds, review repo, and relative timestamps cover every manifest state', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'hm-fixture-'));
  try {
    const now = '2030-06-15T12:00:00.000Z';
    const result = await seed(root, now);
    assert.equal(result.seededAt, now);
    assert.equal(result.manifest, manifest);
    assert.equal(manifest.screens.grid.paneCount, 8);
    assert.equal(manifest.sessions[0].agents.length, 8);
    assert.deepEqual(Object.keys(manifest.screens).sort(), ['dialog', 'focus', 'grid', 'kanban', 'launch', 'review', 'sidebar']);
    const overlayText = await readFile(path.join(root, 'fixture-state.json'), 'utf8');
    assertSynthetic(overlayText, { root, userNames: [process.env.USERNAME, process.env.USER] });
    const overlay = JSON.parse(overlayText);
    for (const session of manifest.sessions) {
      const directory = path.join(result.dataDir, 'sessions', session.id);
      const persistedText = await readFile(path.join(directory, 'session.json'), 'utf8');
      assertSynthetic(persistedText, { root, userNames: [process.env.USERNAME, process.env.USER] });
      const persisted = JSON.parse(persistedText);
      assert.equal(persisted.id, session.id);
      assert.equal(persisted.state, session.state);
      assert.equal(persisted.created_at, new Date(Date.parse(now) - manifest.timing.createdSeconds * 1000).toISOString());
      const expectedProject = session.projectScope === 'instance-root' ? root : path.join(root, 'projects', session.project);
      assert.equal(persisted.project_path, expectedProject);
      assert.equal(persisted.project_path.startsWith('\\\\?\\'), false);
      assert.equal(persisted.worktree_path, path.join(root, 'projects', session.project));
      assert.deepEqual(JSON.parse(await readFile(path.join(directory, 'retirement.json'), 'utf8')), { enabled: false, on_call_agent_id: null });
      assert.deepEqual(JSON.parse(await readFile(path.join(directory, 'state/work-graph.json'), 'utf8')), { nodes: [], edges: [], omissions: [] });
      const gitEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^GIT_/i.test(key)));
      Object.assign(gitEnv, { GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: path.join(root, 'home/.gitconfig'), GIT_CONFIG_SYSTEM: path.join(root, 'home/.gitconfig-system'), HOME: path.join(root, 'home'), USERPROFILE: path.join(root, 'home') });
      const diff = execFileSync('git', ['diff', '--', 'navigation.txt'], { cwd: persisted.worktree_path, env: gitEnv, encoding: 'utf8', windowsHide: true });
      assert.match(diff, /\+Course: violet/);
      assert.equal(execFileSync('git', ['status', '--porcelain'], { cwd: root, env: gitEnv, encoding: 'utf8', windowsHide: true }).trim(), '');
      for (const agent of session.agents) {
        const stored = persisted.agents.find(item => item.id === agent.id);
        assert.ok(stored.transcript_cwd.startsWith(root + path.sep));
        assert.equal(stored.base_commit_sha.length, 40);
        const projected = overlay.sessions.find(item => item.session_id === session.id).agents.find(item => item.agent_id === agent.id);
        if (['working', 'markdown', 'idle'].includes(agent.state)) {
          assert.equal(projected.refresh_heartbeat, true, `${agent.id} explicitly refreshes its heartbeat`);
        } else {
          assert.equal(Object.hasOwn(projected, 'refresh_heartbeat'), false, `${agent.id} preserves its seeded heartbeat`);
        }
        const age = (Date.parse(now) - Date.parse(projected.heartbeat.last_activity)) / 1000;
        assert.equal(age, ['finished', 'stalled'].includes(agent.state) ? manifest.timing.oldActivitySeconds : manifest.timing.recentActivitySeconds);
        if (agent.state === 'finished') {
          assert.equal(projected.work_state, 'finished');
          assert.equal(projected.heartbeat.status, 'completed');
          assert.ok(age > 180);
        } else if (agent.state === 'stalled') {
          assert.equal(projected.status, 'Running');
          assert.equal(projected.work_state, 'working');
          assert.ok(age > 180);
        } else if (agent.state === 'exited_during_startup') assert.deepEqual(projected.status, { Error: 'exited_during_startup:17' });
        else if (agent.state === 'delivery_failed') assert.match(projected.status.WaitingForInput, /^injected_delivery_failed:/);
        const feed = path.join(directory, 'agent-feeds', agent.id);
        assert.equal(await readFile(feed + '.source', 'utf8'), 'protocol');
        const text = await readFile(feed + '.jsonl', 'utf8');
        assertSynthetic(text, { root });
        let offset = 0;
        for (const line of text.trimEnd().split('\n')) {
          const event = JSON.parse(line);
          assert.equal(event.agent_id, agent.id);
          assert.equal(event.ts, projected.heartbeat.last_activity);
          assert.equal(JSON.parse(Buffer.from(event.cursor, 'base64url')).byte_offset, offset);
          offset += Buffer.byteLength(line + '\n');
          if (agent.state === 'markdown' && event.kind === 'assistant_message') {
            for (const snippet of [manifest.chat.tableText, manifest.chat.codeText, manifest.chat.link, manifest.chat.literalHtml, '| --- |', '```javascript']) assert.ok(event.payload.text.includes(snippet));
            assert.ok(event.payload.text.length > 3000);
          }
        }
      }
    }
    const conversation = manifest.conversation;
    const storedConversation = await readFile(path.join(result.dataDir, 'sessions', conversation.sessionId, 'conversations', `${conversation.agentId}.md`), 'utf8');
    const metadata = JSON.parse(storedConversation.split('\n')[2].match(/^<!-- hive-envelope: (.*) -->$/)[1]);
    assert.equal(metadata.renderer, 'diff');
    assert.deepEqual(metadata.data, conversation.data);
    assert.equal(metadata.body_bytes, Buffer.byteLength(conversation.data.unified, 'utf8'));
    assert.equal(storedConversation.split('\n').slice(3).join('\n').trimEnd(), conversation.data.unified);
    await assert.rejects(seed(root, now), /must be empty/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('seeding rejects ambiguous roots and seed times', async () => {
  await assert.rejects(seed('relative-root'), /absolute normalized/);
  await assert.rejects(seed(path.parse(os.tmpdir()).root), /absolute normalized/);
  await assert.rejects(seed(path.join(os.tmpdir(), 'unused-fixture'), 'invalid-time'), /Invalid seed time/);
});
