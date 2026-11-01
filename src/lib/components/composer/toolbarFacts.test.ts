import { describe, expect, it } from 'vitest';
import type { AgentInfo } from '$lib/stores/sessions';
import { toolbarFacts } from './toolbarFacts';

function agent(cli: string, overrides = {}): AgentInfo {
  return { id: 'fixture', role: 'Queen', status: 'Running', parent_id: null,
    config: { cli, flags: [], ...overrides } };
}

describe('toolbarFacts truth rules', () => {
  it.each(['claude', 'codex'])('derives full access only for %s non-protocol sources', (cli) => {
    for (const source of ['none', 'transcript', undefined] as const) {
      expect(toolbarFacts(agent(cli), source).accessLabel).toBe('Full access');
    }
    expect(toolbarFacts(agent(cli), 'protocol').accessLabel).toBeUndefined();
    expect(toolbarFacts(agent('cursor'), 'transcript').accessLabel).toBeUndefined();
  });
  it('omits unknown and empty values and names the placeholder from the config label', () => {
    expect(toolbarFacts(undefined, undefined)).toEqual({ placeholder: 'Message this agent…' });
    expect(toolbarFacts(agent('cursor', { model: ' ', label: ' ' }), 'none'))
      .toEqual({ placeholder: 'Message this agent…' });
    expect(toolbarFacts(agent('codex', { model: 'gpt-fixture', label: 'Builder' }), 'transcript'))
      .toEqual({ accessLabel: 'Full access', model: 'gpt-fixture', placeholder: 'Message Builder' });
  });
  it('extracts exact Codex effort assignments, never Claude effort or a substring of another setting', () => {
    const flags = ['-c', 'model_reasoning_effort="xhigh"'];
    expect(toolbarFacts(agent('codex', { flags }), 'transcript').effort).toBe('xhigh');
    expect(toolbarFacts(agent('claude', { flags }), 'transcript').effort).toBeUndefined();
    expect(toolbarFacts(agent('codex', { flags: ['other_model_reasoning_effort="high"'] }), 'none').effort).toBeUndefined();
    expect(toolbarFacts(agent('codex', { flags: ['model_reasoning_effort="low"', '-c', 'model_reasoning_effort="high"'] }), 'protocol').effort).toBe('high');
  });
});
