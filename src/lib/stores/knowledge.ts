import { writable } from 'svelte/store';
import { apiUrl } from '$lib/config';
import { normalizeKnowledgeGraph, normalizeKnowledgePage } from '$lib/knowledge/graphUtils';
import type { KnowledgeGraph, KnowledgePage } from '$lib/knowledge/types';

export interface KnowledgeState {
  graph: KnowledgeGraph;
  loading: boolean;
  refreshing: boolean;
  error: string | null;
  selectedId: string | null;
  page: KnowledgePage | null;
  pageLoading: boolean;
  pageError: string | null;
}

const EMPTY_GRAPH: KnowledgeGraph = { nodes: [], edges: [], truncated: false };

function responseError(response: Response, subject: string): string {
  return `${subject} request failed (${response.status})`;
}

function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  if (typeof error === 'string' && error.trim()) return error;
  return fallback;
}

function withSessionId(path: string, sessionId: string | null): string {
  if (sessionId === null) return path;
  const separator = path.includes('?') ? '&' : '?';
  return `${path}${separator}session_id=${encodeURIComponent(sessionId)}`;
}

export function createKnowledgeStore() {
  let graphRequest = 0;
  let pageRequest = 0;
  let graphScope: string | null = null;
  let hasGraphScope = false;
  const { subscribe, set, update } = writable<KnowledgeState>({
    graph: EMPTY_GRAPH,
    loading: false,
    refreshing: false,
    error: null,
    selectedId: null,
    page: null,
    pageLoading: false,
    pageError: null,
  });

  async function loadGraph(sessionId: string | null = null): Promise<boolean> {
    const request = ++graphRequest;
    const scopeChanged = hasGraphScope && graphScope !== sessionId;
    graphScope = sessionId;
    hasGraphScope = true;

    if (scopeChanged) pageRequest += 1;
    update((state) => {
      if (scopeChanged) {
        return {
          ...state,
          graph: EMPTY_GRAPH,
          loading: true,
          refreshing: false,
          error: null,
          selectedId: null,
          page: null,
          pageLoading: false,
          pageError: null,
        };
      }

      return {
        ...state,
        loading: state.graph.nodes.length === 0,
        refreshing: state.graph.nodes.length > 0,
        error: null,
      };
    });

    try {
      const response = await fetch(apiUrl(withSessionId('/api/knowledge/graph', sessionId)));
      if (!response.ok) throw new Error(responseError(response, 'Knowledge graph'));
      const graph = normalizeKnowledgeGraph(await response.json());
      if (request !== graphRequest) return false;
      update((state) => {
        const selectedId = state.selectedId;
        const selectionStillExists = selectedId === null
          || graph.nodes.some((node) => node.id === selectedId);

        if (selectionStillExists) {
          return { ...state, graph, loading: false, refreshing: false, error: null };
        }

        pageRequest += 1;
        return {
          ...state,
          graph,
          loading: false,
          refreshing: false,
          error: null,
          selectedId: null,
          page: null,
          pageLoading: false,
          pageError: null,
        };
      });
      return true;
    } catch (error) {
      if (request !== graphRequest) return false;
      update((state) => ({
        ...state,
        loading: false,
        refreshing: false,
        error: errorMessage(error, 'Unable to load the knowledge graph.'),
      }));
      return false;
    }
  }

  async function selectNode(
    id: string | null,
    sessionId: string | null = null,
  ): Promise<boolean> {
    const request = ++pageRequest;
    if (!id) {
      update((state) => ({
        ...state,
        selectedId: null,
        page: null,
        pageLoading: false,
        pageError: null,
      }));
      return true;
    }

    update((state) => ({
      ...state,
      selectedId: id,
      page: null,
      pageLoading: true,
      pageError: null,
    }));

    try {
      const path = withSessionId(
        `/api/knowledge/page?id=${encodeURIComponent(id)}`,
        sessionId,
      );
      const response = await fetch(apiUrl(path));
      if (!response.ok) throw new Error(responseError(response, 'Knowledge page'));
      const page = normalizeKnowledgePage(await response.json());
      if (!page) throw new Error('Knowledge page returned an invalid response.');
      if (request !== pageRequest) return false;
      update((state) => ({ ...state, page, pageLoading: false, pageError: null }));
      return true;
    } catch (error) {
      if (request !== pageRequest) return false;
      update((state) => ({
        ...state,
        page: null,
        pageLoading: false,
        pageError: errorMessage(error, 'Unable to load the knowledge page.'),
      }));
      return false;
    }
  }

  return {
    subscribe,
    loadGraph,
    selectNode,
    retryPage(sessionId: string | null = null) {
      let selectedId: string | null = null;
      const unsubscribe = subscribe((state) => {
        selectedId = state.selectedId;
      });
      unsubscribe();
      return selectNode(selectedId, sessionId);
    },
    reset() {
      graphRequest += 1;
      pageRequest += 1;
      graphScope = null;
      hasGraphScope = false;
      set({
        graph: EMPTY_GRAPH,
        loading: false,
        refreshing: false,
        error: null,
        selectedId: null,
        page: null,
        pageLoading: false,
        pageError: null,
      });
    },
  };
}

export const knowledgeStore = createKnowledgeStore();
