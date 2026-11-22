import { describe, expect, it } from "vitest";
import { findAll, lineOf, pickMatch } from "./fileSearch";

describe("fileSearch", () => {
    it("finds matches, ignoring case unless asked", () => {
        expect(findAll("Abc abc ABC", "abc", false)).toEqual([0, 4, 8]);
        expect(findAll("Abc abc ABC", "abc", true)).toEqual([4]);
        expect(findAll("aaaa", "aa", true)).toEqual([0, 2]);
        expect(findAll("x", "", false)).toEqual([]);
    });

    it("goes to the next or previous match and wraps", () => {
        const m = [3, 10, 20];
        expect(pickMatch(m, 0, true)).toBe(3);
        expect(pickMatch(m, 4, true)).toBe(10);
        expect(pickMatch(m, 21, true)).toBe(3);
        expect(pickMatch(m, 10, false)).toBe(3);
        expect(pickMatch(m, 3, false)).toBe(20);
        expect(pickMatch([], 0, true)).toBeNull();
    });

    it("counts lines", () => {
        expect(lineOf("a\nb\nc", 0)).toBe(0);
        expect(lineOf("a\nb\nc", 2)).toBe(1);
        expect(lineOf("a\nb\nc", 4)).toBe(2);
    });
});
