import { describe, expect, it } from "vitest";
import { changeMenu, ignorePatterns } from "./changeMenu";

const ids = (side: "staged" | "unstaged" | "conflicted", busy = false) =>
    changeMenu({ path: "a/b.txt", side, busy })
        .filter((i) => !i.separator)
        .map((i) => i.id);

describe("changeMenu", () => {
    it("offers what fits each list", () => {
        expect(ids("staged")).toEqual([
            "unstage",
            "open-file",
            "show-folder",
            "copy-path",
            "copy-name",
        ]);
        expect(ids("unstaged")).toEqual([
            "stage",
            "discard",
            "open-file",
            "show-folder",
            "copy-path",
            "copy-name",
        ]);
        expect(ids("conflicted")).toEqual([
            "resolved",
            "show-folder",
            "copy-path",
            "copy-name",
        ]);
    });

    it("greys out write actions while busy but keeps copying", () => {
        const m = changeMenu({
            path: "x",
            side: "unstaged",
            busy: true,
        });
        expect(m.find((i) => i.id === "stage")?.disabled).toBe(true);
        expect(m.find((i) => i.id === "copy-path")?.disabled).toBe(false);
    });

    it("marks discard as dangerous", () => {
        const m = changeMenu({
            path: "x",
            side: "unstaged",
            busy: false,
        });
        expect(m.find((i) => i.id === "discard")?.danger).toBe(true);
    });
});

describe("ignore and open in Files", () => {
    it("offers ignoring only for untracked files", () => {
        const m = (untracked: boolean) =>
            changeMenu({
                path: "logs/a.log",
                side: "unstaged",
                busy: false,
                untracked,
            })
                .map((i) => i.id)
                .filter(Boolean);
        expect(m(true)).toEqual(
            expect.arrayContaining(["ignore-file", "ignore-ext", "open-file"]),
        );
        expect(m(false)).not.toContain("ignore-file");
    });

    it("does not offer opening a conflicted file in Files", () => {
        const m = changeMenu({ path: "a", side: "conflicted", busy: false });
        expect(m.map((i) => i.id)).not.toContain("open-file");
    });

    it("builds the lines to add", () => {
        expect(ignorePatterns("logs/a.log")).toEqual({
            file: "/logs/a.log",
            ext: "*.log",
        });
        expect(ignorePatterns("Makefile").ext).toBeNull();
        expect(ignorePatterns(".env").ext).toBeNull();
    });
});
