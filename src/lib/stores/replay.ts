import { writable, derived } from 'svelte/store';
import { events } from './events';
import type { Event } from '../types/domain';

interface ReplayState {
    currentTimestamp: string | null;
    currentIndex: number;
    playbackSpeed: number;
    isPlaying: boolean;
}

const initialReplayState: ReplayState = {
    currentTimestamp: null,
    currentIndex: -1,
    playbackSpeed: 1,
    isPlaying: false,
};

function createReplayStore() {
    const { subscribe, set, update } = writable<ReplayState>(initialReplayState);

    let playbackInterval: ReturnType<typeof setInterval> | null = null;
    let currentEvents: Event[] = [];

    function resolvePlaybackIndex(events: Event[], timestamp: string | null): number {
        if (!timestamp || events.length === 0) {
            return -1;
        }

        const targetTime = new Date(timestamp).getTime();
        let index = -1;

        for (let i = 0; i < events.length; i += 1) {
            if (new Date(events[i].timestamp).getTime() <= targetTime) {
                index = i;
                continue;
            }
            break;
        }

        return index;
    }

    function startInterval(speed: number) {
        if (playbackInterval) clearInterval(playbackInterval);
        playbackInterval = setInterval(() => {
            update(s => {
                const nextIndex = s.currentIndex + 1;

                if (nextIndex >= currentEvents.length) {
                    if (playbackInterval) clearInterval(playbackInterval);
                    playbackInterval = null;
                    return { ...s, isPlaying: false };
                }

                return {
                    ...s,
                    currentIndex: nextIndex,
                    currentTimestamp: currentEvents[nextIndex].timestamp,
                };
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

                const currentIndex = resolvePlaybackIndex(allEvents, state.currentTimestamp);
                startInterval(state.playbackSpeed);
                return { ...state, currentIndex, isPlaying: true };
            });
        },
        pause: () => {
            update(state => ({ ...state, isPlaying: false }));
            if (playbackInterval) {
                clearInterval(playbackInterval);
                playbackInterval = null;
            }
        },
        setTimestamp: (ts: string | null) =>
            update(state => ({
                ...state,
                currentTimestamp: ts,
                currentIndex: resolvePlaybackIndex(currentEvents, ts),
            })),
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

function compareEventTimestamp(left: Event, right: Event): number {
    return new Date(left.timestamp).getTime() - new Date(right.timestamp).getTime();
}

function toChronologicalOrder(events: Event[]): Event[] {
    if (events.length < 2) {
        return events;
    }

    let isAscending = true;
    let isDescending = true;

    for (let index = 1; index < events.length; index += 1) {
        const comparison = compareEventTimestamp(events[index - 1], events[index]);
        if (comparison > 0) {
            isAscending = false;
        }
        if (comparison < 0) {
            isDescending = false;
        }
        if (!isAscending && !isDescending) {
            break;
        }
    }

    if (isAscending) {
        return [...events];
    }
    if (isDescending) {
        return [...events].reverse();
    }

    return [...events].sort(compareEventTimestamp);
}

export const chronologicalEvents = derived(
    events,
    $events => toChronologicalOrder($events.events)
);

export const eventsAtTimestamp = derived(
    [chronologicalEvents, replay],
    ([$chronologicalEvents, $replay]) => {
        if (!$replay.currentTimestamp) return $chronologicalEvents;
        const targetTs = new Date($replay.currentTimestamp).getTime();
        return $chronologicalEvents.filter(e => new Date(e.timestamp).getTime() <= targetTs);
    }
);
