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

<div class="replay-controls">
    <div class="main-row">
        <button class="control-btn" on:click={() => step(-1)} disabled={currentEventIndex <= 0}>
            Step Back
        </button>

        <button class="control-btn play-pause" on:click={togglePlay}>
            {$replay.isPlaying ? 'Pause' : 'Play'}
        </button>

        <button class="control-btn" on:click={() => step(1)} disabled={currentEventIndex >= $chronologicalEvents.length - 1}>
            Step Forward
        </button>

        <div class="speed-selector">
            <span class="label">Speed:</span>
            <select value={$replay.playbackSpeed} on:change={(e) => replay.setSpeed(parseFloat(e.currentTarget.value))}>
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
        background: var(--color-surface);
        border-top: 1px solid var(--color-border);
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

    .control-btn {
        padding: 6px 12px;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        color: var(--color-text);
        cursor: pointer;
        font-size: 0.8rem;
    }

    .control-btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .control-btn.play-pause {
        background: var(--color-accent);
        color: var(--color-bg);
        font-weight: bold;
        min-width: 80px;
    }

    .speed-selector {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 16px;
    }

    .speed-selector select {
        padding: 4px;
        background: var(--color-bg);
        border: 1px solid var(--color-border);
        border-radius: var(--radius-sm);
        color: var(--color-text);
        font-family: inherit;
        font-size: 0.7rem;
    }

    .label {
        font-size: 0.7rem;
        color: var(--color-text-muted);
        text-transform: uppercase;
    }

    .seek-row {
        display: flex;
        align-items: center;
        gap: 12px;
    }

    .seek-bar {
        flex: 1;
        accent-color: var(--color-accent);
    }

    .timestamp-display {
        font-size: 0.75rem;
        color: var(--color-text-muted);
        min-width: 180px;
        text-align: right;
    }
</style>
