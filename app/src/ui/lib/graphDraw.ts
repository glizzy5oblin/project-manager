// Draws the visible part of the commit graph onto a canvas. Rows are positioned by display
// index; each row's edges run from its centre to the next row's centre.

import type { GraphRow } from "./bindings/GraphRow";
import { graphColorVar, laneX } from "./history";

export type DrawOptions = {
    /** Display rows to draw, inclusive. */
    first: number;
    last: number;
    scrollTop: number;
    rowHeight: number;
    laneWidth: number;
    /** CSS pixel size of the canvas. */
    width: number;
    height: number;
    rowAt: (i: number) => GraphRow | undefined;
    selectedRow: number | null;
    headRow: number | null;
    /** Rows matching a search; others are drawn faded. Null when no search is active. */
    matches: ReadonlySet<number> | null;
};

/** Size the canvas backing store for the device pixel ratio and return a context in CSS pixels. */
export function prepareCanvas(
    canvas: HTMLCanvasElement,
    width: number,
    height: number,
): CanvasRenderingContext2D | null {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(width * dpr));
    const h = Math.max(1, Math.round(height * dpr));
    if (canvas.width !== w) canvas.width = w;
    if (canvas.height !== h) canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return ctx;
}

export function drawGraph(ctx: CanvasRenderingContext2D, o: DrawOptions) {
    const style = getComputedStyle(ctx.canvas);
    const colors = Array.from(
        { length: 8 },
        (_, i) => style.getPropertyValue(graphColorVar(i)).trim() || "#888",
    );
    const bg = style.getPropertyValue("--bg").trim() || "#000";
    const text = style.getPropertyValue("--text").trim() || "#fff";
    const color = (c: number) => colors[c % 8];
    const faded = (r: GraphRow) =>
        r.dim || (o.matches !== null && !o.matches.has(r.row));
    const h = o.rowHeight;
    const lw = o.laneWidth;
    const radius = Math.max(2.5, Math.min(h * 0.2, lw * 0.32));

    ctx.clearRect(0, 0, o.width, o.height);
    ctx.lineWidth = Math.max(1.5, Math.min(2.5, lw / 7));
    ctx.lineCap = "round";

    // Edges first (from the row above the viewport so lines enter from the top), nodes on top.
    for (let i = Math.max(0, o.first - 1); i <= o.last; i++) {
        const r = o.rowAt(i);
        if (!r) continue;
        const y1 = i * h + h / 2 - o.scrollTop;
        const y2 = y1 + h;
        ctx.globalAlpha = faded(r) ? 0.3 : 1;
        for (const e of r.edges) {
            const x1 = laneX(e.from, lw);
            const x2 = laneX(e.to, lw);
            ctx.strokeStyle = color(e.color);
            ctx.beginPath();
            ctx.moveTo(x1, y1);
            if (x1 === x2) {
                ctx.lineTo(x2, y2);
            } else {
                const mid = (y1 + y2) / 2;
                ctx.bezierCurveTo(x1, mid, x2, mid, x2, y2);
            }
            ctx.stroke();
        }
    }
    for (let i = o.first; i <= o.last; i++) {
        const r = o.rowAt(i);
        if (!r) continue;
        const x = laneX(r.lane, lw);
        const y = i * h + h / 2 - o.scrollTop;
        ctx.globalAlpha = faded(r) ? 0.3 : 1;
        ctx.beginPath();
        ctx.arc(x, y, radius, 0, Math.PI * 2);
        if (r.parents.length > 1) {
            // Merge commits: hollow.
            ctx.fillStyle = bg;
            ctx.fill();
            ctx.strokeStyle = color(r.color);
            ctx.stroke();
        } else {
            ctx.fillStyle = color(r.color);
            ctx.fill();
        }
        if (r.row === o.headRow || r.row === o.selectedRow) {
            ctx.beginPath();
            ctx.arc(x, y, radius + 2.5, 0, Math.PI * 2);
            ctx.strokeStyle = r.row === o.selectedRow ? text : color(r.color);
            ctx.stroke();
        }
    }
    ctx.globalAlpha = 1;
}
