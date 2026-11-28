import { describe, expect, it } from "vitest";
import type { Session } from "./bindings/Session";
import {
    closeTab,
    cycle,
    moveTab,
    openTab,
    pruneTabs,
    setSide,
    setView,
} from "./tabs";

const tabs = (...ids: string[]): Session => ({
    tabs: ids.map((projectId) => ({
        projectId,
        view: "overview",
        side: "private",
    })),
    active: null,
});
const order = (s: Session) => s.tabs.map((t) => t.projectId);

describe("tabs", () => {
    it("opens at the end, or shows an open tab without duplicating it", () => {
        let s = openTab(tabs("a"), "b");
        expect(order(s)).toEqual(["a", "b"]);
        expect(s.active).toBe("b");
        s = openTab(s, "a");
        expect(order(s)).toEqual(["a", "b"]);
        expect(s.active).toBe("a");
    });

    it("closing the showing tab shows its right neighbour, then left, then the hub", () => {
        let s: Session = { ...tabs("a", "b", "c"), active: "b" };
        s = closeTab(s, "b");
        expect(s.active).toBe("c");
        s = closeTab(s, "c");
        expect(s.active).toBe("a");
        s = closeTab(s, "a");
        expect(s).toEqual({ tabs: [], active: null });
    });

    it("closing another tab keeps the showing one", () => {
        const s = closeTab({ ...tabs("a", "b"), active: "b" }, "a");
        expect(s.active).toBe("b");
        expect(closeTab(s, "zz")).toBe(s);
    });

    it("moves tabs with clamping", () => {
        const s = tabs("a", "b", "c");
        expect(order(moveTab(s, "c", 0))).toEqual(["c", "a", "b"]);
        expect(order(moveTab(s, "a", 99))).toEqual(["b", "c", "a"]);
        expect(order(moveTab(s, "b", -5))).toEqual(["b", "a", "c"]);
        expect(order(s)).toEqual(["a", "b", "c"]);
    });

    it("cycles through the hub and the tabs, wrapping", () => {
        let s = tabs("a", "b");
        s = cycle(s, 1);
        expect(s.active).toBe("a");
        s = cycle(s, 2);
        expect(s.active).toBe(null);
        s = cycle(s, -1);
        expect(s.active).toBe("b");
    });

    it("keeps a view per tab", () => {
        const s = setView(tabs("a", "b"), "b", "graph");
        expect(s.tabs.map((t) => t.view)).toEqual(["overview", "graph"]);
    });

    it("opens tabs on the private side and keeps a side per tab", () => {
        const opened = openTab(tabs(), "a");
        expect(opened.tabs.map((t) => t.side)).toEqual(["private"]);
        const s = setSide(tabs("a", "b"), "b", "public");
        expect(s.tabs.map((t) => t.side)).toEqual(["private", "public"]);
    });

    it("prunes tabs of removed projects", () => {
        const s = pruneTabs({ ...tabs("a", "b"), active: "b" }, new Set(["a"]));
        expect(s).toEqual({ ...tabs("a"), active: "a" });
    });
});
