/**
 * Slash-command registry for the Composer (#128).
 *
 * Each command has a `label` (the `/token` shown), a `description`, and an `expand()` that
 * returns the text inserted into the flattened plain-text prompt. The MVP set is locked to
 * the three real `SessionMode` values (hive / fusion / research) plus a few quick actions;
 * expanding the set later is purely additive here.
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
    name: 'hive',
    label: '/hive',
    description: 'Hive orchestration mode',
    action: 'insert',
    expand: () => '/hive ',
  },
  {
    name: 'fusion',
    label: '/fusion',
    description: 'Fusion (multi-variant) mode',
    action: 'insert',
    expand: () => '/fusion ',
  },
  {
    name: 'research',
    label: '/research',
    description: 'Research mode',
    action: 'insert',
    expand: () => '/research ',
  },
  {
    name: 'ask',
    label: '/ask',
    description: 'Ask without making changes',
    action: 'insert',
    expand: () => '/ask ',
  },
  {
    name: 'plan',
    label: '/plan',
    description: 'Produce a plan, do not implement',
    action: 'insert',
    expand: () => '/plan ',
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
