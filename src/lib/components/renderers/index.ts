/**
 * Tool-render registry barrel.
 *
 * Public surface for the native tool-render system (issue #127):
 *   - registerToolRenderer / resolveToolRenderer + types  (registry.ts)
 *   - registerBuiltinRenderers                            (builtins.ts)
 *   - ToolRenderHost component                            (ToolRenderHost.svelte)
 */

export {
  registerToolRenderer,
  resolveToolRenderer,
  hasToolRenderer,
  rendererCount,
  clearCustomRenderers,
  clearAllRenderers,
  BUILTIN_RENDERER_IDS,
  type ToolRenderer,
  type ToolRendererComponent,
  type ToolRendererProps,
  type ResolveInput,
  type BuiltinRendererId,
} from './registry';

export { registerBuiltinRenderers, resetBuiltinRegistration } from './builtins';

export { default as ToolRenderHost } from './ToolRenderHost.svelte';
