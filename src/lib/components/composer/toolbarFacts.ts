import type { AgentInfo } from '$lib/stores/sessions';

export interface ToolbarFacts {
  accessLabel?: string;
  model?: string;
  effort?: string;
  placeholder: string;
}

/** Display only launch facts we know; protocol seats do not inherit PTY bypass flags. */
export function toolbarFacts(
  agent: AgentInfo | undefined,
  eventSource: 'none' | 'transcript' | 'protocol' | undefined,
): ToolbarFacts {
  const config = agent?.config;
  const label = config?.label?.trim();
  const facts: ToolbarFacts = { placeholder: label ? `Message ${label}` : 'Message this agent…' };
  if (!config) return facts;
  if ((config.cli === 'claude' || config.cli === 'codex') && eventSource !== 'protocol') {
    facts.accessLabel = 'Full access';
  }
  if (config.model?.trim()) facts.model = config.model.trim();
  // Claude's effort syntax is not verified. Codex uses a standalone -c assignment.
  if (config.cli === 'codex') {
    for (const flag of config.flags) {
      const match = /^model_reasoning_effort\s*=\s*"([a-z]+)"$/.exec(flag.trim());
      if (match) facts.effort = match[1];
    }
  }
  return facts;
}
