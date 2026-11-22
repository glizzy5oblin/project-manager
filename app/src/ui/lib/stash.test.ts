import { describe, expect, it } from "vitest";
import type { StashEntry } from "./bindings/StashEntry";
import { gitAtLeast, stashMeta, stashStillListed, stashTitle } from "./stash";

const entry = (over: Partial<StashEntry> = {}): StashEntry => ({
    index: 0,
    commit: "a".repeat(40),
    message: "before the merge",
    branch: "main",
    created: 1_700_000_000,
    files: 3,
    untracked: 0,
    untrackedCommit: null,
    ...over,
});

describe("gitAtLeast", () => {
    it("compares major, minor and patch in turn", () => {
        const v = (major: number, minor: number, patch = 0) => ({
            major,
            minor,
            patch,
        });
        expect(gitAtLeast(v(2, 35))).toBe(true);
        expect(gitAtLeast(v(2, 34, 9))).toBe(false);
        expect(gitAtLeast(v(3, 0))).toBe(true);
        expect(gitAtLeast(v(1, 99))).toBe(false);
        expect(gitAtLeast(v(2, 36))).toBe(true);
    });
    it("trusts an unknown version, since the backend refuses what git cannot do", () => {
        expect(gitAtLeast(null)).toBe(true);
    });
});

describe("stashTitle and stashMeta", () => {
    it("shows the message, or a placeholder when there is none", () => {
        expect(stashTitle(entry())).toBe("before the merge");
        expect(stashTitle(entry({ message: "  " }))).toBe("(no message)");
    });
    it("lists the branch, the files and the untracked files", () => {
        expect(stashMeta(entry())).toBe("main · 3 files");
        expect(stashMeta(entry({ files: 1, untracked: 2 }))).toBe(
            "main · 1 file · 2 untracked",
        );
        expect(stashMeta(entry({ branch: null }))).toBe(
            "detached HEAD · 3 files",
        );
    });
});

describe("stashStillListed", () => {
    it("is false for no selection and for a stash that left the list", () => {
        const list = [entry()];
        expect(stashStillListed(list[0].commit, list)).toBe(true);
        expect(stashStillListed("b".repeat(40), list)).toBe(false);
        expect(stashStillListed(null, list)).toBe(false);
    });
});
