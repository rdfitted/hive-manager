/**
 * Slash-command registry for the Composer (#128).
 *
 * Each command has a `label` (the `/token` shown), a `description`, and an `expand()` that
 * returns CLI-independent text inserted into the flattened plain-text prompt. Session modes
 * are launched through the sidebar dialog, not sent to the attached CLI as slash text.
 *
 * `clear` and `attach` are control commands: they expand to an empty string (the Composer
 * intercepts them by `action` rather than inserting text).
 */

export type SlashCommandAction = 'insert' | 'clear' | 'attach';

export interface SlashCommand {
  /** The token after the leading slash, e.g. "research". */
  name: string;
  /** Human label including the leading slash, e.g. "/research". */
  label: string;
  description: string;
  action: SlashCommandAction;
  /** Text inserted into the flattened prompt when the command is selected. */
  expand: () => string;
}

export const SLASH_COMMANDS: SlashCommand[] = [
  {
    name: 'ask',
    label: '/ask',
    description: 'Ask without making changes',
    action: 'insert',
    expand: () => 'Answer without modifying any files: ',
  },
  {
    name: 'plan',
    label: '/plan',
    description: 'Produce a plan, do not implement',
    action: 'insert',
    expand: () => 'Produce a plan only; do not modify files: ',
  },
  {
    name: 'clear',
    label: '/clear',
    description: 'Clear the composer',
    action: 'clear',
    expand: () => '',
  },
  {
    name: 'attach',
    label: '/attach',
    description: 'Attach a file or selection',
    action: 'attach',
    expand: () => '',
  },
];

/**
 * Filter the command set by a typed prefix (the text after the leading slash). An empty
 * query returns every command. Matching is case-insensitive against the command name.
 */
export function filterCommands(query: string): SlashCommand[] {
  const q = query.trim().toLowerCase();
  if (q === '') return SLASH_COMMANDS.slice();
  return SLASH_COMMANDS.filter((c) => c.name.toLowerCase().startsWith(q));
}

/** Look up a single command by exact name (no leading slash). */
export function findCommand(name: string): SlashCommand | undefined {
  return SLASH_COMMANDS.find((c) => c.name.toLowerCase() === name.toLowerCase());
}
