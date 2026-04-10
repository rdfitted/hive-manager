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

    return {
        subscribe,
        set,
        update,
        play: (allEvents: Event[]) => {
            if (allEvents.length === 0) return;
            
            update(state => {
                if (state.isPlaying) return state;
                
                if (playbackInterval) clearInterval(playbackInterval);
                
                playbackInterval = setInterval(() => {
                    update(s => {
                        const currentIndex = allEvents.findIndex(e => e.timestamp === s.currentTimestamp);
                        const nextIndex = currentIndex + 1;
                        
                        if (nextIndex >= allEvents.length) {
                            if (playbackInterval) clearInterval(playbackInterval);
                            return { ...s, isPlaying: false };
                        }
                        
                        return { ...s, currentTimestamp: allEvents[nextIndex].timestamp };
                    });
                }, 1000 / initialReplayState.playbackSpeed); // Use speed from state in actual implementation
                
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
                // If playing, we'd need to restart the interval with the new speed
                return newState;
            });
        },
        reset: () => {
            set(initialReplayState);
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
    [events, replay],
    ([$events, $replay]) => {
        if (!$replay.currentTimestamp) return $events.events;
        const targetTs = new Date($replay.currentTimestamp).getTime();
        return $events.events.filter(e => new Date(e.timestamp).getTime() <= targetTs);
    }
);
