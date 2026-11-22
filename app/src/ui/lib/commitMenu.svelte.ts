// The right-click menu for a commit in the graph and log: the git actions the commit panel
// offers (see CommitActions.svelte), for the commit and the branches that point at it. Choosing one
// hands it to the panel, which runs it and shows the result or the error beside the commit.

import type { Action } from "./bindings/Action";
import type { HeadState } from "./bindings/HeadState";
import type { RefLabel } from "./bindings/RefLabel";
import type { ResetMode } from "./bindings/ResetMode";
import type { MenuItem } from "./contextMenu";
import { shortId } from "./history";

/** An inline name form in the commit panel: a new branch here, or a new name for `from`. */
export type NameForm = { kind: "create" } | { kind: "rename"; from: string };

export type CommitCommand =
    | { type: "form"; form: NameForm }
    | { type: "action"; action: Action }
    | { type: "copy"; text: string };

export interface CommitMenu {
    items: MenuItem[];
    commands: Record<string, CommitCommand>;
}

const RESET_LABEL: Record<ResetMode, string> = {
    soft: "keep changes staged",
    mixed: "keep changes unstaged",
    hard: "discard changes",
};

/** What a menu item does, in plain words, shown on hover. */
function hintFor(id: string): string | undefined {
    if (id === "new-branch") return "Start a new branch from this commit";
    if (id === "reset-soft")
        return "Move the branch to this commit and keep all your changes staged";
    if (id === "reset-mixed")
        return "Move the branch to this commit and keep your changes, unstaged";
    if (id === "reset-hard")
        return "Move the branch to this commit and throw away your changes. A backup is taken first";
    if (id.startsWith("checkout:")) return "Switch your files to this branch";
    if (id.startsWith("merge:"))
        return "Bring this branch's commits into the branch you are on";
    if (id.startsWith("rename:")) return "Give this branch another name";
    if (id.startsWith("delete:"))
        return "Delete this branch. A backup is taken first";
    if (id === "copy-id") return "Copy the full commit id to the clipboard";
    if (id === "copy-subject")
        return "Copy the commit's first line to the clipboard";
    return undefined;
}

/** The menu for one commit. Write items are disabled while an action runs on the project. */
export function commitMenu(c: {
    commit: string;
    subject: string;
    refs: RefLabel[];
    head: HeadState | null;
    busy: boolean;
}): CommitMenu {
    const items: MenuItem[] = [];
    const commands: Record<string, CommitCommand> = {};
    let groups = 0;

    const add = (
        id: string,
        label: string,
        command: CommitCommand,
        extra: Partial<MenuItem> = {},
        /** Why the item cannot be used here. It stays in the menu, greyed out, with this on hover. */
        why: string | null = null,
    ) => {
        const write = command.type !== "copy";
        items.push({
            id,
            label,
            keys: "",
            disabled: (write && c.busy) || why !== null,
            hint: why ?? hintFor(id),
            ...extra,
        });
        commands[id] = command;
    };
    const group = () => {
        if (items.length > 0 && !items[items.length - 1].separator) {
            items.push({
                id: `sep-${groups++}`,
                label: "",
                keys: "",
                disabled: true,
                separator: true,
            });
        }
    };

    const current = c.head?.branch ?? null;
    const isHeadCommit = c.head?.commit === c.commit;
    const branches = c.refs.filter((r) => r.kind === "branch");
    const remotes = c.refs.filter((r) => r.kind === "remote");

    add("new-branch", "New branch here…", {
        type: "form",
        form: { kind: "create" },
    });

    const mergeWhy = (r: RefLabel): string | null => {
        if (!current)
            return "Check out a branch first: you are not on one right now";
        if (r.isHead) return "This is the branch you are on";
        if (isHeadCommit)
            return "Nothing to merge: your branch is already at this commit";
        return null;
    };

    group();
    const resetWhy = !c.head?.commit
        ? "This project has no commits yet"
        : isHeadCommit
          ? "Already here: the checked-out commit is this one"
          : null;
    for (const mode of ["soft", "mixed", "hard"] as const) {
        add(
            `reset-${mode}`,
            `Reset ${current ?? "HEAD"} here (${mode}: ${RESET_LABEL[mode]})`,
            {
                type: "action",
                action: { type: "reset", target: c.commit, mode },
            },
            { danger: mode === "hard" },
            resetWhy,
        );
    }

    for (const b of branches) {
        group();
        add(
            `checkout:${b.name}`,
            `Check out ${b.name}`,
            {
                type: "action",
                action: { type: "checkout", branch: b.name },
            },
            {},
            b.isHead ? "You are already on this branch" : null,
        );
        add(
            `merge:${b.name}`,
            `Merge ${b.name} into ${current ?? "HEAD"}`,
            {
                type: "action",
                action: { type: "merge", branch: b.name },
            },
            {},
            mergeWhy(b),
        );
        add(`rename:${b.name}`, `Rename ${b.name}…`, {
            type: "form",
            form: { kind: "rename", from: b.name },
        });
        add(
            `delete:${b.name}`,
            `Delete ${b.name}`,
            {
                type: "action",
                action: { type: "deleteBranch", name: b.name },
            },
            { danger: true },
            b.isHead
                ? "You cannot delete the branch you are on. Check out another branch first"
                : null,
        );
    }

    for (const r of remotes) {
        group();
        add(
            `merge:${r.name}`,
            `Merge ${r.name} into ${current ?? "HEAD"}`,
            {
                type: "action",
                action: { type: "merge", branch: r.name },
            },
            {},
            mergeWhy(r),
        );
    }

    group();
    add("copy-id", `Copy commit id (${shortId(c.commit)})`, {
        type: "copy",
        text: c.commit,
    });
    if (c.subject) {
        add("copy-subject", "Copy subject", { type: "copy", text: c.subject });
    }
    return { items, commands };
}

/** A command chosen in the menu, waiting for the commit panel to run it. */
export const commitCommands = $state<{
    request: { commit: string; command: CommitCommand } | null;
}>({ request: null });
