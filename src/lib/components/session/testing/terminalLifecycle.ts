export interface TerminalProps {
  agentId: string;
  isFocused: boolean;
  isVisible: boolean;
  onReady?: () => void;
}

export const terminalLifecycle = {
  created: [] as string[],
  destroyed: [] as string[],
  terminals: new Map<string, { props: TerminalProps; node: HTMLElement }>(),
  trace: [] as string[],
};
