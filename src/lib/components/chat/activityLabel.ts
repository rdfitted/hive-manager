const labels: Record<string, string> = {
  Bash: 'Running bash',
  shell_command: 'Running bash',
  exec_command: 'Running bash',
  write_stdin: 'Running bash',
  Read: 'Reading a file',
  Edit: 'Editing a file',
  Write: 'Editing a file',
  apply_patch: 'Editing a file',
  Grep: 'Searching',
  Glob: 'Searching',
  web__run: 'Searching the web',
  view_image: 'Viewing an image',
  Task: 'Running a sub-agent',
  spawn_agent: 'Running a sub-agent',
  followup_task: 'Running a sub-agent',
  wait_agent: 'Running a sub-agent',
  send_message: 'Running a sub-agent',
  interrupt_agent: 'Running a sub-agent',
  list_agents: 'Running a sub-agent',
  sleep: 'Waiting',
  wait: 'Waiting',
  request_user_input_async: 'Waiting for your answer',
};

/** Describe the first tool inside Codex exec without displaying its input. */
export function activityLabel(name: string, input?: unknown): string {
  if (name === 'exec') {
    const inner = typeof input === 'string' ? /\btools\.([A-Za-z_$][\w$]*)/.exec(input)?.[1] : undefined;
    if (!inner) return 'Running code';
    name = inner;
  }
  return Object.hasOwn(labels, name) ? labels[name] : `Running ${name}`;
}
