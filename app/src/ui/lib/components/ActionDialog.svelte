<script lang="ts">
    // Confirms a write action: its impact in plain words and the exact git commands. Dangerous
    // actions say that a backup is taken first; the run carries the preview's token, so what the
    // user confirms is what runs (docs/actions.md).
    import { actions as runner } from "../actions.svelte";
    import Modal from "./Modal.svelte";

    const p = $derived(runner.pending);
</script>

{#if p}
    <Modal title={p.preview.title} onclose={() => p.answer(false)}>
        {#if p.note}
            <p class="note" role="status">{p.note}</p>
        {/if}
        {#if p.untrusted}
            <p class="warn">
                This repository is owned by another user. Its own git settings
                (hooks, filters, commands) can run programs when git writes to
                it. Continue only if you trust it.
            </p>
        {/if}
        {#if p.preview.impact.length}
            <ul class="impact">
                {#each p.preview.impact as line, i (i)}
                    <li>{line}</li>
                {/each}
            </ul>
        {/if}
        {#if p.preview.dangerous}
            <p class="muted">
                A backup is taken first. Its name is listed in Activity.
            </p>
        {/if}
        <details>
            <summary title="The exact git commands that will run">
                Commands ({p.preview.commands.length})
            </summary>
            <pre>{p.preview.commands.join("\n")}</pre>
        </details>
        {#snippet actions()}
            <button
                title="Do nothing. Your project stays as it is"
                type="button"
                onclick={() => p.answer(false)}
            >
                Cancel
            </button>
            <!-- svelte-ignore a11y_autofocus -->
            <button
                title={p.preview.dangerous
                    ? "Take a backup, then make this change"
                    : "Make this change"}
                type="button"
                class={p.preview.dangerous || p.untrusted
                    ? "danger"
                    : "primary"}
                autofocus={!p.preview.dangerous}
                onclick={() => p.answer(true)}
            >
                {p.preview.dangerous ? "Back up and run" : "Run"}
            </button>
        {/snippet}
    </Modal>
{/if}

<style>
    p {
        margin: 0;
    }
    .note {
        padding: 0.4rem 0.6rem;
        border-left: 3px solid var(--accent);
        background: var(--surface-2);
    }
    .warn {
        color: var(--warning);
    }
    .muted {
        color: var(--text-muted);
        font-size: 0.9em;
    }
    .impact {
        display: grid;
        gap: 0.3rem;
        margin: 0;
        padding-left: 1.2rem;
    }
    details {
        color: var(--text-muted);
    }
    pre {
        max-height: 10rem;
        margin: 0.4rem 0 0;
        padding: 0.5rem;
        overflow: auto;
        border-radius: 4px;
        background: var(--bg);
        color: var(--text);
        font-size: 0.85em;
        white-space: pre-wrap;
        word-break: break-word;
    }
</style>
