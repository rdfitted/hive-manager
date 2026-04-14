import { writable } from 'svelte/store';
import type { Event as SessionEvent } from '../types/domain';
import { apiUrl } from '$lib/config';

const EVENT_TYPES = [
    'session_created',
    'session_status_changed',
    'cell_created',
    'cell_status_changed',
    'conversation_message',
    'workspace_created',
    'agent_launched',
    'agent_completed',
    'agent_waiting_input',
    'agent_failed',
    'artifact_updated',
    'resolver_selected_candidate',
] as const;

interface EventsState {
    events: SessionEvent[];
    loading: boolean;
    error: string | null;
}

function createEventsStore() {
    const { subscribe, set, update } = writable<EventsState>({
        events: [],
        loading: false,
        error: null,
    });

    let eventSource: EventSource | null = null;
    const MAX_CONSECUTIVE_ERRORS_BEFORE_PROBE = 3;
    const SESSION_NOT_FOUND_ERROR = 'Session no longer exists';

    function prependEvent(raw: MessageEvent<string>, source: EventSource) {
        if (eventSource !== source) {
            return;
        }

        try {
            const event: SessionEvent = JSON.parse(raw.data);
            update(state => ({
                ...state,
                events: [event, ...state.events].slice(0, 1000),
            }));
        } catch (err) {
            console.error('Failed to parse event:', err);
        }
    }

    function handleLagged(raw: MessageEvent<string>, source: EventSource, sessionId: string, store: any) {
        if (eventSource !== source) {
            return;
        }

        try {
            const data = JSON.parse(raw.data);
            const syntheticEvent: SessionEvent = {
                id: `lagged-${Date.now()}`,
                session_id: sessionId,
                event_type: 'lagged',
                timestamp: new Date().toISOString(),
                payload: data,
                severity: 'warning'
            };

            update(state => ({
                ...state,
                events: [syntheticEvent, ...state.events].slice(0, 1000),
            }));

            // CONTRACT: resync after lag
            store.fetchEvents(sessionId);
        } catch (err) {
            console.error('Failed to handle lagged event:', err);
        }
    }

    return {
        subscribe,

        connect(sessionId: string) {
            if (eventSource) {
                eventSource.close();
            }

            let consecutiveErrors = 0;
            update(state => ({ ...state, loading: true, error: null }));

            const source = new EventSource(apiUrl(`/api/sessions/${sessionId}/stream`));
            eventSource = source;

            source.onopen = () => {
                if (eventSource !== source) {
                    return;
                }

                update(state => ({ ...state, loading: false }));
            };

            source.onmessage = (event) => {
                consecutiveErrors = 0;
                prependEvent(event, source);
            };
            EVENT_TYPES.forEach((eventType) => {
                source.addEventListener(eventType, (event) => {
                    consecutiveErrors = 0;
                    prependEvent(event as MessageEvent<string>, source);
                });
            });

            source.addEventListener('lagged', (event) => {
                handleLagged(event as MessageEvent<string>, source, sessionId, this);
            });

            source.onerror = async (err) => {
                console.error('EventSource failed:', err);
                if (eventSource !== source) {
                    return;
                }

                consecutiveErrors += 1;
                if (consecutiveErrors >= MAX_CONSECUTIVE_ERRORS_BEFORE_PROBE) {
                    try {
                        const response = await fetch(apiUrl(`/api/sessions/${sessionId}`));
                        if (eventSource !== source) {
                            return;
                        }

                        if (response.status === 404) {
                            this.disconnect();
                            update(state => ({ ...state, loading: false, error: SESSION_NOT_FOUND_ERROR }));
                            return;
                        }
                    } catch (probeError) {
                        console.error('Failed to probe session after EventSource error:', probeError);
                    }
                }

                update(state => ({ ...state, loading: false, error: 'Connection lost, retrying...' }));
            };
        },

        disconnect() {
            if (eventSource) {
                eventSource.close();
                eventSource = null;
            }
        },

        async fetchEvents(sessionId: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/sessions/${sessionId}/events`));
                if (!response.ok) throw new Error(`Failed to fetch events: ${response.statusText}`);
                const events: SessionEvent[] = await response.json();
                
                update(state => ({
                    ...state,
                    events: [...events].reverse(), // Show newest first
                    loading: false
                }));
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        }
    };
}

export const events = createEventsStore();
