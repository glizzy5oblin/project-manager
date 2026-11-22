import { describe, expect, it } from "vitest";
import type { GraphRow } from "./bindings/GraphRow";
import { graphShapes } from "./graphShapes";
import { laneX } from "./history";

function row(over: Partial<GraphRow> & { row: number }): GraphRow {
    return {
        id: `id${over.row}`,
        lane: 0,
        color: 0,
        edges: [],
        refs: [],
        branched: [],
        parents: ["p"],
        dim: false,
        ...over,
    } as GraphRow;
}

function shapes(rows: GraphRow[], extra: object = {}) {
    return graphShapes({
        first: 0,
        last: rows.length - 1,
        rowHeight: 26,
        laneWidth: 14,
        rowAt: (i) => rows[i],
        selectedRow: null,
        headRow: null,
        matches: null,
        ...extra,
    });
}

describe("graphShapes", () => {
    it("runs a straight edge from one row's centre to the next", () => {
        const s = shapes([
            row({ row: 0, edges: [{ from: 0, to: 0, color: 0 }] }),
        ]);
        const x = laneX(0, 14);
        expect(s.edges[0].d).toBe(`M${x} 13L${x} 39`);
        expect(s.edges[0].color).toBe("var(--graph-1)");
    });

    it("curves an edge that changes lane", () => {
        const s = shapes([
            row({ row: 0, edges: [{ from: 0, to: 1, color: 1 }] }),
        ]);
        expect(s.edges[0].d).toBe(
            `M${laneX(0, 14)} 13C${laneX(0, 14)} 26 ${laneX(1, 14)} 26 ${laneX(1, 14)} 39`,
        );
    });

    it("places rows relative to the first row drawn, so scrolling does not move them", () => {
        const rows = [row({ row: 5 }), row({ row: 6 })];
        const s = graphShapes({
            first: 5,
            last: 6,
            rowHeight: 26,
            laneWidth: 14,
            rowAt: (i) => rows[i - 5],
            selectedRow: null,
            headRow: null,
            matches: null,
        });
        expect(s.nodes.map((n) => n.cy)).toEqual([13, 39]);
        expect(s.height).toBe(3 * 26);
    });

    it("draws merge commits hollow and rings the selected row over HEAD", () => {
        const s = shapes(
            [row({ row: 0, parents: ["a", "b"] }), row({ row: 1 })],
            { selectedRow: 0, headRow: 0 },
        );
        expect(s.nodes[0]).toMatchObject({ hollow: true, ring: "selected" });
        expect(s.nodes[1]).toMatchObject({ hollow: false, ring: null });
        expect(shapes([row({ row: 0 })], { headRow: 0 }).nodes[0].ring).toBe(
            "head",
        );
    });

    it("fades dimmed rows and rows outside a search", () => {
        const s = shapes([row({ row: 0, dim: true }), row({ row: 1 })]);
        expect(s.nodes.map((n) => n.faded)).toEqual([true, false]);
        const m = shapes([row({ row: 0 }), row({ row: 1 })], {
            matches: new Set([1]),
        });
        expect(m.nodes.map((n) => n.faded)).toEqual([true, false]);
    });

    it("skips rows that are not loaded yet", () => {
        const s = graphShapes({
            first: 0,
            last: 2,
            rowHeight: 26,
            laneWidth: 14,
            rowAt: (i) => (i === 1 ? row({ row: 1 }) : undefined),
            selectedRow: null,
            headRow: null,
            matches: null,
        });
        expect(s.nodes).toHaveLength(1);
        expect(s.nodes[0].cy).toBe(39);
    });
});
