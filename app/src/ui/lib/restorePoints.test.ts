import { describe, expect, it } from "vitest";
import type { RestorePreview } from "./bindings/RestorePreview";
import { previewLines, restoreLabel } from "./restorePoints";

const preview = (over: Partial<RestorePreview> = {}): RestorePreview => ({
    backup: {} as RestorePreview["backup"],
    moves: [],
    headNow: "main",
    headThen: "main",
    changedNow: 0,
    untrackedThen: 0,
    stashesMissing: 0,
    ...over,
});

describe("restoreLabel", () => {
    it("explains known actions and passes others through", () => {
        expect(restoreLabel("stash-push")).toBe("Stashing changes");
        expect(restoreLabel("something-new")).toBe("something-new");
    });
});

describe("previewLines", () => {
    it("always says what happens to the files", () => {
        expect(previewLines(preview())).toEqual([
            "Your files are put back as they were.",
        ]);
    });

    it("lists moved and returning branches, the branch switch and stashes", () => {
        const lines = previewLines(
            preview({
                moves: [
                    {
                        name: "dev",
                        kind: "branch",
                        from: "a".repeat(40),
                        to: "b".repeat(40),
                    },
                    {
                        name: "old",
                        kind: "branch",
                        from: null,
                        to: "c".repeat(40),
                    },
                ],
                headNow: null,
                headThen: "dev",
                changedNow: 2,
                untrackedThen: 1,
                stashesMissing: 1,
            }),
        );
        expect(lines).toEqual([
            "The branch dev moves back to bbbbbbbb.",
            "The branch old comes back.",
            "You return to the branch dev.",
            "2 files with changes right now are replaced by the files as they were. A new restore point of them is made first.",
            "1 new file that Git was not tracking comes back.",
            "1 stash deleted since then comes back.",
        ]);
    });
});
