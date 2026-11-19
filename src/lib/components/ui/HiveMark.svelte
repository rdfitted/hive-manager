<script lang="ts">
  let { size = 24, title }: { size?: number | string; title: string } = $props();
  const titleId = $props.id();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" role="img" aria-labelledby={titleId}>
  <title id={titleId}>{title}</title>
  <path
    fill="currentColor"
    fill-rule="evenodd"
    d="M12 1.6 21.01 6.8v10.4L12 22.4 2.99 17.2V6.8Z M12 6.6 16.68 9.3v5.4L12 17.4 7.32 14.7V9.3Z"
  />
</svg>

<style>
  svg { color: var(--accent); }
</style>
