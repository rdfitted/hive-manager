<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  interface AppConfig {
    global_wiki_path: string | null;
    knowledge_wiki_folders: string[] | null;
    [key: string]: unknown;
  }

  let { onSaved }: { onSaved?: () => void } = $props();
  let root = $state('');
  let folderMode = $state<'all' | 'none' | 'list'>('all');
  let folderList = $state('');
  let loading = $state(true);
  let saving = $state(false);
  let error = $state('');
  let saved = $state(false);

  onMount(() => {
    void load();
  });

  async function load() {
    try {
      const config = await invoke<AppConfig>('get_app_config');
      root = config.global_wiki_path ?? '';
      const folders = config.knowledge_wiki_folders;
      folderMode = folders === null ? 'all' : folders.length === 0 ? 'none' : 'list';
      folderList = folders?.join(', ') ?? '';
      error = '';
    } catch (cause) {
      error = `Could not load wiki settings: ${String(cause)}`;
    } finally {
      loading = false;
    }
  }

  async function save() {
    saving = true;
    saved = false;
    try {
      const config = await invoke<AppConfig>('get_app_config');
      const folders = folderMode === 'all'
        ? null
        : folderMode === 'none'
          ? []
          : folderList.split(',').map((name) => name.trim()).filter(Boolean);
      await invoke('update_app_config', {
        config: {
          ...config,
          global_wiki_path: root.trim() || null,
          knowledge_wiki_folders: folders,
        },
      });
      saved = true;
      error = '';
      onSaved?.();
    } catch (cause) {
      error = `Could not save wiki settings: ${String(cause)}`;
    } finally {
      saving = false;
    }
  }
</script>

<form class="wiki-settings atlas-surface lattice-forced-colors-boundary" onsubmit={(event) => { event.preventDefault(); void save(); }}>
  <label for="wiki-root">Wiki root</label>
  <input id="wiki-root" class="lattice-input" bind:value={root} disabled={loading || saving} placeholder="~/.ai-docs/wiki" />
  <p>Leave blank for the default. HIVE_WIKI_ROOT overrides this saved path for the current process.</p>

  <label for="wiki-folder-mode">Atlas folders</label>
  <select id="wiki-folder-mode" class="lattice-input" bind:value={folderMode} disabled={loading || saving}>
    <option value="all">All folders (default)</option>
    <option value="none">No folders</option>
    <option value="list">Only listed folders</option>
  </select>
  {#if folderMode === 'list'}
    <label for="wiki-folder-list">Allowed folders, separated by commas</label>
    <input id="wiki-folder-list" class="lattice-input" bind:value={folderList} disabled={loading || saving} placeholder="patterns, research" />
  {/if}
  <p>All folders stores null; No folders stores an empty list. Use a list when the wiki has private folders.</p>

  <button class="lattice-btn lattice-btn--primary" type="submit" disabled={loading || saving}>{saving ? 'Saving…' : 'Save wiki settings'}</button>
  {#if saved}<span role="status">Wiki settings saved.</span>{/if}
  {#if error}<span role="alert">{error}</span>{/if}
</form>

<style>
  .wiki-settings { display: grid; gap: 0.6rem; padding: 1rem; border-radius: var(--lattice-radius-md); }
  label { font-weight: 600; }
  p { margin: 0 0 0.35rem; color: var(--lattice-text-muted); font-size: 0.82rem; }
  input, select { width: 100%; }
  button { justify-self: start; }
</style>
