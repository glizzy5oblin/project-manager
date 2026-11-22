// Sizes of resizable panes. A size is a fraction of the container (so it follows the window), kept
// between a minimum and a maximum, and remembered per pane in the webview's local storage.

export type Axis = "x" | "y";
/** Which edge the sized pane sits against: its size is measured from there. */
export type From = "start" | "end";

export function clamp(v: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, v));
}

/**
 * The smallest and largest fraction a pane may take so that it keeps `minPx` pixels and the pane
 * on the other side keeps `endMinPx`, in a container `size` pixels long. `min` and `max` still
 * apply; when the container is too small for both floors the pane keeps its own floor.
 */
export function limits(
    size: number,
    min: number,
    max: number,
    minPx = 0,
    endMinPx = 0,
): [number, number] {
    if (size <= 0) return [min, max];
    const lo = Math.max(min, minPx / size);
    const hi = Math.min(max, 1 - endMinPx / size);
    return [lo, Math.max(lo, hi)];
}

/** The size for a pointer at `pos`, in a container that starts at `start` and is `size` long. */
export function sizeFromPointer(
    pos: number,
    start: number,
    size: number,
    from: From,
    min: number,
    max: number,
): number {
    if (size <= 0) return min;
    const p = (pos - start) / size;
    return clamp(from === "start" ? p : 1 - p, min, max);
}

/**
 * The size after an arrow key, or null for a key that does nothing. Arrow keys along the axis
 * move the handle by `step` (`big` with Shift); Home and End go to the extremes.
 */
export function sizeFromKey(
    e: { key: string; shiftKey: boolean },
    axis: Axis,
    from: From,
    value: number,
    min: number,
    max: number,
    step = 0.02,
    big = 0.1,
): number | null {
    const [back, forward] =
        axis === "x" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"];
    if (e.key === "Home") return min;
    if (e.key === "End") return max;
    if (e.key !== back && e.key !== forward) return null;
    // Moving the handle toward the end grows a pane at the start and shrinks one at the end.
    const dir = (e.key === forward ? 1 : -1) * (from === "start" ? 1 : -1);
    return clamp(value + dir * (e.shiftKey ? big : step), min, max);
}

/**
 * Stop text from being selected while a handle is dragged, and clear what the press selected.
 * Preventing the default of a pointer event does not do this in every webview. Returns the
 * function that lets selection work again.
 */
export function lockSelection(): () => void {
    const style = document.body.style;
    const before = style.userSelect;
    style.userSelect = "none";
    getSelection()?.removeAllRanges();
    return () => {
        style.userSelect = before;
    };
}

const PREFIX = "pm.pane.";

/** A column's width in pixels after dragging its left edge from `startX` to `x`. */
export function columnFromPointer(
    startWidth: number,
    startX: number,
    x: number,
    min: number,
    max: number,
): number {
    // The column sits against the right: its left edge moving left makes it wider.
    return clamp(Math.round(startWidth - (x - startX)), min, max);
}

/** A column's width after an arrow key (left widens, right narrows), or null for other keys. */
export function columnFromKey(
    e: { key: string; shiftKey: boolean },
    width: number,
    min: number,
    max: number,
    step = 8,
    big = 32,
): number | null {
    if (e.key === "Home") return min;
    if (e.key === "End") return max;
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return null;
    const d = (e.shiftKey ? big : step) * (e.key === "ArrowLeft" ? 1 : -1);
    return clamp(Math.round(width + d), min, max);
}

/**
 * A remembered size, or null when none was saved or the saved value is not usable: it must lie
 * above `lo` and below `hi` (a fraction by default).
 */
export function loadSize(
    key: string,
    store: Pick<Storage, "getItem"> | undefined = safeStorage(),
    lo = 0,
    hi = 1,
): number | null {
    try {
        const raw = store?.getItem(PREFIX + key);
        if (raw === null || raw === undefined) return null;
        const v = Number(raw);
        return Number.isFinite(v) && v > lo && v < hi ? v : null;
    } catch {
        return null;
    }
}

/** Remember a size; null forgets it. Storage that refuses is ignored: the size just resets. */
export function saveSize(
    key: string,
    value: number | null,
    store: Pick<Storage, "setItem" | "removeItem"> | undefined = safeStorage(),
): void {
    try {
        if (value === null) store?.removeItem(PREFIX + key);
        else
            store?.setItem(
                PREFIX + key,
                String(Math.round(value * 1000) / 1000),
            );
    } catch {
        // Storage can be full or blocked; nothing to do.
    }
}

function safeStorage(): Storage | undefined {
    try {
        return typeof localStorage === "undefined" ? undefined : localStorage;
    } catch {
        return undefined;
    }
}
