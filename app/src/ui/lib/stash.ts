// Pure helpers for the stash list in the Changes view: how an entry reads, which options the
// installed git offers, and the files a stash shows.

import type { GitVersion } from "./bindings/GitVersion";
import type { StashEntry } from "./bindings/StashEntry";

/** `git stash push --staged` needs git 2.35 (docs/actions.md). */
export const STAGED_MIN_GIT: GitVersion = { major: 2, minor: 35, patch: 0 };

/** Whether `v` is at least `min`; an unknown version is treated as new enough (git decides). */
export function gitAtLeast(
    v: GitVersion | null,
    min: GitVersion = STAGED_MIN_GIT,
): boolean {
    if (!v) return true;
    if (v.major !== min.major) return v.major > min.major;
    if (v.minor !== min.minor) return v.minor > min.minor;
    return v.patch >= min.patch;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** The stash's title: what the user typed, or git's own text for a stash made without one. */
export function stashTitle(s: StashEntry): string {
    return s.message.trim() || "(no message)";
}

/** `main · 3 files · 1 untracked`, the line under the title. */
export function stashMeta(s: StashEntry): string {
    const parts = [s.branch ?? "detached HEAD", plural(s.files, "file")];
    if (s.untracked) parts.push(`${s.untracked} untracked`);
    return parts.join(" · ");
}

/** Whether the stash is still in the list (it was applied, popped or dropped otherwise). */
export function stashStillListed(
    commit: string | null,
    list: StashEntry[],
): boolean {
    return commit !== null && list.some((s) => s.commit === commit);
}
