// Plain-words descriptions of restore points (backups) for the Activity view.

import type { RestorePreview } from "./bindings/RestorePreview";

const WHAT: Record<string, string> = {
    "stash-push": "Stashing changes",
    resolve: "Settling a merge conflict",
    "stash-drop": "Deleting a stash",
    "stash-clear": "Deleting every stash",
    discard: "Discarding changes",
    amend: "Amending the last commit",
    "delete-branch": "Deleting a branch",
    "abort-merge": "Aborting a merge",
    "reset-soft": "Resetting a branch (soft)",
    "reset-mixed": "Resetting a branch (mixed)",
    "reset-hard": "Resetting a branch (hard)",
    restore: "Restoring a restore point",
};

/** What the app was about to do when the restore point was taken. */
export function restoreLabel(action: string): string {
    return WHAT[action] ?? action;
}

const plural = (n: number, one: string, many: string) =>
    `${n} ${n === 1 ? one : many}`;

/** What restoring would change, one sentence per line. */
export function previewLines(p: RestorePreview): string[] {
    const out: string[] = [];
    for (const m of p.moves) {
        out.push(
            m.from === null
                ? `The ${m.kind} ${m.name} comes back.`
                : `The ${m.kind} ${m.name} moves back to ${m.to.slice(0, 8)}.`,
        );
    }
    if (p.headThen !== p.headNow && p.headThen !== null) {
        out.push(`You return to the branch ${p.headThen}.`);
    }
    out.push(
        p.changedNow > 0
            ? `${plural(p.changedNow, "file with changes", "files with changes")} right now ${p.changedNow === 1 ? "is" : "are"} replaced by the files as they were. A new restore point of them is made first.`
            : "Your files are put back as they were.",
    );
    if (p.untrackedThen > 0) {
        out.push(
            `${plural(p.untrackedThen, "new file", "new files")} that Git was not tracking ${p.untrackedThen === 1 ? "comes" : "come"} back.`,
        );
    }
    if (p.stashesMissing > 0) {
        out.push(
            `${plural(p.stashesMissing, "stash", "stashes")} deleted since then ${p.stashesMissing === 1 ? "comes" : "come"} back.`,
        );
    }
    return out;
}
