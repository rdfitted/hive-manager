import { writable, derived } from 'svelte/store';
import { events } from './events';
import type { Event } from '../types/domain';

interface ReplayState {
    currentTimestamp: string | null;
    playbackSpeed: number;
    isPlaying: boolean;
}

const initialReplayState: ReplayState = {
    currentTimestamp: null,
    playbackSpeed: 1,
    isPlaying: false,
};

function createReplayStore() {
    const { subscribe, set, update } = writable<ReplayState>(initialReplayState);

    let playbackInterval: any = null;
    let currentEvents: Event[] = [];

    function startInterval(speed: number) {
        if (playbackInterval) clearInterval(playbackInterval);
        playbackInterval = setInterval(() => {
            update(s => {
                const currentIndex = currentEvents.findIndex(e => e.timestamp === s.currentTimestamp);
                const nextIndex = currentIndex + 1;

                if (nextIndex >= currentEvents.length) {
                    if (playbackInterval) clearInterval(playbackInterval);
                    playbackInterval = null;
                    return { ...s, isPlaying: false };
                }

                return { ...s, currentTimestamp: currentEvents[nextIndex].timestamp };
            });
        }, 1000 / speed);
    }

    return {
        subscribe,
        set,
        update,
        play: (allEvents: Event[]) => {
            if (allEvents.length === 0) return;
            currentEvents = allEvents;

            update(state => {
                if (state.isPlaying) return state;

                startInterval(state.playbackSpeed);
                return { ...state, isPlaying: true };
            });
        },
        pause: () => {
            update(state => ({ ...state, isPlaying: false }));
            if (playbackInterval) {
                clearInterval(playbackInterval);
                playbackInterval = null;
            }
        },
        setTimestamp: (ts: string | null) => update(state => ({ ...state, currentTimestamp: ts })),
        setSpeed: (speed: number) => {
            update(state => {
                const newState = { ...state, playbackSpeed: speed };
                if (newState.isPlaying) {
                    startInterval(speed);
                }
                return newState;
            });
        },
        reset: () => {
            set(initialReplayState);
            currentEvents = [];
            if (playbackInterval) {
                clearInterval(playbackInterval);
                playbackInterval = null;
            }
        }
    };
}

export const replay = createReplayStore();

export const chronologicalEvents = derived(
    events,
    $events => [...$events.events].sort((a, b) => 
        new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime()
    )
);

export const eventsAtTimestamp = derived(
    [chronologicalEvents, replay],
    ([$chronologicalEvents, $replay]) => {
        if (!$replay.currentTimestamp) return $chronologicalEvents;
        const targetTs = new Date($replay.currentTimestamp).getTime();
        return $chronologicalEvents.filter(e => new Date(e.timestamp).getTime() <= targetTs);
    }
);
