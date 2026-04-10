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
                <span class="status-badge {status}">{status.replaceAll('_', ' ')}</span>
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
        background: var(--bg-void);
        border-bottom: 1px solid var(--border-structural);
        padding: 12px 20px;
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 32px;
    }

    .main-info {
        flex: 1;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    .top-row {
        display: flex;
        align-items: center;
        gap: 12px;
    }

    .session-name {
        margin: 0;
        font-size: 18px;
        font-weight: 600;
        color: var(--text-primary);
    }

    .mode-badge {
        font-size: 9px;
        font-weight: 800;
        text-transform: uppercase;
        padding: 2px 6px;
        border-radius: var(--radius-sm);
        letter-spacing: 0.1em;
    }

    .mode-badge.hive { background: var(--status-warning); color: var(--bg-void); }
    .mode-badge.fusion { background: var(--accent-cyan); color: var(--bg-void); }
    .mode-badge.swarm { background: var(--status-success); color: var(--bg-void); }
    .mode-badge.solo { background: var(--status-error); color: var(--bg-void); }

    .status-badge {
        font-size: 10px;
        padding: 2px 8px;
        border-radius: 999px;
        background: var(--bg-surface);
        color: var(--text-secondary);
        border: 1px solid var(--border-structural);
        text-transform: capitalize;
    }

    .status-badge.running { color: var(--status-success); border-color: var(--status-success); background: color-mix(in srgb, var(--status-success) 5%, transparent); }

    .objective {
        margin: 0;
        font-size: 12px;
        color: var(--text-secondary);
        max-width: 800px;
        line-height: 1.4;
    }

    .stats {
        display: flex;
        gap: 20px;
    }

    .stat {
        display: flex;
        flex-direction: column;
        gap: 2px;
        align-items: flex-end;
    }

    .stat .label {
        font-size: 9px;
        text-transform: uppercase;
        color: var(--text-secondary);
        font-weight: 700;
        letter-spacing: 0.05em;
    }

    .stat .value {
        font-size: 11px;
        color: var(--text-primary);
        font-family: var(--font-mono);
    }

    .placeholder {
        color: var(--text-disabled);
        font-style: italic;
        padding: 8px 0;
    }
</style>
