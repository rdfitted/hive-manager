<script lang="ts">
  import { Keyboard, X } from 'phosphor-svelte';

  interface Props {
    open: boolean;
    onClose: () => void;
  }

  let { open, onClose }: Props = $props();

  const GROUPS: Array<{ title: string; shortcuts: Array<{ keys: string[]; action: string }> }> = [
    {
      title: 'Layout',
      shortcuts: [
        { keys: ['Ctrl', 'B'], action: 'Toggle left sidebar' },
        { keys: ['Ctrl', 'J'], action: 'Toggle right panel' },
        { keys: ['↑', '↓'], action: 'Navigate agents' },
      ],
    },
    {
      title: 'Terminal',
      shortcuts: [
        { keys: ['Ctrl', 'F'], action: 'Find in terminal' },
        { keys: ['Ctrl', '='], action: 'Increase font size' },
        { keys: ['Ctrl', '-'], action: 'Decrease font size' },
        { keys: ['Ctrl', '0'], action: 'Reset font size' },
        { keys: ['Shift', 'Enter'], action: 'Newline without submitting' },
        { keys: ['Ctrl', 'C'], action: 'Copy selection (interrupt when nothing selected)' },
        { keys: ['Ctrl', 'V'], action: 'Paste' },
      ],
    },
    {
      title: 'Context',
      shortcuts: [
        { keys: ['Ctrl', 'I'], action: 'Capture selection / cell context for next turn' },
      ],
    },
    {
      title: 'Help',
      shortcuts: [
        { keys: ['Ctrl', '/'], action: 'Show this overlay' },
      ],
    },
  ];
</script>

{#if open}
  <div
    class="overlay"
    role="presentation"
    onclick={onClose}
    onkeydown={(e) => e.key === 'Escape' && onClose()}
  >
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      aria-label="Keyboard shortcuts"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => { e.stopPropagation(); if (e.key === 'Escape') onClose(); }}
    >
      <div class="dialog-header">
        <span class="dialog-icon"><Keyboard size={18} weight="light" /></span>
        <h2>Keyboard Shortcuts</h2>
        <button class="close-btn" onclick={onClose} title="Close (Esc)" aria-label="Close" type="button">
          <X size={16} weight="light" />
        </button>
      </div>
      <div class="dialog-body">
        {#each GROUPS as group (group.title)}
          <section class="group">
            <h3>{group.title}</h3>
            <ul>
              {#each group.shortcuts as shortcut (shortcut.action)}
                <li>
                  <span class="keys">
                    {#each shortcut.keys as key, i (key)}
                      {#if i > 0}<span class="plus">+</span>{/if}
                      <kbd>{key}</kbd>
                    {/each}
                  </span>
                  <span class="action">{shortcut.action}</span>
                </li>
              {/each}
            </ul>
          </section>
        {/each}
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg-void) 60%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
  }

  .dialog {
    width: 440px;
    max-width: 90vw;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-surface);
    border: 1px solid var(--border-structural);
    border-radius: var(--radius-sm);
    box-shadow: var(--shadow-lg);
  }

  .dialog-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px;
    border-bottom: 1px solid var(--border-structural);
  }

  .dialog-icon {
    display: flex;
    align-items: center;
    color: var(--accent-cyan);
  }

  .dialog-header h2 {
    flex: 1;
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .close-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    cursor: pointer;
  }

  .close-btn:hover {
    background: var(--bg-elevated);
    color: var(--text-primary);
  }

  .dialog-body {
    padding: 8px 16px 16px;
    overflow-y: auto;
  }

  .group h3 {
    margin: 14px 0 8px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .group ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .group li {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .keys {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    min-width: 120px;
  }

  kbd {
    padding: 2px 6px;
    background: var(--bg-void);
    border: 1px solid var(--border-structural);
    border-bottom-width: 2px;
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.4;
  }

  .plus {
    color: var(--text-secondary);
    font-size: 11px;
  }

  .action {
    font-size: 12px;
    color: var(--text-secondary);
  }
</style>
