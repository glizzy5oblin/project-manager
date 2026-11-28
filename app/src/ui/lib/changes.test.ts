import { describe, expect, it } from "vitest";
import {
    changeRows,
    commitProblem,
    followSelection,
    kindLetter,
    replayControls,
} from "./changes";

const status = {
    staged: [
        { path: "src/b.rs", kind: "modified" as const },
        { path: "a.txt", kind: "added" as const },
    ],
    unstaged: [{ path: "src/b.rs", kind: "modified" as const }],
    untracked: ["new.txt", "0-first.txt"],
    conflicted: ["clash.txt"],
};

describe("changeRows", () => {
    it("lists conflicted, staged, then unstaged files with untracked ones sorted in", () => {
        expect(changeRows(status)).toEqual([
            { path: "clash.txt", kind: "conflicted", side: "conflicted" },
            { path: "src/b.rs", kind: "modified", side: "staged" },
            { path: "a.txt", kind: "added", side: "staged" },
            { path: "0-first.txt", kind: "untracked", side: "unstaged" },
            { path: "new.txt", kind: "untracked", side: "unstaged" },
            { path: "src/b.rs", kind: "modified", side: "unstaged" },
        ]);
    });

    it("is empty for a bare repository", () => {
        expect(changeRows(null)).toEqual([]);
    });
});

describe("followSelection", () => {
    const rows = changeRows(status);

    it("keeps a row that is still there, as the same object", () => {
        const sel = { path: "src/b.rs", side: "unstaged" as const };
        expect(followSelection(sel, rows)).toBe(sel);
    });

    it("follows a file into the other list after staging", () => {
        expect(
            followSelection({ path: "a.txt", side: "unstaged" }, rows),
        ).toEqual({ path: "a.txt", side: "staged" });
    });

    it("drops a file that is gone", () => {
        expect(followSelection({ path: "gone", side: "staged" }, rows)).toBe(
            null,
        );
        expect(followSelection(null, rows)).toBe(null);
    });
});

describe("commitProblem", () => {
    it("needs a message and staged changes, unless amending", () => {
        expect(commitProblem("", false, 1, 0)).toMatch(/message/);
        expect(commitProblem("  \n", true, 0, 0)).toMatch(/message/);
        expect(commitProblem("fix", false, 0, 0)).toMatch(/Stage/);
        expect(commitProblem("fix", true, 0, 0)).toBe(null);
        expect(commitProblem("fix", false, 2, 0)).toBe(null);
    });

    it("refuses while files are conflicted", () => {
        expect(commitProblem("merge", false, 3, 1)).toMatch(/conflicts/);
    });
});

describe("kindLetter", () => {
    it("gives one letter per kind", () => {
        expect(kindLetter("untracked")).toBe("U");
        expect(kindLetter("conflicted")).toBe("!");
    });
});

describe("replayControls", () => {
    it("offers Continue and the matching Abort for a cherry-pick, revert or rebase", () => {
        expect(replayControls("cherryPick")?.abort).toEqual({
            type: "abortCherryPick",
        });
        expect(replayControls("revert")?.abort).toEqual({
            type: "abortRevert",
        });
        expect(replayControls("rebase")?.abort).toEqual({
            type: "abortRebase",
        });
    });

    it("leaves merges and everything else alone", () => {
        for (const op of ["merge", "bisect", "applyMailbox", null] as const)
            expect(replayControls(op)).toBeNull();
    });
});
