// @vitest-environment jsdom
import { fireEvent, render, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

import WikiSettings from './WikiSettings.svelte';

describe('WikiSettings', () => {
  beforeEach(() => invoke.mockReset());

  it.each([
    { original: null, mode: 'all', expected: null },
    { original: [], mode: 'none', expected: [] },
    { original: ['patterns'], mode: 'list', expected: ['patterns', 'research'] },
  ])('round-trips $mode without changing null and empty-list meanings', async ({ original, mode, expected }) => {
    const config = {
      global_wiki_path: '~/wiki',
      knowledge_wiki_folders: original,
      unrelated_option: 'preserved',
    };
    invoke.mockImplementation(async (command: string) => command === 'get_app_config' ? config : undefined);
    const onSaved = vi.fn();
    const view = render(WikiSettings, { onSaved });

    await waitFor(() => expect((view.getByLabelText('Wiki root') as HTMLInputElement).value).toBe('~/wiki'));
    expect((view.getByLabelText('Atlas folders') as HTMLSelectElement).value).toBe(mode);
    if (mode === 'list') {
      await fireEvent.input(view.getByLabelText('Allowed folders, separated by commas'), {
        target: { value: 'patterns, research' },
      });
    }
    await fireEvent.click(view.getByRole('button', { name: 'Save wiki settings' }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('update_app_config', {
      config: { ...config, knowledge_wiki_folders: expected },
    }));
    expect(onSaved).toHaveBeenCalledOnce();
  });
});
