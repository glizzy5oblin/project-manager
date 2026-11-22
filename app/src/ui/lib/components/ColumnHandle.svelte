<script lang="ts">
    // A drag handle on the left edge of a column header, for a column that sits against the
    // right edge of the list (its width is set in pixels, the column before it takes the rest).
    // Pointer drag, arrow keys (left widens, right narrows; Shift for bigger steps) and a double
    // click (back to the default, `null`) work. The width is remembered under `storageKey`.
    import { untrack } from "svelte";
    import {
        columnFromKey,
        columnFromPointer,
        loadSize,
        lockSelection,
        saveSize,
    } from "../paneSize";

    let {
        value = $bindable(null),
        min,
        max,
        label,
        storageKey,
        edge = "left",
    }: {
        /** Which edge of the header cell the handle is on. A column at the left of the list has its
         *  handle on the right edge, and dragging right widens it. */
        edge?: "left" | "right";
        /** Width in pixels; null while it is the default. */
        value?: number | null;
        min: number;
        max: number;
        label: string;
        storageKey: string;
    } = $props();

    let loaded = false;
    $effect.pre(() => {
        if (loaded) return;
        loaded = true;
        untrack(() => {
            if (value === null)
                value = loadSize(storageKey, undefined, 0, 4000);
        });
    });
    $effect(() => {
        saveSize(storageKey, value);
    });

    /** The column's width now: the header cell the handle sits in. */
    const widthOf = (el: HTMLElement) =>
        el.parentElement?.getBoundingClientRect().width ?? min;

    function drag(e: PointerEvent) {
        if (e.button !== 0) return;
        const handle = e.currentTarget as HTMLElement;
        const startW = widthOf(handle);
        const startX = e.clientX;
        handle.setPointerCapture(e.pointerId);
        e.preventDefault();
        e.stopPropagation();
        const unlock = lockSelection();
        const move = (ev: PointerEvent) => {
            value =
                edge === "right"
                    ? Math.min(
                          max,
                          Math.max(
                              min,
                              Math.round(startW + ev.clientX - startX),
                          ),
                      )
                    : columnFromPointer(startW, startX, ev.clientX, min, max);
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
        // At the right edge the arrows go the other way round.
        const flip = edge === "right";
        const v = columnFromKey(
            flip && (e.key === "ArrowLeft" || e.key === "ArrowRight")
                ? {
                      ...e,
                      key: e.key === "ArrowLeft" ? "ArrowRight" : "ArrowLeft",
                  }
                : e,
            widthOf(e.currentTarget as HTMLElement),
            min,
            max,
        );
        if (v === null) return;
        value = v;
        e.preventDefault();
    }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (a focusable separator is the ARIA splitter pattern) -->
<div
    class="handle"
    class:right={edge === "right"}
    role="separator"
    aria-orientation="vertical"
    aria-label={label}
    aria-valuenow={value ?? undefined}
    aria-valuemin={min}
    aria-valuemax={max}
    title="Drag to resize, double-click to reset"
    tabindex="0"
    onpointerdown={drag}
    onkeydown={key}
    ondblclick={() => (value = null)}
></div>

<style>
    /* Sits in the gap before the column; the line shows on hover and focus. */
    .handle {
        position: absolute;
        top: -0.25rem;
        bottom: -0.25rem;
        left: -0.5rem;
        width: 0.6rem;
        cursor: col-resize;
        touch-action: none;
        user-select: none;
    }
    .handle.right {
        right: -0.1rem;
        left: auto;
    }
    .handle::after {
        position: absolute;
        inset: 0.2rem auto 0.2rem 50%;
        width: 1px;
        background: var(--border);
        content: "";
    }
    .handle:hover::after,
    .handle:focus-visible::after {
        width: 3px;
        margin-left: -1px;
        background: var(--accent);
    }
    .handle:focus-visible {
        outline: none;
    }
</style>
