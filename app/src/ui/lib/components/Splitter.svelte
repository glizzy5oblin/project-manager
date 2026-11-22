<script lang="ts">
    // A draggable divider between two panes. The size of the pane it resizes is a fraction of
    // `box` (the container), so it follows the window; it is remembered under `storageKey`.
    // Pointer drag, arrow keys (Shift for bigger steps, Home/End for the limits) and a double
    // click (back to the default, `null`) all work.
    import { untrack } from "svelte";
    import {
        type Axis,
        type From,
        limits,
        loadSize,
        lockSelection,
        saveSize,
        sizeFromKey,
        sizeFromPointer,
    } from "../paneSize";

    let {
        box,
        axis,
        from = "start",
        value = $bindable(null),
        current,
        min = 0.15,
        max = 0.7,
        minPx = 0,
        endMinPx = 0,
        label,
        storageKey,
    }: {
        /** The container the size is a fraction of. */
        box: HTMLElement | undefined;
        /** `x`: a vertical bar that sets a width. `y`: a horizontal bar that sets a height. */
        axis: Axis;
        /** The container edge the resized pane sits against. */
        from?: From;
        /** The chosen size; null while it is the default. */
        value?: number | null;
        /** The size now, as a fraction, for the first key press when `value` is null. */
        current: number;
        min?: number;
        max?: number;
        /** Pixels the resized pane always keeps, whatever the fraction says. */
        minPx?: number;
        /** Pixels the pane on the other side always keeps. */
        endMinPx?: number;
        label: string;
        storageKey: string;
    } = $props();

    // Take the remembered size once, then keep it up to date.
    let loaded = false;
    $effect.pre(() => {
        if (loaded) return;
        loaded = true;
        untrack(() => {
            if (value === null) value = loadSize(storageKey);
        });
    });
    $effect(() => {
        saveSize(storageKey, value);
    });

    const now = $derived(value ?? current);

    function drag(e: PointerEvent) {
        if (!box || e.button !== 0) return;
        const handle = e.currentTarget as HTMLElement;
        handle.setPointerCapture(e.pointerId);
        e.preventDefault();
        const unlock = lockSelection();
        const move = (ev: PointerEvent) => {
            const r = box.getBoundingClientRect();
            const [lo, hi] = limits(
                axis === "x" ? r.width : r.height,
                min,
                max,
                minPx,
                endMinPx,
            );
            value =
                axis === "x"
                    ? sizeFromPointer(ev.clientX, r.left, r.width, from, lo, hi)
                    : sizeFromPointer(
                          ev.clientY,
                          r.top,
                          r.height,
                          from,
                          lo,
                          hi,
                      );
        };
        const up = () => {
            unlock();
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
            handle.removeEventListener("pointercancel", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
    }

    function key(e: KeyboardEvent) {
        const r = box?.getBoundingClientRect();
        const [lo, hi] = limits(
            r ? (axis === "x" ? r.width : r.height) : 0,
            min,
            max,
            minPx,
            endMinPx,
        );
        const v = sizeFromKey(e, axis, from, now, lo, hi);
        if (v === null) return;
        value = v;
        e.preventDefault();
    }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (a focusable separator is the ARIA splitter pattern) -->
<div
    class="splitter {axis}"
    role="separator"
    aria-orientation={axis === "x" ? "vertical" : "horizontal"}
    aria-label={label}
    aria-valuenow={Math.round(now * 100)}
    aria-valuemin={Math.round(min * 100)}
    aria-valuemax={Math.round(max * 100)}
    title="Drag to resize, double-click to reset"
    tabindex="0"
    onpointerdown={drag}
    onkeydown={key}
    ondblclick={() => (value = null)}
></div>

<style>
    .splitter {
        position: relative;
        flex: none;
        touch-action: none;
        user-select: none;
    }
    .splitter.x {
        width: 6px;
        cursor: col-resize;
    }
    .splitter.y {
        height: 6px;
        cursor: row-resize;
    }
    /* The visible line is thin; the 6px around it is the target. */
    .splitter::after {
        position: absolute;
        background: var(--border);
        content: "";
    }
    .splitter.x::after {
        inset: 0 auto 0 50%;
        width: 1px;
    }
    .splitter.y::after {
        inset: 50% 0 auto;
        height: 1px;
    }
    .splitter:hover::after,
    .splitter:focus-visible::after {
        background: var(--accent);
    }
    .splitter.x:hover::after,
    .splitter.x:focus-visible::after {
        inset-inline: calc(50% - 1px) auto;
        width: 3px;
    }
    .splitter.y:hover::after,
    .splitter.y:focus-visible::after {
        inset-block: calc(50% - 1px) auto;
        height: 3px;
    }
    .splitter:focus-visible {
        outline: none;
    }
</style>
