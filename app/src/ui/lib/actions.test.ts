import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const { ActionRunner, outcomeNotice } = await import("./actions.svelte");
import type { Preview } from "./bindings/Preview";
import type { TaskOutcome } from "./bindings/TaskOutcome";

const preview = (dangerous: boolean, token = "t1"): Preview => ({
    title: "Do it",
    dangerous,
    impact: [],
    commands: ["git do"],
    token,
});
const ok: TaskOutcome = {
    status: "ok",
    value: { title: "Done", backup: null, conflicts: [] },
};
const failed = (kind: string): TaskOutcome => ({
    status: "failed",
    error: { kind: kind as "git", message: kind, detail: null, issues: [] },
});

/** A runner whose backend answers from the given lists, in order. */
function setup(previews: (Preview | Error)[], outcomes: TaskOutcome[]) {
    const deps = {
        preview: vi.fn(async () => {
            const p = previews.shift()!;
            if (p instanceof Error) throw p;
            return p;
        }),
        run: vi.fn(async () => 7),
        wait: vi.fn(async () => outcomes.shift()!),
    };
    return { runner: new ActionRunner(deps), deps };
}

/** Answer the dialog once it shows. */
async function answer(runner: InstanceType<typeof ActionRunner>, yes: boolean) {
    await vi.waitFor(() => expect(runner.pending).not.toBe(null));
    runner.pending!.answer(yes);
}

const stage = { type: "stage" as const, paths: ["a"] };
const reset = {
    type: "reset" as const,
    target: "HEAD~1",
    mode: "hard" as const,
};

describe("ActionRunner", () => {
    it("runs a safe action without asking or sending a token", async () => {
        const { runner, deps } = setup([preview(false)], [ok]);
        const out = await runner.run("p", stage);
        expect(out?.title).toBe("Done");
        expect(runner.pending).toBe(null);
        expect(deps.run).toHaveBeenCalledWith(
            "p",
            stage,
            null,
            false,
            "private",
        );
        expect(runner.busy).toEqual({});
    });

    it("asks before a dangerous action and sends the preview's token", async () => {
        const { runner, deps } = setup([preview(true, "tok")], [ok]);
        const done = runner.run("p", reset);
        expect(runner.busy).toEqual({ p: 1 });
        await answer(runner, true);
        await done;
        expect(deps.run).toHaveBeenCalledWith(
            "p",
            reset,
            "tok",
            false,
            "private",
        );
    });

    it("runs nothing when the user cancels", async () => {
        const { runner, deps } = setup([preview(true)], []);
        const done = runner.run("p", reset);
        await answer(runner, false);
        expect(await done).toBe(null);
        expect(deps.run).not.toHaveBeenCalled();
    });

    it("previews again and asks again when the repository changed", async () => {
        const { runner, deps } = setup(
            [preview(true, "old"), preview(true, "new")],
            [failed("stale"), ok],
        );
        const done = runner.run("p", reset);
        await answer(runner, true);
        await vi.waitFor(() => expect(runner.pending?.note).toMatch(/changed/));
        runner.pending!.answer(true);
        await done;
        expect(deps.run).toHaveBeenLastCalledWith(
            "p",
            reset,
            "new",
            false,
            "private",
        );
    });

    it("asks with a warning before writing to an untrusted repository", async () => {
        const untrusted = Object.assign(new Error("untrusted"), {
            kind: "untrusted",
        });
        const { runner, deps } = setup([untrusted, preview(false)], [ok]);
        const done = runner.run("p", stage);
        await vi.waitFor(() => expect(runner.pending?.untrusted).toBe(true));
        runner.pending!.answer(true);
        await done;
        expect(deps.preview).toHaveBeenLastCalledWith(
            "p",
            stage,
            true,
            "private",
        );
        expect(deps.run).toHaveBeenCalledWith(
            "p",
            stage,
            null,
            true,
            "private",
        );
    });

    it("rejects with the task's error and clears the busy count", async () => {
        const { runner } = setup([preview(false)], [failed("git")]);
        await expect(runner.run("p", stage)).rejects.toMatchObject({
            kind: "git",
        });
        expect(runner.busy).toEqual({});
    });

    it("refuses a second confirmation while one is open", async () => {
        const { runner } = setup([preview(true), preview(true)], [ok]);
        const first = runner.run("p", reset);
        await vi.waitFor(() => expect(runner.pending).not.toBe(null));
        expect(await runner.run("p", reset)).toBe(null);
        runner.pending!.answer(true);
        expect((await first)?.title).toBe("Done");
    });
});

describe("outcomeNotice", () => {
    const backup = { id: "20261120-155701-abort-merge" } as never;

    it("ends each part as a sentence", () => {
        expect(
            outcomeNotice({ title: "Abort the merge", backup, conflicts: [] }),
        ).toEqual({
            text: "Abort the merge. Backup 20261120-155701-abort-merge taken.",
            warn: false,
        });
        expect(
            outcomeNotice(
                { title: "Commit", backup: null, conflicts: [] },
                "Committed.",
            ).text,
        ).toBe("Committed.");
    });

    it("warns about conflicts", () => {
        expect(
            outcomeNotice({
                title: "Merge clash into main",
                backup: null,
                conflicts: ["story.txt"],
            }),
        ).toEqual({
            text: "Merge clash into main. 1 conflicted file: resolve it in Changes.",
            warn: true,
        });
        expect(
            outcomeNotice({
                title: "Merge",
                backup: null,
                conflicts: ["a", "b"],
            }).text,
        ).toBe("Merge. 2 conflicted files: resolve them in Changes.");
    });
});
