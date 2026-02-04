import { writable } from 'svelte/store';

export interface Settings {
  defaultWorkerCount: number;
  defaultModel: string;
  theme: 'dark' | 'light';
  fontSize: number;
  fontFamily: string;
}

const defaultSettings: Settings = {
  defaultWorkerCount: 2,
  defaultModel: 'sonnet',
  theme: 'dark',
  fontSize: 14,
  fontFamily: 'Cascadia Code, Consolas, monospace',
};

function createSettingsStore() {
  // Load from localStorage if available
  const stored = typeof localStorage !== 'undefined'
    ? localStorage.getItem('hive-manager-settings')
    : null;
  const initial = stored ? { ...defaultSettings, ...JSON.parse(stored) } : defaultSettings;

  const { subscribe, set, update } = writable<Settings>(initial);

  return {
    subscribe,
    update(partial: Partial<Settings>) {
      update((settings) => {
        const updated = { ...settings, ...partial };
        if (typeof localStorage !== 'undefined') {
          localStorage.setItem('hive-manager-settings', JSON.stringify(updated));
        }
        return updated;
      });
    },
    reset() {
      set(defaultSettings);
      if (typeof localStorage !== 'undefined') {
        localStorage.removeItem('hive-manager-settings');
      }
    },
  };
}

export const settings = createSettingsStore();
