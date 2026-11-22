// Geometry of the commit graph for a run of rows, as plain data the list renders as SVG inside
// its scrolled content. Rows are positioned by display index; each row's edges run from its
// centre to the next row's centre. Coordinates are CSS pixels relative to the top of row `first`,
// so the shapes only change when the row range does, never while the list scrolls.

import type { GraphRow } from "./bindings/GraphRow";
import { graphColorVar, laneX } from "./history";

export type ShapeOptions = {
    /** Display rows to draw, inclusive. */
    first: number;
    last: number;
    rowHeight: number;
    laneWidth: number;
    rowAt: (i: number) => GraphRow | undefined;
    selectedRow: number | null;
    headRow: number | null;
    /** Rows matching a search; others are drawn faded. Null when no search is active. */
    matches: ReadonlySet<number> | null;
};

export type EdgeShape = {
    /** SVG path data. */
    d: string;
    /** CSS color, a theme variable. */
    color: string;
    faded: boolean;
};

export type NodeShape = {
    cx: number;
    cy: number;
    r: number;
    color: string;
    /** Merge commits are hollow. */
    hollow: boolean;
    faded: boolean;
    /** Ring drawn around the node: the selected commit, or HEAD. */
    ring: "selected" | "head" | null;
};

export type GraphShapes = {
    edges: EdgeShape[];
    nodes: NodeShape[];
    /** Stroke width for edges and hollow nodes. */
    lineWidth: number;
    /** Extra radius of the selection/HEAD ring. */
    ringGap: number;
    /** Height of the drawing: the last row's edges end one row below its centre. */
    height: number;
};

const colorOf = (c: number) => `var(${graphColorVar(c)})`;

export function graphShapes(o: ShapeOptions): GraphShapes {
    const h = o.rowHeight;
    const lw = o.laneWidth;
    const faded = (r: GraphRow) =>
        r.dim || (o.matches !== null && !o.matches.has(r.row));
    const edges: EdgeShape[] = [];
    const nodes: NodeShape[] = [];

    for (let i = o.first; i <= o.last; i++) {
        const r = o.rowAt(i);
        if (!r) continue;
        const y1 = (i - o.first) * h + h / 2;
        const y2 = y1 + h;
        for (const e of r.edges) {
            const x1 = laneX(e.from, lw);
            const x2 = laneX(e.to, lw);
            let d = `M${x1} ${y1}`;
            if (x1 === x2) {
                d += `L${x2} ${y2}`;
            } else {
                const mid = (y1 + y2) / 2;
                d += `C${x1} ${mid} ${x2} ${mid} ${x2} ${y2}`;
            }
            edges.push({ d, color: colorOf(e.color), faded: faded(r) });
        }
        nodes.push({
            cx: laneX(r.lane, lw),
            cy: y1,
            r: Math.max(2.5, Math.min(h * 0.2, lw * 0.32)),
            color: colorOf(r.color),
            hollow: r.parents.length > 1,
            faded: faded(r),
            ring:
                r.row === o.selectedRow
                    ? "selected"
                    : r.row === o.headRow
                      ? "head"
                      : null,
        });
    }
    return {
        edges,
        nodes,
        lineWidth: Math.max(1.5, Math.min(2.5, lw / 7)),
        ringGap: 2.5,
        height: Math.max(0, o.last - o.first + 2) * h,
    };
}
