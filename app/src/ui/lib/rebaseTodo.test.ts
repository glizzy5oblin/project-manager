import { describe, expect, it } from "vitest";
import type { RebaseCommit } from "./bindings/RebaseCommit";
import type { RebaseRange } from "./bindings/RebaseRange";
import {
    cleanMessage,
    defaultMessage,
    foldedInto,
    groupOf,
    hasChanges,
    move,
    setMessage,
    setOp,
    startRows,
    summary,
    toTodo,
    todoProblem,
} from "./rebaseTodo";

const commit = (n: string, message = `add ${n}\n`): RebaseCommit => ({
    id: n.repeat(40),
    shortId: n.repeat(7),
    subject: message.split("\n")[0],
    message,
    author: "A U Thor",
    pushed: false,
});

const range: RebaseRange = {
    branch: "main",
    base: "0".repeat(40),
    commits: [
        commit("a"),
        commit("b", "add b\n\nwith a body\n"),
        commit("c"),
        commit("d"),
    ],
};

describe("rebaseTodo", () => {
    it("starts with every commit kept, in order, and nothing to apply", () => {
        const rows = startRows(range);
        expect(rows.map((r) => r.op)).toEqual(["pick", "pick", "pick", "pick"]);
        expect(hasChanges(range, rows)).toBe(false);
        expect(summary(range, rows)).toBe("No changes yet");
        expect(todoProblem("main", rows)).toBeNull();
        expect(toTodo(rows)).toEqual(
            range.commits.map((c) => ({
                commit: c.id,
                op: "pick",
                message: null,
            })),
        );
    });

    it("moves rows within the list only", () => {
        const rows = startRows(range);
        const moved = move(rows, 1, -1);
        expect(moved.map((r) => r.commit.subject)).toEqual([
            "add b",
            "add a",
            "add c",
            "add d",
        ]);
        expect(rows[0].commit.subject).toBe("add a");
        expect(move(rows, 0, -1)).toBe(rows);
        expect(move(rows, 3, 1)).toBe(rows);
        expect(hasChanges(range, moved)).toBe(true);
        expect(summary(range, moved)).toBe("new order: 4 commits left");
    });

    it("folds squashes and fixups into the kept commit above, skipping drops", () => {
        let rows = startRows(range);
        rows = setOp(rows, 1, "squash");
        rows = setOp(rows, 2, "drop");
        rows = setOp(rows, 3, "fixup");
        expect(foldedInto(rows, 0)).toEqual([1, 3]);
        expect(groupOf(rows)).toEqual([0, 0, null, 0]);
        expect(defaultMessage(rows, 0)).toBe("add a\n\nadd b\n\nwith a body\n");
        expect(summary(range, rows)).toBe(
            "2 commits folded, 1 commit dropped: 1 commit left",
        );
        expect(toTodo(rows).map((t) => t.op)).toEqual([
            "pick",
            "squash",
            "drop",
            "fixup",
        ]);
    });

    it("sends a new message only where it differs from what git would make", () => {
        let rows = startRows(range);
        rows = setMessage(rows, 0, "add a  \n\n\n");
        expect(hasChanges(range, rows)).toBe(false);
        expect(toTodo(rows)[0].message).toBeNull();
        rows = setMessage(rows, 0, "Add A");
        expect(toTodo(rows)[0].message).toBe("Add A");
        expect(summary(range, rows)).toBe("1 new message: 4 commits left");

        // On a group, the combined message is the default; the commit's own one is a change.
        rows = setOp(startRows(range), 1, "squash");
        rows = setMessage(rows, 0, "add a\n\nadd b\n\nwith a body");
        expect(toTodo(rows)[0].message).toBeNull();
        rows = setMessage(rows, 0, "add a");
        expect(toTodo(rows)[0].message).toBe("add a");

        // A folded or dropped commit loses any message written for it.
        rows = setMessage(startRows(range), 2, "C");
        rows = setOp(rows, 2, "drop");
        expect(rows[2].message).toBeNull();
    });

    it("explains lists that cannot be applied", () => {
        const all = startRows(range).map((r) => ({
            ...r,
            op: "drop" as const,
        }));
        expect(todoProblem("main", all)).toContain("reset main");
        let rows = setOp(startRows(range), 0, "drop");
        rows = setOp(rows, 1, "fixup");
        expect(todoProblem("main", rows)).toContain(
            "bbbbbbb add b cannot be squashed or fixed up",
        );
        rows = setMessage(startRows(range), 2, " \n\n ");
        expect(todoProblem("main", rows)).toBe(
            "The message for ccccccc add c is empty",
        );
    });

    it("tidies messages as git does", () => {
        expect(cleanMessage("\n\nsubject  \r\n\n\n\nbody\t\n\n")).toBe(
            "subject\n\nbody\n",
        );
        expect(cleanMessage("   \n")).toBe("");
    });
});
