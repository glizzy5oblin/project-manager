// The sync bar's buttons (SyncBar.svelte): fetch, pull, push and force push for the branch you are
// on, and pushing a tag. Each button says why it cannot be used, from the last-read repository
// state; the backend checks again with fresh numbers when the action is previewed.

import type { Action } from "./bindings/Action";
import type { RepoState } from "./bindings/RepoState";

export type SyncButton = {
    id: "fetch" | "pull" | "pull-rebase" | "push" | "force-push";
    label: string;
    action: Action;
    /** Why it cannot be used now, or null. */
    why: string | null;
    /** What it does, shown on hover when it can be used. */
    hint: string;
    danger?: boolean;
};

export type SyncInfo = {
    branch: string | null;
    upstream: string | null;
    ahead: number | null;
    behind: number | null;
    /** The remote a push goes to without asking, or null when there is none or a choice to make. */
    pushRemote: string | null;
    buttons: SyncButton[];
};

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** Ahead and behind in words, e.g. "2 to push, 1 to pull". */
export function aheadBehind(ahead: number | null, behind: number | null) {
    if (ahead === null || behind === null) return "not fetched yet";
    if (!ahead && !behind) return "up to date";
    const parts = [];
    if (ahead) parts.push(`${plural(ahead, "commit")} to push`);
    if (behind) parts.push(`${plural(behind, "commit")} to pull`);
    return parts.join(", ");
}

export function syncInfo(repo: RepoState, busy: boolean): SyncInfo {
    const branch = repo.head.branch;
    const info = repo.branches.find((b) => b.isHead && b.name === branch);
    const upstream = info?.upstream ?? null;
    const ahead = info?.ahead ?? null;
    const behind = info?.behind ?? null;
    const upstreamRemote = upstream
        ? (repo.remotes.find((r) => upstream.startsWith(`${r.name}/`))?.name ??
          null)
        : null;
    const pushRemote =
        upstreamRemote ??
        (repo.remotes.length === 1 ? repo.remotes[0].name : null);

    const none =
        repo.remotes.length === 0 ? "This project has no remote" : null;
    const mid = repo.inProgress
        ? "Finish or abort the operation in progress first"
        : null;
    const onBranch = !branch
        ? "Check out a branch first: you are not on one right now"
        : !repo.head.commit
          ? "Make a first commit first"
          : null;
    const st = repo.status;
    const tracked =
        !!st &&
        st.staged.length + st.unstaged.length + st.conflicted.length > 0;
    const noUpstream =
        branch && !upstream
            ? `${branch} does not track a remote branch yet: push it first`
            : null;
    const gone =
        upstream && ahead === null
            ? `${upstream} is not there: fetch, or push to create it`
            : null;
    const pullWhy = none ?? mid ?? onBranch ?? noUpstream ?? gone;
    const pushBase =
        none ??
        mid ??
        onBranch ??
        (pushRemote
            ? null
            : "This project has several remotes and the branch tracks none of them: push it once from a terminal with -u to choose");

    const buttons: SyncButton[] = [
        {
            id: "fetch",
            label: "Fetch",
            action: { type: "fetch", remote: null },
            why: none,
            hint: "Download new commits from every remote, without changing your branches or files",
        },
        {
            id: "pull",
            label: "Pull",
            action: { type: "pull", rebase: false },
            why:
                pullWhy ??
                (behind === 0
                    ? `${branch} already has everything from ${upstream} (as of the last fetch)`
                    : null),
            hint: `Fetch ${upstream ?? "the upstream"} and bring its commits into ${branch ?? "your branch"}, with a merge commit if both have new work`,
        },
        {
            id: "pull-rebase",
            label: "Pull (rebase)",
            action: { type: "pull", rebase: true },
            why:
                pullWhy ??
                (behind === 0
                    ? `${branch} already has everything from ${upstream} (as of the last fetch)`
                    : tracked
                      ? "Commit or stash your changes first: a rebase needs a clean working tree"
                      : null),
            hint: `Fetch ${upstream ?? "the upstream"} and move ${branch ?? "your branch"}'s own commits on top of it. They get new ids; a backup is taken first`,
            danger: true,
        },
        {
            id: "push",
            label: "Push",
            action: { type: "push", remote: null, force: false },
            why:
                pushBase ??
                (upstream && ahead === 0
                    ? `Nothing to push: ${upstream} already has every commit of ${branch}`
                    : upstream && behind
                      ? `${upstream} has ${plural(behind, "commit")} ${branch} lacks: pull first, or force push to replace them`
                      : null),
            hint: upstream
                ? `Send ${branch}'s new commits to ${upstream}`
                : `Create ${branch} on ${pushRemote ?? "the remote"} and track it from then on`,
        },
        {
            id: "force-push",
            label: "Force push…",
            action: { type: "push", remote: null, force: true },
            why:
                pushBase ??
                (!upstream || ahead === null
                    ? `${branch} is not on the remote yet: use Push`
                    : !behind
                      ? "A normal Push is enough: nothing on the remote would be replaced"
                      : null),
            hint: `Make ${upstream ?? "the remote branch"} match ${branch ?? "your branch"}, replacing the commits only it has. It stops if the remote changed since your last fetch; a backup is taken first`,
            danger: true,
        },
    ];
    if (busy) {
        for (const b of buttons) b.why ??= "Another action is running";
    }
    return { branch, upstream, ahead, behind, pushRemote, buttons };
}

/** Why a tag cannot be pushed, or null. */
export function pushTagBlocked(
    repo: RepoState,
    tag: string,
    remote: string | null,
    busy: boolean,
): string | null {
    if (!repo.remotes.length) return "This project has no remote";
    if (!repo.tags.length) return "This project has no tags";
    if (!tag) return "Choose a tag first";
    if (!remote) return "Choose a remote first";
    if (busy) return "Another action is running";
    return null;
}
