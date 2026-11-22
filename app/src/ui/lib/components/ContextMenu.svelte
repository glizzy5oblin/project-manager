<script lang="ts">
    // Replaces the webview's right-click menu with clipboard commands for the text under the
    // pointer. Mounted once, in App.svelte.
    import { tick } from "svelte";
    import {
        type CustomMenu,
        type MenuCommand,
        type MenuItem,
        menuItems,
        menuPosition,
        nextEnabled,
        runCommand,
        selectedText,
        takeMenu,
        textField,
    } from "../contextMenu";

    let items = $state<MenuItem[]>([]);
    let open = $state(false);
    let pos = $state({ x: 0, y: 0 });
    let active = $state(-1);
    let menu = $state<HTMLDivElement>();
    // The text field the menu was opened on, if any.
    let field: ReturnType<typeof textField> = null;
    // What choosing an item does: a view's own menu, or the clipboard commands.
    let run: CustomMenu["run"] = () => {};

    async function show(e: MouseEvent) {
        // Our own menu everywhere, so the webview's never shows.
        e.preventDefault();
        const custom = takeMenu(e);
        if (custom) {
            field = null;
            items = custom.items;
            run = custom.run;
        } else {
            field = textField(e.target);
            const editable =
                field !== null && !field.readOnly && !field.disabled;
            items = menuItems({
                editable,
                hasSelection: selectedText(field) !== "",
            });
            const f = field;
            run = (id) => void runCommand(id as MenuCommand, f);
        }
        pos = { x: e.clientX, y: e.clientY };
        active = -1;
        open = true;
        await tick();
        if (!menu) return;
        pos = menuPosition(
            e.clientX,
            e.clientY,
            menu.offsetWidth,
            menu.offsetHeight,
            window.innerWidth,
            window.innerHeight,
        );
    }

    function close() {
        open = false;
    }

    async function choose(item: MenuItem) {
        if (item.disabled) return;
        close();
        try {
            await run(item.id);
        } catch {
            // The clipboard can refuse (no permission, nothing to paste); nothing else to do.
        }
    }

    function keydown(e: KeyboardEvent) {
        if (!open) return;
        if (e.key === "Escape") {
            // Only the menu closes, not whatever is behind it.
            e.preventDefault();
            close();
        } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            active = nextEnabled(items, active, e.key === "ArrowDown" ? 1 : -1);
        } else if (e.key === "Home" || e.key === "End") {
            active =
                e.key === "Home"
                    ? nextEnabled(items, -1, 1)
                    : nextEnabled(items, 0, -1);
        } else if ((e.key === "Enter" || e.key === " ") && active >= 0) {
            void choose(items[active]);
        } else if (e.key === "Tab") {
            close();
            return;
        } else {
            return;
        }
        e.preventDefault();
        e.stopPropagation();
    }
</script>

<svelte:window
    oncontextmenu={show}
    onkeydowncapture={keydown}
    onmousedowncapture={(e) => {
        if (open && !menu?.contains(e.target as Node)) close();
    }}
    onblur={close}
    onresize={close}
    onwheel={close}
/>

{#if open}
    <!-- mousedown is kept off the menu so the selection and focus of the field stay put. -->
    <div
        class="menu"
        role="menu"
        tabindex="-1"
        bind:this={menu}
        style:left="{pos.x}px"
        style:top="{pos.y}px"
        onmousedown={(e) => e.preventDefault()}
    >
        {#each items as item, i (item.id)}
            {#if item.separator}
                <hr />
            {:else}
                <button
                    type="button"
                    role="menuitem"
                    class:active={i === active}
                    class:danger={item.danger}
                    disabled={item.disabled}
                    title={item.hint}
                    onmouseenter={() => (active = item.disabled ? -1 : i)}
                    onclick={() => choose(item)}
                >
                    <span>{item.label}</span>
                    {#if item.keys}<kbd>{item.keys}</kbd>{/if}
                </button>
            {/if}
        {/each}
    </div>
{/if}

<style>
    .menu {
        position: fixed;
        z-index: 1000;
        min-width: 11rem;
        max-width: min(28rem, calc(100vw - 8px));
        max-height: calc(100vh - 8px);
        overflow-y: auto;
        padding: 0.25rem;
        border: 1px solid var(--border);
        border-radius: 8px;
        background: var(--surface-2);
        box-shadow: 0 6px 20px rgb(0 0 0 / 35%);
    }
    button {
        display: flex;
        justify-content: space-between;
        gap: 1.5rem;
        width: 100%;
        padding: 0.35rem 0.6rem;
        border: 0;
        border-radius: 5px;
        background: transparent;
        color: var(--text);
        font: inherit;
        text-align: left;
        cursor: default;
    }
    button.active:not(:disabled) {
        background: var(--accent);
        color: var(--accent-text);
    }
    button:disabled {
        color: var(--text-muted);
        opacity: 0.6;
    }
    button.danger:not(.active) {
        color: var(--danger);
    }
    hr {
        margin: 0.25rem 0.2rem;
        border: 0;
        border-top: 1px solid var(--border);
    }
    kbd {
        font: inherit;
        font-size: 0.85em;
        opacity: 0.7;
    }
</style>
