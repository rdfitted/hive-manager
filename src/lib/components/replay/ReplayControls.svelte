<script lang="ts">
    import { replay, chronologicalEvents } from '$lib/stores/replay';

    $: currentEventIndex = $chronologicalEvents.findIndex(e => e.timestamp === $replay.currentTimestamp);
    $: progress = $chronologicalEvents.length > 0 
        ? ((currentEventIndex + 1) / $chronologicalEvents.length) * 100 
        : 0;

    function handleSeek(e: Event) {
        const value = parseInt((e.target as HTMLInputElement).value);
        const index = Math.floor((value / 100) * ($chronologicalEvents.length - 1));
        if ($chronologicalEvents[index]) {
            replay.setTimestamp($chronologicalEvents[index].timestamp);
        }
    }

    function togglePlay() {
        if ($replay.isPlaying) {
            replay.pause();
        } else {
            replay.play($chronologicalEvents);
        }
    }

    function step(direction: number) {
        const nextIndex = currentEventIndex + direction;
        if (nextIndex >= 0 && nextIndex < $chronologicalEvents.length) {
            replay.setTimestamp($chronologicalEvents[nextIndex].timestamp);
        }
    }
</script>

<div class="replay-controls lattice-panel">
    <div class="main-row">
        <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--compact" on:click={() => step(-1)} disabled={currentEventIndex <= 0}>
            Step Back
        </button>

        <button type="button" class="lattice-btn lattice-btn--primary lattice-btn--filled lattice-btn--compact" on:click={togglePlay}>
            {$replay.isPlaying ? 'Pause' : 'Play'}
        </button>

        <button type="button" class="lattice-btn lattice-btn--ghost lattice-btn--compact" on:click={() => step(1)} disabled={currentEventIndex >= $chronologicalEvents.length - 1}>
            Step Forward
        </button>

        <div class="speed-selector">
            <span class="label">Speed:</span>
            <select class="lattice-input" value={$replay.playbackSpeed} on:change={(e) => replay.setSpeed(parseFloat(e.currentTarget.value))}>
                <option value={0.5}>0.5x</option>
                <option value={1}>1x</option>
                <option value={2}>2x</option>
                <option value={5}>5x</option>
            </select>
        </div>
    </div>

    <div class="seek-row">
        <input 
            type="range" 
            min="0" 
            max="100" 
            value={progress} 
            on:input={handleSeek}
            class="seek-bar"
        />
        <div class="timestamp-display">
            {$replay.currentTimestamp ? new Date($replay.currentTimestamp).toLocaleString() : 'No timestamp selected'}
        </div>
    </div>
</div>

<style>
    .replay-controls {
        padding: 12px;
        display: flex;
        flex-direction: column;
        gap: 8px;
        font-family: var(--font-mono);
    }

    .main-row {
        display: flex;
        align-items: center;
        gap: 12px;
        justify-content: center;
    }

    .speed-selector {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 16px;
    }

    .label {
        font-size: 0.7rem;
        color: var(--text-secondary);
        text-transform: uppercase;
    }

    .seek-row {
        display: flex;
        align-items: center;
        gap: 12px;
    }

    .seek-bar {
        flex: 1;
        accent-color: var(--accent-cyan);
    }

    .timestamp-display {
        font-size: 0.75rem;
        color: var(--text-secondary);
        min-width: 180px;
        text-align: right;
    }
</style>
