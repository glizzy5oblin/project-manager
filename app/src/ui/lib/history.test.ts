import { describe, expect, it, vi } from "vitest";
import type { DiffLine } from "./bindings/DiffLine";
import type { GraphRow } from "./bindings/GraphRow";
import {
    CHUNK,
    RowCache,
    dayEnd,
    dayStart,
    formatOffset,
    graphColorVar,
    graphWidth,
    langForPath,
    lowerBound,
    mergeSegments,
    safeColor,
    scrollToShow,
    splitRows,
    visibleRange,
} from "./history";

function fakeRows(start: number, count: number): GraphRow[] {
    return Array.from(
        { length: count },
        (_, i) => ({ row: start + i }) as GraphRow,
    );
}

describe("RowCache", () => {
    it("loads a chunk once and serves rows from it", async () => {
        const load = vi.fn((s: number, c: number) =>
            Promise.resolve(fakeRows(s, c)),
        );
        const onload = vi.fn();
        const cache = new RowCache(load, onload, () => {});
        expect(cache.get(5)).toBeUndefined();
        expect(cache.get(7)).toBeUndefined();
        expect((await cache.row(5))?.row).toBe(5);
        expect(load).toHaveBeenCalledTimes(1);
        expect(load).toHaveBeenCalledWith(0, CHUNK);
        expect(cache.get(CHUNK - 1)?.row).toBe(CHUNK - 1);
        expect(onload).toHaveBeenCalledTimes(1);
        expect((await cache.row(CHUNK + 3))?.row).toBe(CHUNK + 3);
        expect(load).toHaveBeenLastCalledWith(CHUNK, CHUNK);
    });

    it("evicts the least recently used chunk", async () => {
        const load = vi.fn((s: number, c: number) =>
            Promise.resolve(fakeRows(s, c)),
        );
        const cache = new RowCache(
            load,
            () => {},
            () => {},
            2,
        );
        await cache.row(0);
        await cache.row(CHUNK);
        cache.get(0); // chunk 0 is now the most recent
        await cache.row(2 * CHUNK);
        expect(cache.size).toBe(2);
        expect(cache.get(0)).toBeDefined();
        expect(cache.get(CHUNK)).toBeUndefined();
    });

    it("reports errors and ignores results after close", async () => {
        const onerror = vi.fn();
        const failing = new RowCache(
            () => Promise.reject(new Error("stale")),
            () => {},
            onerror,
        );
        await failing.row(0);
        expect(onerror).toHaveBeenCalledTimes(1);

        let resolve!: (r: GraphRow[]) => void;
        const onload = vi.fn();
        const closing = new RowCache(
            () => new Promise((r) => (resolve = r)),
            onload,
            () => {},
        );
        const p = closing.row(0);
        closing.close();
        resolve(fakeRows(0, CHUNK));
        expect(await p).toBeUndefined();
        expect(onload).not.toHaveBeenCalled();
    });
});

describe("geometry", () => {
    it("cycles eight lane colors", () => {
        expect(graphColorVar(0)).toBe("--graph-1");
        expect(graphColorVar(7)).toBe("--graph-8");
        expect(graphColorVar(8)).toBe("--graph-1");
    });

    it("caps the graph width", () => {
        expect(graphWidth(2, 14)).toBe(40);
        expect(graphWidth(0, 14)).toBe(26);
        expect(graphWidth(500, 14)).toBe(320);
    });

    it("computes the visible rows and scroll targets", () => {
        expect(visibleRange(0, 100, 20, 1000)).toEqual([0, 5]);
        expect(visibleRange(50, 100, 20, 1000, 2)).toEqual([0, 10]);
        expect(visibleRange(0, 100, 20, 3)).toEqual([0, 2]);
        expect(visibleRange(0, 100, 20, 0)).toEqual([0, -1]);
        expect(scrollToShow(0, 100, 100, 20)).toBe(0);
        expect(scrollToShow(10, 0, 100, 20)).toBe(120);
        expect(scrollToShow(6, 100, 100, 20)).toBe(100);
    });

    it("binary searches", () => {
        expect(lowerBound([1, 3, 5], 3)).toBe(1);
        expect(lowerBound([1, 3, 5], 4)).toBe(2);
        expect(lowerBound([1, 3, 5], 9)).toBe(3);
        expect(lowerBound([], 1)).toBe(0);
    });
});

