<script module lang="ts">
  export const PREVIEW_CSP = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src 'none'; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'";
  const tags = new Set('a abbr address article aside b blockquote br caption code col colgroup dd del details div dl dt em figcaption figure footer h1 h2 h3 h4 h5 h6 header hr i img ins kbd li main mark nav ol p pre s samp section small span strong style sub summary sup table tbody td tfoot th thead time tr u ul var'.split(' '));
  const attributes = new Set('class id title style dir lang hidden colspan rowspan scope start reversed open'.split(' '));

  export function previewDocument(source: string): string {
    // Template contents have no browsing context: parsing cannot fetch hostile
    // images/frames in the privileged app, unlike a DOMParser document.
    const template = document.createElement('template');
    template.innerHTML = source;
    for (const element of template.content.querySelectorAll('*')) {
      if (element.namespaceURI !== 'http://www.w3.org/1999/xhtml' || !tags.has(element.localName)) {
        element.remove();
        continue;
      }
      for (const attribute of [...element.attributes]) {
        const dataImage = element.localName === 'img' && attribute.name === 'src'
          && /^data:image\/(?:png|gif|jpeg|webp|avif);base64,[a-z\d+/=\s]+$/i.test(attribute.value);
        if (!attributes.has(attribute.name) && !attribute.name.startsWith('aria-') && !dataImage) element.removeAttribute(attribute.name);
      }
    }
    // The frame inherits the app policy; this first meta can only tighten it.
    // Navigation attributes are removed too: CSP fetch rules do not block links.
    return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}"><meta name="referrer" content="no-referrer"><meta name="color-scheme" content="dark"></head><body>${template.innerHTML}</body></html>`;
  }
</script>

<script lang="ts">
  let { source }: { source: string } = $props();
  let open = $state(false);
  const srcdoc = $derived(open ? previewDocument(source) : '');
  $effect(() => { void source; open = false; });
</script>

<div class="html-preview">
  <button type="button" class="lattice-btn lattice-btn--ghost" aria-expanded={open} onclick={() => { open = !open; }}>
    {open ? 'Close HTML preview' : 'Preview HTML'}
  </button>
  {#if open}
    <iframe title="HTML preview" {srcdoc} sandbox="" referrerpolicy="no-referrer"></iframe>
  {/if}
</div>

<style>
  .html-preview { padding: 0 var(--space-3) var(--space-3); }
  iframe { display: block; box-sizing: border-box; width: 100%; height: 24rem; margin-top: var(--space-2); border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--surface); }
  @media (forced-colors: active) { iframe { border-color: CanvasText; } }
</style>
