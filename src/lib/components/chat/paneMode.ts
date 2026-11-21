export type PaneMode = 'chat' | 'split' | 'terminal' | 'raw';

export const paneStatusLineContext = Symbol('paneStatusLine');
/** True while the owning pane shows its terminal as a drawer under the transcript. */
export const paneSplitContext = Symbol('paneSplit');
