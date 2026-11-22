<script lang="ts">
    // The hub tab plus one tab per open project. Keyboard: arrows move between tabs, Home/End jump,
    // Alt+Shift+arrows move the focused tab, Delete closes it. Tabs can also be dragged.
    import type { Session } from "../bindings/Session";

    let {
        session,
        names,
        onselect,
        onclose,
        onmove,
    }: {
        session: Session;
        names: Map<string, string>;
        onselect: (id: string | null) => void;
        onclose: (id: string) => void;
        onmove: (id: string, to: number) => void;
    } = $props();

    let bar: HTMLElement;
    let dragging = $state<string | null>(null);
    let dropAt = $state<number | null>(null);

    const ids = $derived([null, ...session.tabs.map((t) => t.projectId)]);

    function focusTab(id: string | null) {
        queueMicrotask(() =>
            bar.querySelector<HTMLElement>(`[data-tab="${id ?? ""}"]`)?.focus(),
        );
    }

    function keydown(e: KeyboardEvent, id: string | null) {
        const i = ids.indexOf(id);
        let next: string | null | undefined;
        if (
            e.altKey &&
            e.shiftKey &&
            id !== null &&
            (e.key === "ArrowLeft" || e.key === "ArrowRight")
        ) {
            onmove(id, i - 1 + (e.key === "ArrowLeft" ? -1 : 1));
            focusTab(id);
            e.preventDefault();
            return;
        }
        if (e.key === "ArrowRight") next = ids[(i + 1) % ids.length];
        else if (e.key === "ArrowLeft")
            next = ids[(i - 1 + ids.length) % ids.length];
        else if (e.key === "Home") next = ids[0];
        else if (e.key === "End") next = ids[ids.length - 1];
        else if (e.key === "Delete" && id !== null) {
            onclose(id);
            focusTab(session.active);
            e.preventDefault();
            return;
        }
        if (next !== undefined) {
            onselect(next);
            focusTab(next);
            e.preventDefault();
        }
    }

    function dragover(e: DragEvent, index: number) {
        if (!dragging) return;
        e.preventDefault();
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        dropAt = e.clientX < rect.left + rect.width / 2 ? index : index + 1;
    }

    function drop(e: DragEvent) {
        e.preventDefault();
        if (dragging && dropAt !== null) {
            const from = session.tabs.findIndex(
                (t) => t.projectId === dragging,
            );
            onmove(dragging, dropAt > from ? dropAt - 1 : dropAt);
        }
        dragging = null;
        dropAt = null;
    }
</script>

<div bind:this={bar} class="bar" role="tablist" aria-label="Open projects">
    <button
        title="Go to the list of your projects"
        type="button"
        role="tab"
        class="tab hub"
        data-tab=""
        aria-selected={session.active === null}
        tabindex={session.active === null ? 0 : -1}
        onclick={() => onselect(null)}
        onkeydown={(e) => keydown(e, null)}
    >
        Hub
    </button>
    {#each session.tabs as tab, index (tab.projectId)}
        {@const id = tab.projectId}
        {@const name = names.get(id) ?? id}
        <div
            class="tab"
            class:selected={session.active === id}
            class:drop-before={dropAt === index}
            class:drop-after={dropAt === index + 1 &&
                index === session.tabs.length - 1}
            class:dragging={dragging === id}
            role="presentation"
            draggable="true"
            ondragstart={(e) => {
                dragging = id;
                e.dataTransfer?.setData("text/plain", name);
                if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
            }}
            ondragend={() => {
                dragging = null;
                dropAt = null;
            }}
            ondragover={(e) => dragover(e, index)}
            ondrop={drop}
        >
            <button
                type="button"
                role="tab"
                class="label"
                data-tab={id}
                aria-selected={session.active === id}
                tabindex={session.active === id ? 0 : -1}
                title={name}
                onclick={() => onselect(id)}
                onauxclick={(e) => e.button === 1 && onclose(id)}
                onkeydown={(e) => keydown(e, id)}
            >
                {name}
            </button>
            <button
                type="button"
                class="close"
                tabindex="-1"
                aria-label={`Close ${name}`}
                title="Close tab (Ctrl+W)"
                onclick={() => onclose(id)}
            >
                ✕
            </button>
        </div>
    {/each}
</div>

<style>
    .bar {
        display: flex;
        gap: 2px;
        min-height: 2.25rem;
        padding: 0 0.25rem;
        overflow-x: auto;
        border-bottom: 1px solid var(--border);
        background: var(--surface);
        scrollbar-width: thin;
    }
    .tab {
        display: flex;
        flex-shrink: 0;
        align-items: center;
        max-width: 14rem;
        border-bottom: 2px solid transparent;
    }
    .tab.selected,
    .hub[aria-selected="true"] {
        border-bottom-color: var(--accent);
    }
    .tab.dragging {
        opacity: 0.5;
    }
    .drop-before {
        box-shadow: inset 2px 0 var(--accent);
    }
    .drop-after {
        box-shadow: inset -2px 0 var(--accent);
    }
    button {
        border: none;
        background: none;
        color: var(--text-muted);
        font: inherit;
    }
    .hub,
    .label {
        overflow: hidden;
        padding: 0.45rem 0.75rem;
        text-overflow: ellipsis;
        white-space: nowrap;
        cursor: pointer;
    }
    /* The only control that gives up width: a long project name is cut with an ellipsis. */
    .label {
        flex-shrink: 1;
        min-width: 0;
        padding-right: 0.25rem;
    }
    [aria-selected="true"] {
        color: var(--text);
    }
    .close {
        padding: 0.1rem 0.4rem;
        border-radius: 3px;
        font-size: 0.8em;
        cursor: pointer;
    }
    .close:hover {
        background: var(--surface-2);
        color: var(--text);
    }
</style>
