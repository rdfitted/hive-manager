import { writable } from 'svelte/store';
import type { RolePack, SessionTemplate } from '../types/domain';
import { apiUrl } from '../config';

interface TemplatesState {
    templates: SessionTemplate[];
    rolePacks: RolePack[];
    loading: boolean;
    error: string | null;
}

interface TemplateCatalog {
    templates: SessionTemplate[];
    role_packs: RolePack[];
}

function createTemplatesStore() {
    const { subscribe, set, update } = writable<TemplatesState>({
        templates: [],
        rolePacks: [],
        loading: false,
        error: null,
    });

    return {
        subscribe,

        async fetchTemplates() {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl('/api/templates'));
                if (!response.ok) throw new Error(`Failed to fetch templates: ${response.statusText}`);
                const catalog: TemplateCatalog = await response.json();
                
                update(state => ({
                    ...state,
                    templates: catalog.templates,
                    rolePacks: catalog.role_packs,
                    loading: false
                }));
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
            }
        },

        async saveTemplate(template: SessionTemplate) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl('/api/templates'), {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(template)
                });
                if (!response.ok) throw new Error(`Failed to save template: ${response.statusText}`);
                const savedTemplate: SessionTemplate = await response.json();

                update(state => ({
                    ...state,
                    templates: [...state.templates.filter(t => t.id !== savedTemplate.id), savedTemplate],
                    loading: false
                }));
                return savedTemplate;
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
                throw err;
            }
        },

        async deleteTemplate(id: string) {
            update(state => ({ ...state, loading: true, error: null }));
            try {
                const response = await fetch(apiUrl(`/api/templates/${id}`), {
                    method: 'DELETE'
                });
                if (!response.ok) throw new Error(`Failed to delete template: ${response.statusText}`);

                update(state => ({
                    ...state,
                    templates: state.templates.filter(t => t.id !== id),
                    loading: false
                }));
            } catch (err) {
                update(state => ({ ...state, loading: false, error: (err as Error).message }));
                throw err;
            }
        }
    };
}

export const templates = createTemplatesStore();
export const selectedTemplate = writable<SessionTemplate | null>(null);
