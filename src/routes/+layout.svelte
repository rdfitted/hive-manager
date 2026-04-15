<script lang="ts">
  import '../lib/styles/lattice-tokens.css';
  import '../lib/styles/lattice-base.css';
  import { page } from '$app/stores';
  import { Kanban, House } from 'phosphor-svelte';
</script>

<nav class="top-nav" aria-label="Primary">
  <a href="/" class="nav-link" class:active={$page.url.pathname === '/'} aria-current={$page.url.pathname === '/' ? 'page' : undefined} title="Session view">
    <House size={14} weight="light" />
    <span>Session</span>
  </a>
  <a href="/dashboard" class="nav-link" class:active={$page.url.pathname === '/dashboard'} aria-current={$page.url.pathname === '/dashboard' ? 'page' : undefined} title="Dashboard">
    <Kanban size={14} weight="light" />
    <span>Dashboard</span>
  </a>
</nav>

<slot />

<style>
  .top-nav {
    position: fixed;
    top: 8px;
    right: 12px;
    z-index: 1000;
    display: flex;
    gap: 6px;
    background: var(--bg-elevated, rgba(21, 24, 32, 0.9));
    border: 1px solid var(--color-border, #222631);
    border-radius: var(--radius-sm, 2px);
    padding: 4px;
    backdrop-filter: blur(6px);
  }
  .nav-link {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    font-size: 12px;
    text-decoration: none;
    color: var(--text-secondary, #8B949E);
    border-radius: var(--radius-sm, 2px);
    font-family: var(--font-display, sans-serif);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
  .nav-link:hover {
    color: var(--accent-cyan, #00E5FF);
  }
  .nav-link.active {
    color: var(--accent-cyan, #00E5FF);
    background: rgba(0, 229, 255, 0.1);
  }
</style>
