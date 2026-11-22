import { describe, expect, it } from "vitest";
import {
    menuItems,
    menuPosition,
    nextEnabled,
    offerMenu,
    takeMenu,
} from "./contextMenu";

const commands = (editable: boolean, hasSelection: boolean) =>
    menuItems({ editable, hasSelection }).map(
        (i) => `${i.id}${i.disabled ? "!" : ""}`,
    );

describe("menuItems", () => {
    it("offers cut and paste only where text can be typed", () => {
        expect(commands(false, false)).toEqual(["copy!", "selectAll"]);
        expect(commands(true, false)).toEqual([
            "cut!",
            "copy!",
            "paste",
            "selectAll",
        ]);
    });

    it("enables cut and copy when something is selected", () => {
        expect(commands(false, true)).toEqual(["copy", "selectAll"]);
        expect(commands(true, true)).toEqual([
            "cut",
            "copy",
            "paste",
            "selectAll",
        ]);
    });
});

describe("menuPosition", () => {
    it("keeps a menu where it was opened when it fits", () => {
        expect(menuPosition(100, 80, 150, 90, 800, 600)).toEqual({
            x: 100,
            y: 80,
        });
    });

    it("pulls it back inside the window at the right and bottom edges", () => {
        expect(menuPosition(790, 590, 150, 90, 800, 600)).toEqual({
            x: 646,
            y: 506,
        });
    });

    it("never goes past the top left, even in a window smaller than the menu", () => {
        expect(menuPosition(-5, -5, 150, 90, 100, 50)).toEqual({ x: 4, y: 4 });
    });
});

describe("nextEnabled", () => {
    const item = (disabled: boolean) => ({
        id: "copy",
        label: "",
        keys: "",
        disabled,
    });
    const items = [item(true), item(false), item(true), item(false)];

    it("skips disabled items and wraps around", () => {
        expect(nextEnabled(items, -1, 1)).toBe(1);
        expect(nextEnabled(items, 1, 1)).toBe(3);
        expect(nextEnabled(items, 3, 1)).toBe(1);
        expect(nextEnabled(items, 1, -1)).toBe(3);
    });

    it("skips separators", () => {
        const sep = { ...item(false), separator: true };
        expect(nextEnabled([item(false), sep, item(false)], 0, 1)).toBe(2);
    });

    it("finds nothing when every item is disabled", () => {
        expect(nextEnabled([item(true), item(true)], -1, 1)).toBe(-1);
        expect(nextEnabled([], -1, 1)).toBe(-1);
    });
});

describe("offered menus", () => {
    const menu = { items: [], run: () => {} };

    it("goes to the event it was offered for, once", () => {
        const e = new Event("contextmenu");
        offerMenu(e, menu);
        expect(takeMenu(e)).toBe(menu);
        expect(takeMenu(e)).toBeNull();
    });

    it("is not used for another event", () => {
        offerMenu(new Event("contextmenu"), menu);
        expect(takeMenu(new Event("contextmenu"))).toBeNull();
    });
});
