<script lang="ts">
    import { activeSession, serdeEnumVariantName } from '../../stores/sessions';

    $: session = $activeSession;
    $: mode = session ? serdeEnumVariantName(session.session_type)?.toLowerCase() ?? 'session' : 'session';
    $: status = session ? serdeEnumVariantName(session.state)?.toLowerCase() ?? 'unknown' : 'unknown';
</script>

<div class="session-header">
    {#if session}
        <div class="main-info">
            <div class="top-row">
                <span class="mode-badge {mode}">{mode}</span>
                <h1 class="session-name">{session.name || session.id}</h1>
                <span class="status-badge {status}">{status.replace('_', ' ')}</span>
            </div>
            <p class="objective">Session ID: {session.id}</p>
        </div>
        
        <div class="stats">
            <div class="stat">
                <span class="label">Project</span>
                <span class="value">{session.project_path.split(/[\\\/]/).pop()}</span>
            </div>
            <div class="stat">
                <span class="label">Created</span>
                <span class="value">{new Date(session.created_at).toLocaleString()}</span>
            </div>
        </div>
    {:else}
        <div class="placeholder">Select a session to view details</div>
    {/if}
</div>

<style>
    .session-header {
        background: #0a0a0a;
        border-bottom: 1px solid rgba(255, 255, 255, 0.05);
        padding: 16px 24px;
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 32px;
    }

    .main-info {
        flex: 1;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .top-row {
        display: flex;
        align-items: center;
        gap: 12px;
    }

    .session-name {
        margin: 0;
        font-size: 20px;
        font-weight: 600;
        color: #fff;
    }

    .mode-badge {
        font-size: 10px;
        font-weight: 800;
        text-transform: uppercase;
        padding: 2px 6px;
        border-radius: 4px;
        letter-spacing: 0.1em;
    }

    .mode-badge.hive { background: #facc15; color: #000; }
    .mode-badge.fusion { background: #818cf8; color: #fff; }

    .status-badge {
        font-size: 11px;
        padding: 2px 8px;
        border-radius: 999px;
        background: rgba(255, 255, 255, 0.05);
        color: #888;
        border: 1px solid rgba(255, 255, 255, 0.1);
        text-transform: capitalize;
    }

    .status-badge.active { color: #10b981; border-color: rgba(16, 185, 129, 0.3); background: rgba(16, 185, 129, 0.05); }

    .objective {
        margin: 0;
        font-size: 14px;
        color: #888;
        max-width: 800px;
        line-height: 1.5;
    }

    .stats {
        display: flex;
        gap: 24px;
    }

    .stat {
        display: flex;
        flex-direction: column;
        gap: 4px;
        align-items: flex-end;
    }

    .stat .label {
        font-size: 10px;
        text-transform: uppercase;
        color: #555;
        font-weight: 700;
        letter-spacing: 0.05em;
    }

    .stat .value {
        font-size: 12px;
        color: #bbb;
        font-family: var(--font-mono);
    }

    .placeholder {
        color: #444;
        font-style: italic;
        padding: 8px 0;
    }
</style>
