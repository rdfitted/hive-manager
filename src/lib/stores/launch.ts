import { writable } from 'svelte/store';
import {
  sessions,
  type DebateLaunchConfig,
  type FusionLaunchConfig,
  type HiveLaunchConfig,
  type ResearchLaunchConfig,
  type SoloLaunchConfig,
} from './sessions';

export type LaunchMode = 'templates' | 'hive' | 'fusion' | 'solo' | 'research' | 'debate';

export interface LaunchState {
  requestId: number;
  requestedMode: LaunchMode | undefined;
  landingOpen: boolean;
  launching: boolean;
  launchError: string;
  defaultProjectPath: string;
}

// Request/lifecycle state only: the single LaunchDialog owns every form field.
export function createLaunchStore() {
  const { subscribe, update } = writable<LaunchState>({
    requestId: 0,
    requestedMode: undefined,
    landingOpen: false,
    launching: false,
    launchError: '',
    defaultProjectPath: '',
  });

  async function runLaunch(action: () => Promise<unknown>) {
    update((state) => ({ ...state, launching: true, launchError: '' }));
    try {
      await action();
      update((state) => ({ ...state, landingOpen: false }));
    } catch (err) {
      update((state) => ({ ...state, launchError: String(err) }));
      console.error('Launch failed:', err);
    } finally {
      update((state) => ({ ...state, launching: false }));
    }
  }

  return {
    subscribe,
    open(mode?: LaunchMode) {
      update((state) => ({
        ...state,
        requestId: state.requestId + 1,
        requestedMode: mode,
        landingOpen: true,
        launchError: '',
      }));
    },
    close() {
      update((state) => ({ ...state, landingOpen: false, launchError: '' }));
    },
    clearError() {
      update((state) => ({ ...state, launchError: '' }));
    },
    setDefaultProjectPath(path: string) {
      update((state) => ({ ...state, defaultProjectPath: path }));
    },
    launchHive: (config: HiveLaunchConfig) => runLaunch(() => sessions.launchHiveV2(config)),
    launchResearch: (config: ResearchLaunchConfig) => runLaunch(() => sessions.launchResearch(config)),
    launchFusion: (config: FusionLaunchConfig) => runLaunch(() => sessions.launchFusion(config)),
    launchSolo: (config: SoloLaunchConfig) => runLaunch(() => sessions.launchSolo(config)),
    launchDebate: (config: DebateLaunchConfig) => runLaunch(() => sessions.launchDebate(config)),
  };
}

export const launch = createLaunchStore();
