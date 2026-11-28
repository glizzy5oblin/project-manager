<script lang="ts">
    // Preferences: the app-wide settings, persisted via the settings file.
    // The General section holds everything the app does globally: the theme and whether
    // dangerous or auto-backup actions take a backup. The other sections (git, GitHub,
    // per-project options) arrive with the rest of the settings work.
    import { onMount } from "svelte";
    import { open } from "@tauri-apps/plugin-dialog";
    import type { Theme } from "../bindings/Theme";
    import { api, toAppError } from "../api";
    import Modal from "./Modal.svelte";
    import ThemeEditor from "./ThemeEditor.svelte";

    let {
        themes,
        themeId,
        backups,
        ontheme,
        onbackups,
        onclose,
    }: {
        themes: Theme[];
        themeId: string;
        backups: boolean;
        /** Change the theme; applies and persists it. */
        ontheme: (id: string) => void;
        /** Toggle backups; applies and persists it. */
        onbackups: (backups: boolean) => void;
        onclose: () => void;
    } = $props();

    // Reset both settings to their defaults (the dark theme, backups on).
    const reset = () => {
        ontheme("dark");
        onbackups(true);
    };

    const current = $derived(themes.find((t) => t.id === themeId) ?? themes[0]);
    let editing = $state(false);
    let importing = $state(false);
    let error = $state<string | null>(null);
    let folder = $state<string | null>(null);

    onMount(async () => {
        try {
            folder = await api.themesFolder();
        } catch {
            folder = null;
        }
    });

    const importTheme = async () => {
        importing = true;
        error = null;
        try {
            const path = await open({
                multiple: false,
                filters: [{ name: "Theme", extensions: ["json"] }],
                title: "Import theme",
            });
            if (typeof path === "string") {
                const id = await api.themeImport(path);
                ontheme(id);
            }
        } catch (e) {
            error = toAppError(e).message;
        } finally {
            importing = false;
        }
    };
</script>

<Modal title="Preferences" {onclose}>
    <section class="general">
        <h3>General</h3>
        <label class="row">
            <span>Theme</span>
            <select
                title="Change the colours of the app"
                value={themeId}
                onchange={(e) => ontheme(e.currentTarget.value)}
            >
                {#each themes as t (t.id)}
                    <option value={t.id}>{t.name}</option>
                {/each}
            </select>
            <button
                title="Edit the colours of this theme"
                type="button"
                onclick={() => (editing = true)}
            >
                Edit…
            </button>
        </label>
        <label class="row">
            <span>Backups</span>
            <input
                type="checkbox"
                title="Take a backup before dangerous and auto-backup actions"
                checked={backups}
                onchange={(e) => onbackups(e.currentTarget.checked)}
            />
            <span class="hint">kept as restore points in the project</span>
        </label>
        {#if folder}
            <p class="muted">
                Themes are kept as files in <code>{folder}</code>
            </p>
        {/if}
        {#if error}
            <p class="error" role="alert">{error}</p>
        {/if}
    </section>

    {#snippet actions()}
        <button
            class="left"
            title="Import a theme file from anywhere on your computer"
            type="button"
            disabled={importing}
            onclick={importTheme}
        >
            Import theme…
        </button>
        <button
            title="Reset every preference to its default (the dark theme, backups on)"
            type="button"
            onclick={reset}
        >
            Reset to defaults
        </button>
        <button title="Close this window" type="button" onclick={onclose}>
            Close
        </button>
    {/snippet}
</Modal>

{#if editing && current}
    <ThemeEditor theme={current} onclose={() => (editing = false)} {ontheme} />
{/if}

<style>
    .general h3 {
        margin: 0 0 0.75rem;
        font-size: 0.95rem;
    }
    .row {
        display: flex;
        align-items: center;
        gap: 0.75rem;
        margin-bottom: 0.5rem;
    }
    .row span {
        min-width: 3.5rem;
        color: var(--text-muted);
    }
    select {
        flex: 1;
        padding: 0.4rem 0.5rem;
        border: 1px solid var(--border);
        border-radius: 6px;
        background: var(--surface-2);
        color: var(--text);
    }
    .row button {
        white-space: nowrap;
    }
    .hint {
        min-width: 0;
        font-size: 0.8rem;
        color: var(--text-muted);
    }
    .muted {
        color: var(--text-muted);
        font-size: 0.8rem;
        margin-top: 0.75rem;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .error {
        color: var(--danger);
        font-size: 0.85rem;
    }
    .left {
        margin-right: auto;
    }
    .muted code {
        font-size: 0.85em;
    }
</style>
