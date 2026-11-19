<script lang="ts">
    // A modal built on <dialog>: focus stays inside, Escape closes, the page behind is inert.
    import type { Snippet } from "svelte";

    let {
        title,
        onclose,
        children,
        actions,
    }: {
        title: string;
        onclose: () => void;
        children: Snippet;
        actions?: Snippet;
    } = $props();

    let dialog: HTMLDialogElement;
    const titleId = `modal-${Math.random().toString(36).slice(2)}`;

    $effect(() => {
        dialog.showModal();
        return () => dialog.close();
    });
</script>

<dialog
    bind:this={dialog}
    aria-labelledby={titleId}
    oncancel={(e) => {
        e.preventDefault();
        onclose();
    }}
>
    <h2 id={titleId}>{title}</h2>
    <div class="body">{@render children()}</div>
    {#if actions}
        <div class="actions">{@render actions()}</div>
    {/if}
</dialog>

<style>
    dialog {
        width: min(32rem, calc(100vw - 2rem));
        padding: 1.25rem;
        border: 1px solid var(--border);
        border-radius: 8px;
        background: var(--surface);
        color: var(--text);
    }
    dialog::backdrop {
        background: rgb(0 0 0 / 50%);
    }
    h2 {
        margin: 0 0 1rem;
        font-size: 1.15rem;
    }
    .body {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: 0.85rem;
    }
    .actions {
        display: flex;
        justify-content: flex-end;
        gap: 0.5rem;
        margin-top: 1.25rem;
    }
</style>
