import { describe, expect, it } from "vitest";
import type { HeadState } from "./bindings/HeadState";
import type { RefLabel } from "./bindings/RefLabel";
import { commitMenu } from "./commitMenu.svelte";

const ref = (
    name: string,
    kind: RefLabel["kind"],
    isHead = false,
): RefLabel => ({
    name,
    full: kind === "remote" ? `refs/remotes/${name}` : `refs/heads/${name}`,
    kind,
    isHead,
});

const head = (branch: string | null, commit: string): HeadState =>
    ({ branch, commit }) as HeadState;

const menu = (over: Partial<Parameters<typeof commitMenu>[0]> = {}) =>
    commitMenu({
        commit: "c".repeat(40),
        subject: "Fix it",
        refs: [],
        head: head("main", "a".repeat(40)),
        busy: false,
        ...over,
    });

const ids = (m: ReturnType<typeof commitMenu>) =>
    m.items.filter((i) => !i.separator).map((i) => i.id);

describe("commitMenu", () => {
    it("on a plain commit: new branch, resets, and copying", () => {
        expect(ids(menu())).toEqual([
            "new-branch",
            "reset-soft",
            "reset-mixed",
            "reset-hard",
            "copy-id",
            "copy-subject",
        ]);
    });

    it("greys out, with a reason, what cannot be done at HEAD's own commit", () => {
        const commit = "a".repeat(40);
        const m = menu({
            commit,
            refs: [ref("main", "branch", true), ref("dev", "branch")],
        });
        const off = m.items.filter((i) => i.disabled && !i.separator);
        expect(off.map((i) => i.id)).toEqual([
            "reset-soft",
            "reset-mixed",
            "reset-hard",
            "checkout:main",
            "merge:main",
            "delete:main",
            "merge:dev",
        ]);
        for (const i of off) expect(i.hint, i.id).toBeTruthy();
        expect(m.items.find((i) => i.id === "checkout:dev")?.disabled).toBe(
            false,
        );
    });

    it("offers checkout, merge, rename and delete for other branches", () => {
        const m = menu({ refs: [ref("dev", "branch")] });
        expect(m.commands["merge:dev"]).toEqual({
            type: "action",
            action: { type: "merge", branch: "dev" },
        });
        expect(m.items.find((i) => i.id === "delete:dev")?.danger).toBe(true);
        expect(m.commands["rename:dev"]).toEqual({
            type: "form",
            form: { kind: "rename", from: "dev" },
        });
    });

    it("merges a remote branch into the current branch only", () => {
        const refs = [ref("origin/dev", "remote")];
        const merge = (m: ReturnType<typeof commitMenu>) =>
            m.items.find((i) => i.id === "merge:origin/dev");
        expect(merge(menu({ refs }))?.disabled).toBe(false);
        const detached = merge(
            menu({ refs, head: head(null, "a".repeat(40)) }),
        );
        expect(detached?.disabled).toBe(true);
        expect(detached?.hint).toContain("Check out a branch");
    });

    it("marks only the hard reset as dangerous", () => {
        const danger = menu()
            .items.filter((i) => i.danger)
            .map((i) => i.id);
        expect(danger).toEqual(["reset-hard"]);
    });

    it("disables write items while an action runs, but not copying", () => {
        const m = menu({ busy: true, refs: [ref("dev", "branch")] });
        for (const i of m.items.filter((i) => !i.separator)) {
            expect(i.disabled, i.id).toBe(m.commands[i.id].type !== "copy");
        }
    });

    it("never starts or ends with a separator or doubles one", () => {
        for (const m of [
            menu(),
            menu({ head: null }),
            menu({ refs: [ref("dev", "branch"), ref("o/dev", "remote")] }),
        ]) {
            const seps = m.items.map((i) => !!i.separator);
            expect(seps[0]).toBe(false);
            expect(seps[seps.length - 1]).toBe(false);
            expect(seps.some((s, i) => s && seps[i + 1])).toBe(false);
        }
    });
});
