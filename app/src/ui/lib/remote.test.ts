import { describe, expect, it } from "vitest";
import type { RepoState } from "./bindings/RepoState";
import { aheadBehind, pushTagBlocked, syncInfo } from "./remote";

function state(
    over: Partial<RepoState> = {},
    ahead: number | null = 0,
    behind: number | null = 0,
): RepoState {
    return {
        root: "/r",
        bare: false,
        trusted: true,
        head: { branch: "main", commit: "abc", detached: false },
        branches: [
            {
                name: "main",
                commit: "abc",
                isHead: true,
                upstream: "origin/main",
                ahead,
                behind,
            },
        ],
        remoteBranches: [],
        tags: [{ name: "v1", commit: "abc", annotated: true }],
        remotes: [{ name: "origin", fetchUrl: "u", pushUrl: "u" }],
        status: { staged: [], unstaged: [], untracked: [], conflicted: [] },
        inProgress: null,
        mergeMessage: null,
        hasSubmodules: false,
        ...over,
    };
}

const why = (r: RepoState, busy = false) =>
    Object.fromEntries(syncInfo(r, busy).buttons.map((b) => [b.id, b.why]));

describe("syncInfo", () => {
    it("allows only what makes sense for ahead and behind", () => {
        expect(why(state({}, 2, 0))).toMatchObject({
            fetch: null,
            push: null,
            pull: expect.stringContaining("already has everything"),
            "force-push": expect.stringContaining("normal Push is enough"),
        });
        const diverged = why(state({}, 2, 1));
        expect(diverged.pull).toBeNull();
        expect(diverged["pull-rebase"]).toBeNull();
        expect(diverged.push).toContain("pull first");
        expect(diverged["force-push"]).toBeNull();
        expect(why(state({}, 0, 0)).push).toContain("Nothing to push");
    });

    it("explains a missing remote, branch or upstream", () => {
        const w = why(state({ remotes: [] }));
        for (const v of Object.values(w))
            expect(v).toBe("This project has no remote");
        const detached = why(
            state({ head: { branch: null, commit: "abc", detached: true } }),
        );
        expect(detached.push).toContain("not on one");
        expect(detached.fetch).toBeNull();
        const r = state();
        r.branches[0].upstream = null;
        r.branches[0].ahead = null;
        r.branches[0].behind = null;
        const fresh = why(r);
        expect(fresh.pull).toContain("does not track");
        expect(fresh.push).toBeNull();
        expect(fresh["force-push"]).toContain("use Push");
        expect(
            syncInfo(r, false).buttons.find((b) => b.id === "push")?.hint,
        ).toContain("Create main on origin");
        r.remotes.push({ name: "fork", fetchUrl: "f", pushUrl: "f" });
        expect(why(r).push).toContain("several remotes");
    });

    it("keeps a rebase off a dirty tree and every button off while busy", () => {
        const r = state(
            {
                status: {
                    staged: [],
                    unstaged: [{ path: "a" } as never],
                    untracked: [],
                    conflicted: [],
                },
            },
            1,
            1,
        );
        expect(why(r).pull).toBeNull();
        expect(why(r)["pull-rebase"]).toContain("clean working tree");
        expect(
            why(
                state(
                    {
                        status: {
                            staged: [],
                            unstaged: [],
                            untracked: ["x"],
                            conflicted: [],
                        },
                    },
                    0,
                    1,
                ),
            )["pull-rebase"],
        ).toBeNull();
        for (const v of Object.values(why(state({}, 1, 1), true)))
            expect(v).not.toBeNull();
        expect(why(state({ inProgress: "merge" }, 1, 1)).pull).toContain(
            "Finish or abort",
        );
    });

    it("marks the rewriting buttons as dangerous", () => {
        const danger = syncInfo(state(), false)
            .buttons.filter((b) => b.danger)
            .map((b) => b.id);
        expect(danger).toEqual(["pull-rebase", "force-push"]);
    });
});

describe("aheadBehind", () => {
    it("says it in words", () => {
        expect(aheadBehind(0, 0)).toBe("up to date");
        expect(aheadBehind(2, 1)).toBe("2 commits to push, 1 commit to pull");
        expect(aheadBehind(null, null)).toBe("not fetched yet");
    });
});

describe("pushTagBlocked", () => {
    it("needs a tag, a remote and no running action", () => {
        expect(pushTagBlocked(state(), "v1", "origin", false)).toBeNull();
        expect(pushTagBlocked(state(), "", "origin", false)).toContain(
            "Choose a tag",
        );
        expect(
            pushTagBlocked(state({ tags: [] }), "v1", "origin", false),
        ).toContain("no tags");
        expect(pushTagBlocked(state(), "v1", "origin", true)).toContain(
            "running",
        );
    });
});