function line(kind: DiffLine["kind"], text: string): DiffLine {
    return {
        kind,
        text,
        oldLine: null,
        newLine: null,
        emphasis: [],
        noNewline: false,
    };
}

describe("splitRows", () => {
    it("pairs deletions with the additions after them", () => {
        const c = line("context", "c");
        const [d1, d2, a1] = [
            line("deleted", "d1"),
            line("deleted", "d2"),
            line("added", "a1"),
        ];
        const a2 = line("added", "a2");
        expect(splitRows([c, d1, d2, a1, c, a2])).toEqual([
            { left: c, right: c },
            { left: d1, right: a1 },
            { left: d2, right: null },
            { left: c, right: c },
            { left: null, right: a2 },
        ]);
    });
});

describe("mergeSegments", () => {
    const red = "var(--shiki-token-keyword)";
    const blue = "var(--shiki-token-string)";

    it("splits at token and emphasis boundaries", () => {
        const toks = [
            { content: "let ", color: red },
            { content: "abc", color: blue },
        ];
        expect(mergeSegments("let abc", toks, [[5, 6]])).toEqual([
            { text: "let ", color: red, emph: false },
            { text: "a", color: blue, emph: false },
            { text: "b", color: blue, emph: true },
            { text: "c", color: blue, emph: false },
        ]);
    });

    it("works without tokens and joins equal neighbours", () => {
        expect(
            mergeSegments("abcd", null, [
                [1, 2],
                [2, 3],
            ]),
        ).toEqual([
            { text: "a", color: null, emph: false },
            { text: "bc", color: null, emph: true },
            { text: "d", color: null, emph: false },
        ]);
        expect(mergeSegments("", null, [])).toEqual([]);
    });

    it("ignores tokens that do not spell the line", () => {
        expect(
            mergeSegments("abc", [{ content: "xyz", color: red }], []),
        ).toEqual([{ text: "abc", color: null, emph: false }]);
    });

    it("uses UTF-16 offsets", () => {
        expect(mergeSegments("😀 y", null, [[3, 4]])).toEqual([
            { text: "😀 ", color: null, emph: false },
            { text: "y", color: null, emph: true },
        ]);
    });

    it("lets only theme variables and hex colors through", () => {
        expect(safeColor(red)).toBe(red);
        expect(safeColor("#a1b2c3")).toBe("#a1b2c3");
        expect(safeColor("red; background: url(x)")).toBeNull();
        expect(safeColor("var(--x)")).toBeNull();
        expect(safeColor(undefined)).toBeNull();
    });
});

describe("formatting", () => {
    it("converts local days to epoch seconds", () => {
        const start = dayStart("2024-03-01")!;
        expect(new Date(start * 1000).getDate()).toBe(1);
        expect(dayEnd("2024-03-01")! - start).toBe(86399);
        expect(dayStart("bad")).toBeNull();
    });

    it("formats offsets", () => {
        expect(formatOffset(7200)).toBe("+0200");
        expect(formatOffset(-19800)).toBe("-0530");
        expect(formatOffset(0)).toBe("+0000");
    });

    it("picks a language by extension", () => {
        expect(langForPath("scripts/main.lua")).toBe("lua");
        expect(langForPath("Data/Items.JSON")).toBe("json");
        expect(langForPath("Makefile")).toBeNull();
        expect(langForPath(".gitignore")).toBeNull();
        expect(langForPath("a.unknown")).toBeNull();
    });
});
