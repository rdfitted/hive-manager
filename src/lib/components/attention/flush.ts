import { apiUrl } from '$lib/config';

/** Bare Enter uses HTTP because the Tauri operator_inject command drops receipts. */
export async function flushEnter(sessionId: string, agentId: string): Promise<string> {
  const response = await fetch(apiUrl(`/api/sessions/${encodeURIComponent(sessionId)}/inject`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ target_agent_id: agentId, message: '', submit: true }),
  });
  const body = await response.text();
  if (!response.ok) throw new Error(`Flush failed (${response.status})${body ? `: ${body}` : ''}`);
  return body;
}
