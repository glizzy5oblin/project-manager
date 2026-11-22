// Pure helpers for the Changes view: the file rows, the selection as files move between lists,
// and why a commit cannot be made yet.

import type { ChangeKind } from "./bindings/ChangeKind";
import type { StatusSummary } from "./bindings/StatusSummary";

export type ChangeSide = "conflicted" | "staged" | "unstaged";

export type ChangeRow = { path: string; kind: ChangeKind; side: ChangeSide };

export type Selection = { path: string; side: ChangeSide };

/** Conflicted, then staged, then unstaged files (untracked ones among them), each by path. */
export function changeRows(status: StatusSummary | null): ChangeRow[] {
    if (!status) return [];
    const unstaged: ChangeRow[] = [
        ...status.unstaged.map((c) => ({
            path: c.path,
            kind: c.kind,
            side: "unstaged" as const,
        })),
        ...status.untracked.map((path) => ({
            path,
            kind: "untracked" as const,
            side: "unstaged" as const,
        })),
    ].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    return [
        ...status.conflicted.map((path) => ({
            path,
            kind: "conflicted" as const,
            side: "conflicted" as const,
        })),
        ...status.staged.map((c) => ({
            path: c.path,
            kind: c.kind,
            side: "staged" as const,
        })),
        ...unstaged,
    ];
}

/**
 * The selection after the lists changed: the same row if it is still there, else the same file
 * in another list (it was just staged or unstaged; a file in both stays where it was), else none.
 * Returns the same object when nothing changed, so a reactive assignment settles.
 */
export function followSelection(
    sel: Selection | null,
    rows: ChangeRow[],
): Selection | null {
    if (!sel) return null;
    if (rows.some((r) => r.path === sel.path && r.side === sel.side)) {
        return sel;
    }
    const moved = rows.find((r) => r.path === sel.path);
    return moved ? { path: moved.path, side: moved.side } : null;
}

const LETTERS: Record<ChangeKind, string> = {
    added: "A",
    modified: "M",
    deleted: "D",
    renamed: "R",
    typeChange: "T",
    conflicted: "!",
    untracked: "U",
};

export function kindLetter(kind: ChangeKind): string {
    return LETTERS[kind] ?? "?";
}

/** Why the commit button is off, or null when a commit can be made. */
export function commitProblem(
    message: string,
    amend: boolean,
    staged: number,
    conflicted: number,
): string | null {
    if (conflicted > 0) return "Resolve the conflicts first";
    if (!message.trim()) return "Write a commit message";
    if (!amend && staged === 0) return "Stage some changes first";
    return null;
}
