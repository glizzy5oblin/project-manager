import { describe, expect, it } from "vitest";
import type { ConflictInfo } from "./bindings/ConflictInfo";
import { explain, sideNames, steps } from "./conflicts";

const info = (over: Partial<ConflictInfo> = {}): ConflictInfo => ({
    path: "a.txt",
    kind: "bothModified",
    oursName: "main",
    theirsName: "feature/x",
    binary: false,
    regions: [],
    moreRegions: false,
    text: null,
    ...over,
});

describe("explain", () => {
    it("says who changed what, by branch name", () => {
        expect(explain(info()).headline).toBe(
            "You (main) and feature/x both changed this file.",
        );
        expect(explain(info({ kind: "deletedByThem" })).headline).toBe(
            "feature/x deleted this file, but you (main) changed it.",
        );
    });

    it("counts the disagreements", () => {
        const region = {
            startLine: 2,
            endLine: 6,
            ours: ["a"],
            theirs: ["b"],
            base: null,
            truncated: false,
        };
        expect(explain(info({ regions: [region] })).why).toContain("1 place");
        expect(explain(info({ regions: [region, region] })).why).toContain(
            "2 places",
        );
    });

    it("names the sides even without a branch", () => {
        const none = info({ oursName: null, theirsName: null });
        expect(sideNames(none)).toEqual({
            you: "your version",
            them: "the other side",
        });
        expect(explain(none).headline).toContain("the other side");
    });

    it("tells a binary file to pick one", () => {
        expect(explain(info({ binary: true })).why).toContain(
            "not a text file",
        );
    });
});

describe("steps and regions", () => {
    it("points at the buttons, and at Delete only where a file can be removed", () => {
        expect(steps(info()).join(" ")).toContain("Keep mine");
        expect(steps(info({ binary: true })).join(" ")).toContain(
            "Keep my file",
        );
        expect(steps(info({ kind: "deletedByUs" })).join(" ")).toContain(
            "Delete the file",
        );
    });

    it("always ends with how to finish or cancel", () => {
        for (const kind of [
            "bothModified",
            "bothAdded",
            "deletedByUs",
        ] as const)
            expect(steps(info({ kind })).at(-1)).toContain("Abort merge");
    });
});

import { findRegions, resolveRegion, withLineEnds } from "./conflicts";

const two =
    "a\n<<<<<<< HEAD\nmine1\n=======\ntheirs1\n>>>>>>> other\nb\n<<<<<<< HEAD\nmine2\n||||||| base\nold\n=======\ntheirs2\n>>>>>>> other\nc\n";

describe("editing the text", () => {
    it("finds each conflict, with a base section left out", () => {
        const r = findRegions(two);
        expect(r.length).toBe(2);
        expect(r[0].ours).toEqual(["mine1"]);
        expect(r[1].ours).toEqual(["mine2"]);
        expect(r[1].theirs).toEqual(["theirs2"]);
    });

    it("settles one conflict and leaves the rest", () => {
        expect(resolveRegion(two, 0, "theirs")).toBe(
            "a\ntheirs1\nb\n<<<<<<< HEAD\nmine2\n||||||| base\nold\n=======\ntheirs2\n>>>>>>> other\nc\n",
        );
        const done = resolveRegion(resolveRegion(two, 0, "ours"), 0, "both");
        expect(done).toBe("a\nmine1\nb\nmine2\ntheirs2\nc\n");
        expect(findRegions(done)).toEqual([]);
        expect(resolveRegion(two, 0, "theirsFirst")).toContain(
            "a\ntheirs1\nmine1\nb",
        );
    });

    it("ignores an index that is not there and unclosed markers", () => {
        expect(resolveRegion(two, 5, "ours")).toBe(two);
        expect(findRegions("<<<<<<< x\na\n=======\nb\n")).toEqual([]);
    });

    it("keeps the file's own line ends", () => {
        expect(withLineEnds("a\nb\n", "x\r\ny\r\n")).toBe("a\r\nb\r\n");
        expect(withLineEnds("a\nb\n", "x\ny\n")).toBe("a\nb\n");
        expect(
            findRegions("<<<<<<< x\r\na\r\n=======\r\nb\r\n>>>>>>> y\r\n")
                .length,
        ).toBe(1);
    });
});
