<script lang="ts">
    // A command palette built on <dialog>: typing filters the commands, arrows walk them,
    // Enter runs the highlighted one, Escape and a backdrop click close. Focus stays in the
    // input and points at the highlighted command (role="dialog" + combobox pattern).
    import { filterCommands, type Command } from "../commandPalette";

    let {
        commands,
        onpick,
        onclose,
    }: {
        commands: Command[];
        onpick: (id: string) => void;
        onclose: () => void;
    } = $props();

    let dialog: HTMLDialogElement;
    let input: HTMLInputElement;
    let query = $state("");
    let index = $state(0);

    const results = $derived(filterCommands(commands, query));

    $effect(() => {
        // Reset to the first command whenever the query changes.
        void query;
        index = 0;
    });

    $effect(() => {
        dialog.showModal();
        const timer = setTimeout(() => input?.focus(), 0);
        return () => {
            clearTimeout(timer);
            dialog.close();
        };
    });

    function keydown(e: KeyboardEvent) {
        if (e.key === "ArrowDown") {
            e.preventDefault();
            index = Math.min(index + 1, results.length - 1);
        } else if (e.key === "ArrowUp") {
            e.preventDefault();
            index = Math.max(index - 1, 0);
        } else if (e.key === "Enter") {
            e.preventDefault();
            const command = results[index];
            if (command) onpick(command.id);
        }
    }
</script>

<dialog
    bind:this={dialog}
    class="palette"
    aria-modal="true"
    aria-label="Command palette"
    oncancel={(e) => {
        e.preventDefault();
        onclose();
    }}
    onmousedown={(e) => {
        if (e.target === e.currentTarget) onclose();
    }}
>
    <form
        class="palette-box"
        onsubmit={(e) => {
            e.preventDefault();
            const command = results[index];
            if (command) onpick(command.id);
        }}
    >
        <input
            bind:this={input}
            bind:value={query}
            type="text"
            title="Type to filter commands"
            placeholder="Type a command…"
            aria-label="Command palette"
            onkeydown={keydown}
            aria-controls="command-list"
            aria-expanded="true"
            aria-activedescendant={results[index]?.id
                ? `command-${results[index].id}`
                : undefined}
            role="combobox"
        />
        <ul id="command-list" role="listbox" aria-label="Commands">
            {#each results as c, i (c.id)}
                <li
                    id={`command-${c.id}`}
                    role="option"
                    aria-selected={i === index}
                >
                    <button
                        type="button"
                        class:active={i === index}
                        title={c.hint}
                        onclick={() => onpick(c.id)}
                        onmouseenter={() => (index = i)}
                    >
                        <span class="label">{c.label}</span>
                        <span class="hint">{c.hint}</span>
                    </button>
                </li>
            {:else}
                <li class="empty" role="option" aria-selected="false">
                    No commands match.
                </li>
            {/each}
        </ul>
    </form>
</dialog>

<style>
    dialog {
        width: min(36rem, calc(100vw - 2rem));
        margin: 4rem auto auto;
        padding: 0;
        border: 1px solid var(--border);
        border-radius: 10px;
        background: var(--surface);
        color: var(--text);
        box-shadow: 0 1rem 3rem rgb(0 0 0 / 45%);
    }
    dialog::backdrop {
        background: rgb(0 0 0 / 45%);
    }
    .palette-box {
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
        padding: 0.75rem;
    }
    input {
        width: 100%;
        padding: 0.6rem 0.7rem;
        font-size: 1rem;
        border-radius: 6px;
    }
    ul {
        list-style: none;
        margin: 0;
        padding: 0;
        max-height: 18rem;
        overflow-y: auto;
        display: flex;
        flex-direction: column;
        gap: 2px;
    }
    button {
        width: 100%;
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 2px;
        padding: 0.5rem 0.65rem;
        border: 1px solid transparent;
        border-radius: 6px;
        background: transparent;
        text-align: left;
    }
    button.active,
    button:hover {
        background: var(--surface-2);
        border-color: var(--border);
    }
    .label {
        font-weight: 600;
    }
    .hint {
        font-size: 0.8rem;
        color: var(--text-muted);
    }
    .empty {
        padding: 0.8rem;
        color: var(--text-muted);
        text-align: center;
    }
</style>
