<script lang="ts">
    // Theme editor: edit the colors of one theme (a built-in theme saved with a new name becomes
    // a user theme). Colors are edited as text with a live swatch; the backend re-validates
    // every value on save (unknown keys and unparsable colors are refused, and only the colors
    // that differ from the built-in theme of the same scheme are stored).
    import { save } from "@tauri-apps/plugin-dialog";
    import type { Theme } from "../bindings/Theme";
    import { api, toAppError } from "../api";
    import Modal from "./Modal.svelte";

    let {
        theme,
        onclose,
        ontheme,
    }: {
        theme: Theme;
        onclose: () => void;
        /** Select the theme after a save; applies and persists it. */
        ontheme: (id: string) => void;
    } = $props();

    const isBuiltin = $derived(theme.builtin);
    let draftName = $state(theme.name);
    let colors = $state({ ...theme.colors });
    let busy = $state(false);
    let error = $state<string | null>(null);

    const keys = $derived(Object.keys(colors).sort());
    const changed = $derived(keys.filter((k) => colors[k] !== theme.colors[k]));

    const message = (e: unknown) => toAppError(e).message;

    const set = (key: string, value: string) => {
        colors = { ...colors, [key]: value };
    };

    const saveTheme = async () => {
        busy = true;
        error = null;
        try {
            if (isBuiltin) {
                const id = await api.themeSaveAs(
                    draftName,
                    theme.scheme,
                    colors,
                );
                ontheme(id);
            } else {
                await api.themeSaveUser(theme.id, colors);
            }
            onclose();
        } catch (e) {
            error = message(e);
        } finally {
            busy = false;
        }
    };

    const exportTheme = async () => {
        busy = true;
        error = null;
        try {
            const path = await save({
                defaultPath: `${theme.name}.json`,
                filters: [{ name: "Theme", extensions: ["json"] }],
            });
            if (path) {
                await api.themeExport(theme.id, path);
            }
        } catch (e) {
            error = message(e);
        } finally {
            busy = false;
        }
    };
</script>

<Modal
    title={isBuiltin ? "Edit theme — new theme" : "Edit theme"}
    {onclose}
    wide
>
    <div class="editor">
        {#if isBuiltin}
            <label class="name-field">
                <span>New theme name</span>
                <input
                    title="A name for the new theme; this saves a copy of the built-in theme under a new id"
                    type="text"
                    maxlength="60"
                    bind:value={draftName}
                />
            </label>
        {:else}
            <p class="hint">
                Themes live as <code>.json</code> files in the themes folder; the
                file name is the theme's id.
            </p>
        {/if}

        {#if changed.length > 0}
            <p class="hint">
                Unsaved changes: {changed.join(", ")}
            </p>
        {/if}

        <div class="grid">
            {#each keys as key (key)}
                <label
                    class="cell"
                    title={`${key} — click the colour to change it`}
                >
                    <span class="key">{key}</span>
                    <input
                        type="text"
                        title={`${key} — a colour, e.g. #3a86ff or rgb(0 0 0)`}
                        spellcheck="false"
                        aria-label={key}
                        value={colors[key]}
                        oninput={(e) => set(key, e.currentTarget.value)}
                    />
                    <span class="swatch" style:background={colors[key]}></span>
                </label>
            {/each}
        </div>

        {#if error}
            <p class="error" role="alert">{error}</p>
        {/if}
    </div>

    {#snippet actions()}
        <button
            type="button"
            title="Export this theme as a complete .json file"
            disabled={busy}
            onclick={exportTheme}
        >
            Export…
        </button>
        <button
            type="button"
            title="Close without saving"
            disabled={busy}
            onclick={onclose}
        >
            Cancel
        </button>
        <button
            type="button"
            class="primary"
            title={isBuiltin
                ? "Save a copy of this built-in theme under the new name"
                : "Save the colours you changed"}
            disabled={busy || (isBuiltin && !draftName.trim())}
            onclick={saveTheme}
        >
            {isBuiltin ? "Save as new theme" : "Save"}
        </button>
    {/snippet}
</Modal>

<style>
    .editor {
        display: grid;
        gap: 0.75rem;
    }
    .name-field {
        display: grid;
        gap: 0.3rem;
    }
    .name-field span {
        color: var(--text-muted);
        font-size: 0.85rem;
    }
    .name-field input,
    .cell input {
        padding: 0.35rem 0.45rem;
        border: 1px solid var(--border);
        border-radius: 6px;
        background: var(--surface-2);
        color: var(--text);
        font: inherit;
    }
    .hint {
        margin: 0;
        color: var(--text-muted);
        font-size: 0.8rem;
    }
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr));
        gap: 0.4rem;
        max-height: 22rem;
        overflow-y: auto;
        padding-right: 0.25rem;
    }
    .cell {
        display: grid;
        grid-template-columns: 4.5rem 1fr 1.4rem;
        align-items: center;
        gap: 0.4rem;
    }
    .key {
        color: var(--text-muted);
        font-size: 0.8rem;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .swatch {
        width: 1.4rem;
        height: 1.4rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: transparent;
    }
    .error {
        margin: 0;
        padding: 0.4rem 0.6rem;
        border: 1px solid var(--danger);
        border-radius: 6px;
        color: var(--danger);
        font-size: 0.85rem;
    }
    button.primary {
        border-color: var(--accent);
        background: var(--accent);
        color: var(--accent-text);
    }
    code {
        font-size: 0.85em;
    }
</style>
