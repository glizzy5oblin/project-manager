// The list behind the commit editor (interactive rebase): each commit of the range, oldest first,
// in its new place, with what happens to it. The backend checks the list again when it plans
// the rebase; the checks here only keep the Apply button honest while the list is edited.

import type { RebaseCommit } from "./bindings/RebaseCommit";
import type { RebaseRange } from "./bindings/RebaseRange";
import type { TodoItem } from "./bindings/TodoItem";
import type { TodoOp } from "./bindings/TodoOp";

export interface TodoRow {
    commit: RebaseCommit;
    op: TodoOp;
    /**
     * A new message written for this kept commit, which is the whole message of the commits
     * folded into it. Null keeps the message git would make (see `defaultMessage`).
     */
    message: string | null;
}

/** What each operation does, in plain words, for buttons and hints. */
export const OP_HINT: Record<TodoOp, string> = {
    pick: "Keep this commit",
    squash: "Fold this commit into the kept commit above it, adding its message to that commit's",
    fixup: "Fold this commit into the kept commit above it, leaving its message out",
    drop: "Leave this commit out. The backup keeps it",
};

export const OP_LABEL: Record<TodoOp, string> = {
    pick: "Keep",
    squash: "Squash",
    fixup: "Fixup",
    drop: "Drop",
};

/** Every commit kept, in its old order, with its message. */
export function startRows(range: RebaseRange): TodoRow[] {
    return range.commits.map((commit) => ({
        commit,
        op: "pick",
        message: null,
    }));
}

/** The rows with row `i` moved one place up (`by` -1) or down (+1); unchanged at an end. */
export function move(rows: TodoRow[], i: number, by: -1 | 1): TodoRow[] {
    const j = i + by;
    if (i < 0 || i >= rows.length || j < 0 || j >= rows.length) return rows;
    const out = rows.slice();
    [out[i], out[j]] = [out[j], out[i]];
    return out;
}

/** The rows with row `i` set to `op`. Only a kept commit holds a new message. */
export function setOp(rows: TodoRow[], i: number, op: TodoOp): TodoRow[] {
    return rows.map((r, k) =>
        k === i ? { ...r, op, message: op === "pick" ? r.message : null } : r,
    );
}

/** The rows with a new message for row `i`, or none (null). */
export function setMessage(
    rows: TodoRow[],
    i: number,
    message: string | null,
): TodoRow[] {
    return rows.map((r, k) => (k === i ? { ...r, message } : r));
}

/**
 * The rows folded into kept row `i`: the squashed and fixed-up ones after it, up to the next
 * kept one. Dropped rows in between are skipped.
 */
export function foldedInto(rows: TodoRow[], i: number): number[] {
    const out: number[] = [];
    for (let k = i + 1; k < rows.length; k++) {
        const op = rows[k].op;
        if (op === "pick") break;
        if (op !== "drop") out.push(k);
    }
    return out;
}

/**
 * For each row, the kept row it is folded into: its own index for a kept row, the index of the
 * nearest kept row above it for a squash or fixup, and null for a dropped row or a fold with
 * no kept row above it.
 */
export function groupOf(rows: TodoRow[]): (number | null)[] {
    let head: number | null = null;
    return rows.map((r, i) => {
        if (r.op === "pick") head = i;
        return r.op === "drop" ? null : head;
    });
}

/** A message tidied as git does with `--cleanup=whitespace`: no trailing spaces, no runs of blank lines. */
export function cleanMessage(m: string): string {
    let out = "";
    let blank = false;
    for (const raw of m.split("\n")) {
        const line = raw.trimEnd();
        if (line === "") {
            blank = out !== "";
            continue;
        }
        if (blank) {
            out += "\n";
            blank = false;
        }
        out += line + "\n";
    }
    return out;
}

/**
 * The message kept row `i` ends up with when no new one is written: its own, followed by the
 * messages of the commits squashed into it (not the fixed-up ones).
 */
export function defaultMessage(rows: TodoRow[], i: number): string {
    const parts = [rows[i].commit.message];
    for (const k of foldedInto(rows, i)) {
        if (rows[k].op === "squash") parts.push(rows[k].commit.message);
    }
    return cleanMessage(parts.join("\n\n"));
}

/** The message row `i` ends up with: the new one if written, else the default. Null for a fold or drop. */
export function finalMessage(rows: TodoRow[], i: number): string | null {
    if (rows[i].op !== "pick") return null;
    const m = rows[i].message;
    return m === null ? defaultMessage(rows, i) : cleanMessage(m);
}

/** Whether kept row `i` has a new message written that differs from what git would make. */
export function isReworded(rows: TodoRow[], i: number): boolean {
    const m = rows[i].message;
    return (
        rows[i].op === "pick" &&
        m !== null &&
        cleanMessage(m) !== defaultMessage(rows, i)
    );
}

/** The list to send with the action: a message only where one was written and differs. */
export function toTodo(rows: TodoRow[]): TodoItem[] {
    return rows.map((r, i) => ({
        commit: r.commit.id,
        op: r.op,
        message: isReworded(rows, i) ? r.message : null,
    }));
}

/** Whether the list would change anything: a move, a fold, a drop or a new message. */
export function hasChanges(range: RebaseRange, rows: TodoRow[]): boolean {
    return rows.some(
        (r, i) =>
            r.commit.id !== range.commits[i]?.id ||
            r.op !== "pick" ||
            isReworded(rows, i),
    );
}

const label = (c: RebaseCommit) => `${c.shortId} ${c.subject}`;

/**
 * Why the list cannot be applied as it is, or null when it can. "Nothing to change yet" is
 * not a problem: `hasChanges` covers it.
 */
export function todoProblem(branch: string, rows: TodoRow[]): string | null {
    const kept = rows.filter((r) => r.op !== "drop");
    if (kept.length === 0)
        return `That drops every commit. To do that, reset ${branch} to the commit before them instead`;
    if (kept[0].op !== "pick")
        return `${label(kept[0].commit)} cannot be squashed or fixed up: there is no kept commit above it to fold it into`;
    for (let i = 0; i < rows.length; i++) {
        const m = finalMessage(rows, i);
        if (m !== null && m.trim() === "")
            return `The message for ${label(rows[i].commit)} is empty`;
    }
    return null;
}

/** Counts for the summary line under the list. */
export function summary(range: RebaseRange, rows: TodoRow[]): string {
    const n = (k: number, what: string) => `${k} ${what}${k === 1 ? "" : "s"}`;
    const parts: string[] = [];
    if (rows.some((r, i) => r.commit.id !== range.commits[i]?.id))
        parts.push("new order");
    const folded = rows.filter(
        (r) => r.op === "squash" || r.op === "fixup",
    ).length;
    if (folded) parts.push(`${n(folded, "commit")} folded`);
    const dropped = rows.filter((r) => r.op === "drop").length;
    if (dropped) parts.push(`${n(dropped, "commit")} dropped`);
    const reworded = rows.filter((_, i) => isReworded(rows, i)).length;
    if (reworded) parts.push(`${n(reworded, "new message")}`);
    const kept = rows.length - folded - dropped;
    return parts.length
        ? `${parts.join(", ")}: ${n(kept, "commit")} left`
        : "No changes yet";
}
