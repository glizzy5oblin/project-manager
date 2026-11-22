import { describe, expect, it } from "vitest";
import {
    clamp,
    limits,
    columnFromKey,
    columnFromPointer,
    loadSize,
    saveSize,
    sizeFromKey,
    sizeFromPointer,
} from "./paneSize";

const key = (key: string, shiftKey = false) => ({ key, shiftKey });

describe("sizeFromPointer", () => {
    it("measures a pane at the start from the start edge", () => {
        expect(sizeFromPointer(300, 100, 800, "start", 0.1, 0.7)).toBe(0.25);
    });

    it("measures a pane at the end from the end edge", () => {
        expect(sizeFromPointer(700, 100, 800, "end", 0.1, 0.7)).toBe(0.25);
    });

    it("stays between the limits, wherever the pointer goes", () => {
        expect(sizeFromPointer(-50, 100, 800, "start", 0.1, 0.7)).toBe(0.1);
        expect(sizeFromPointer(5000, 100, 800, "start", 0.1, 0.7)).toBe(0.7);
    });

    it("gives the minimum for a container with no size", () => {
        expect(sizeFromPointer(10, 0, 0, "start", 0.1, 0.7)).toBe(0.1);
    });
});

describe("sizeFromKey", () => {
    it("grows a start pane when the handle moves toward the end", () => {
        expect(
            sizeFromKey(key("ArrowRight"), "x", "start", 0.3, 0.1, 0.7),
        ).toBeCloseTo(0.32);
        expect(
            sizeFromKey(key("ArrowLeft"), "x", "start", 0.3, 0.1, 0.7),
        ).toBeCloseTo(0.28);
    });

    it("shrinks an end pane when the handle moves toward the end", () => {
        expect(
            sizeFromKey(key("ArrowDown"), "y", "end", 0.3, 0.1, 0.7),
        ).toBeCloseTo(0.28);
        expect(
            sizeFromKey(key("ArrowUp"), "y", "end", 0.3, 0.1, 0.7),
        ).toBeCloseTo(0.32);
    });

    it("takes bigger steps with Shift and stops at the limits", () => {
        expect(
            sizeFromKey(key("ArrowRight", true), "x", "start", 0.3, 0.1, 0.7),
        ).toBeCloseTo(0.4);
        expect(
            sizeFromKey(key("ArrowRight", true), "x", "start", 0.68, 0.1, 0.7),
        ).toBe(0.7);
        expect(sizeFromKey(key("Home"), "x", "start", 0.3, 0.1, 0.7)).toBe(0.1);
        expect(sizeFromKey(key("End"), "x", "start", 0.3, 0.1, 0.7)).toBe(0.7);
    });

    it("ignores keys of the other axis and other keys", () => {
        expect(
            sizeFromKey(key("ArrowUp"), "x", "start", 0.3, 0.1, 0.7),
        ).toBeNull();
        expect(sizeFromKey(key("a"), "y", "start", 0.3, 0.1, 0.7)).toBeNull();
    });
});

describe("remembered sizes", () => {
    const memory = () => {
        const m = new Map<string, string>();
        return {
            getItem: (k: string) => m.get(k) ?? null,
            setItem: (k: string, v: string) => void m.set(k, v),
            removeItem: (k: string) => void m.delete(k),
        };
    };

    it("round-trips and forgets", () => {
        const s = memory();
        saveSize("a", 0.3333333, s);
        expect(loadSize("a", s)).toBe(0.333);
        saveSize("a", null, s);
        expect(loadSize("a", s)).toBeNull();
    });

    it("ignores values that are not a fraction", () => {
        const s = memory();
        for (const bad of ["", "abc", "0", "1", "-0.2", "7", "NaN"]) {
            s.setItem("pm.pane.x", bad);
            expect(loadSize("x", s), bad).toBeNull();
        }
    });

    it("survives storage that throws", () => {
        const broken = {
            getItem: () => {
                throw new Error("blocked");
            },
            setItem: () => {
                throw new Error("full");
            },
            removeItem: () => {
                throw new Error("blocked");
            },
        };
        expect(loadSize("a", broken)).toBeNull();
        expect(() => saveSize("a", 0.5, broken)).not.toThrow();
    });
});

describe("clamp", () => {
    it("limits both ways", () => {
        expect(clamp(5, 0, 3)).toBe(3);
        expect(clamp(-1, 0, 3)).toBe(0);
        expect(clamp(2, 0, 3)).toBe(2);
    });
});

describe("columnFromPointer", () => {
    it("widens a column when its left edge is dragged left", () => {
        expect(columnFromPointer(160, 500, 460, 60, 400)).toBe(200);
        expect(columnFromPointer(160, 500, 540, 60, 400)).toBe(120);
    });

    it("stays between the limits", () => {
        expect(columnFromPointer(160, 500, 0, 60, 400)).toBe(400);
        expect(columnFromPointer(160, 500, 900, 60, 400)).toBe(60);
    });
});

describe("columnFromKey", () => {
    it("left widens and right narrows, more with Shift", () => {
        expect(columnFromKey(key("ArrowLeft"), 160, 60, 400)).toBe(168);
        expect(columnFromKey(key("ArrowRight"), 160, 60, 400)).toBe(152);
        expect(columnFromKey(key("ArrowLeft", true), 160, 60, 400)).toBe(192);
    });

    it("goes to the limits with Home and End and ignores other keys", () => {
        expect(columnFromKey(key("Home"), 160, 60, 400)).toBe(60);
        expect(columnFromKey(key("End"), 160, 60, 400)).toBe(400);
        expect(columnFromKey(key("ArrowUp"), 160, 60, 400)).toBeNull();
    });
});

describe("pixel sizes", () => {
    it("are loaded within the bounds given", () => {
        const s = { getItem: () => "245" };
        expect(loadSize("c", s, 0, 4000)).toBe(245);
        expect(loadSize("c", s)).toBeNull();
        expect(loadSize("c", { getItem: () => "5000" }, 0, 4000)).toBeNull();
    });
});

describe("limits", () => {
    it("keeps the pixel floors of both panes", () => {
        // 1000px: the pane keeps 250px (0.25) and the other side keeps 400px (so at most 0.6).
        expect(limits(1000, 0.1, 0.9, 250, 400)).toEqual([0.25, 0.6]);
    });
    it("keeps the fractional limits when they are stricter", () => {
        expect(limits(1000, 0.3, 0.5, 100, 100)).toEqual([0.3, 0.5]);
    });
    it("lets the pane keep its own floor when the container is too small for both", () => {
        const [lo, hi] = limits(500, 0.1, 0.9, 300, 300);
        expect(lo).toBeCloseTo(0.6);
        expect(hi).toBeCloseTo(0.6);
    });
    it("falls back to the fractions with no size yet", () => {
        expect(limits(0, 0.2, 0.7, 300, 300)).toEqual([0.2, 0.7]);
    });
});
