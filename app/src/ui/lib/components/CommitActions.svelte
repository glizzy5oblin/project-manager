<script lang="ts">
    // Write actions on the selected commit and the branches pointing at it: new branch here,
    // reset the current branch here, and check out, merge, rename or delete a branch. All go
    // through the action runner, so dangerous ones are confirmed and backed up first.
    import { actions, outcomeNotice } from "../actions.svelte";
    import type { Action } from "../bindings/Action";
    import type { AppError } from "../bindings/AppError";
    import type { HeadState } from "../bindings/HeadState";
    import type { RefLabel } from "../bindings/RefLabel";
    import type { ResetMode } from "../bindings/ResetMode";
    import { commitCommands, type NameForm } from "../commitMenu.svelte";
    import { shortId } from "../history";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        commit,
        refs,
        head,
    }: {
        projectId: string;
        commit: string;
        refs: RefLabel[];
        head: HeadState | null;
    } = $props();

    let form = $state<NameForm | null>(null);
    let name = $state("");
    let checkout = $state(true);
    let resetMode = $state<ResetMode>("mixed");
    let error = $state<AppError | null>(null);
    let notice = $state("");
    // A notice that needs the user's attention (conflicts) is shown as a warning.
    let noticeWarn = $state(false);

    // A different commit closes an open form and clears old messages.
    $effect.pre(() => {
        void commit;
        form = null;
        error = null;
        notice = "";
        noticeWarn = false;
    });

    // A command chosen in the commit's right-click menu (HistoryView). It waits here until the
    // panel for that commit is showing, so its result and errors appear beside the commit.
    $effect(() => {
        const req = commitCommands.request;
        if (!req || req.commit !== commit) return;
        commitCommands.request = null;
        if (req.command.type === "form") open(req.command.form);
        else if (req.command.type === "action") void act(req.command.action);
    });

    const busy = $derived(!!actions.busy[projectId]);
    const current = $derived(head?.branch ?? null);
    const isHeadCommit = $derived(head?.commit === commit);
    const branches = $derived(refs.filter((r) => r.kind === "branch"));
    const remotes = $derived(refs.filter((r) => r.kind === "remote"));

    /** A reset needs a commit to move and somewhere else to move it from. */
    const resetOk = $derived(!!head?.commit && !isHeadCommit);

    /** Why a merge of this branch cannot be done here, or null when it can. */
    function mergeBlocked(r: RefLabel): string | null {
        if (!current)
            return "Check out a branch first: you are not on one right now";
        if (r.isHead) return "This is the branch you are on";
        if (isHeadCommit)
            return "Nothing to merge: your branch is already at this commit";
        return null;
    }

    async function act(action: Action) {
        error = null;
        notice = "";
        try {
            const outcome = await actions.run(projectId, action);
            if (!outcome) return false;
            ({ text: notice, warn: noticeWarn } = outcomeNotice(outcome));
            return true;
        } catch (e) {
            error = e as AppError;
            return false;
        }
    }

    function open(f: NameForm) {
        form = f;
        name = f.kind === "rename" ? f.from : "";
        checkout = true;
    }

    async function submit(e: SubmitEvent) {
        e.preventDefault();
        if (!form || !name.trim()) return;
        const n = name.trim();
        const ok = await act(
            form.kind === "create"
                ? { type: "createBranch", name: n, start: commit, checkout }
                : { type: "renameBranch", from: form.from, to: n },
        );
        if (ok) form = null;
    }

    // Not `autofocus`: Svelte applies it only while nothing has focus, and the button that
    // opened the form still has it.
    function focusOnOpen(el: HTMLInputElement) {
        el.focus();
        el.select();
    }

    function formKey(e: KeyboardEvent) {
        if (e.key === "Escape") {
            e.stopPropagation();
            form = null;
        }
    }
</script>

<section class="acts" aria-label="Actions">
    <div class="row top">
        <button
            title="Start a new branch from this commit"
            type="button"
            disabled={busy}
            onclick={() => open({ kind: "create" })}
        >
            New branch here…
        </button>
        <!-- Buttons that cannot be used here stay visible, greyed out, and say why on hover. -->
        <span class="reset">
            <button
                type="button"
                disabled={busy || !resetOk}
                title={resetOk
                    ? `Move ${current ?? "the detached HEAD"} to ${shortId(commit)}`
                    : !head?.commit
                      ? "This project has no commits yet"
                      : "Already here: the checked-out commit is this one"}
                onclick={() =>
                    act({ type: "reset", target: commit, mode: resetMode })}
            >
                Reset {current ?? "HEAD"} here
            </button>
            <select
                bind:value={resetMode}
                aria-label="Reset mode"
                title="soft keeps the changes staged, mixed keeps them unstaged, hard discards them"
                disabled={busy || !resetOk}
            >
                <option value="soft">soft</option>
                <option value="mixed">mixed</option>
                <option value="hard">hard</option>
            </select>
        </span>
    </div>

    {#each branches as b (b.full)}
        {@const mergeWhy = mergeBlocked(b)}
        <div class="row line">
            <code class="ref" class:head={b.isHead} title={b.name}>
                {b.name}
            </code>
            <button
                title={b.isHead
                    ? "You are already on this branch"
                    : "Switch your files to this branch"}
                type="button"
                disabled={busy || b.isHead}
                onclick={() => act({ type: "checkout", branch: b.name })}
            >
                Check out
            </button>
            <button
                type="button"
                class="merge"
                title={mergeWhy ?? `Merge ${b.name} into ${current}`}
                disabled={busy || mergeWhy !== null}
                onclick={() => act({ type: "merge", branch: b.name })}
            >
                Merge into {current ?? "HEAD"}
            </button>
            <button
                title="Give this branch another name"
                type="button"
                disabled={busy}
                onclick={() => open({ kind: "rename", from: b.name })}
            >
                Rename…
            </button>
            <button
                title={b.isHead
                    ? "You cannot delete the branch you are on. Check out another branch first"
                    : "Delete this branch. A backup is taken first"}
                type="button"
                disabled={busy || b.isHead}
                onclick={() => act({ type: "deleteBranch", name: b.name })}
            >
                Delete
            </button>
        </div>
    {/each}
    {#each remotes as r (r.full)}
        {@const mergeWhy = mergeBlocked(r)}
        <div class="row line">
            <code class="ref remote" title={r.name}>{r.name}</code>
            <button
                type="button"
                class="merge"
                title={mergeWhy ?? `Merge ${r.name} into ${current}`}
                disabled={busy || mergeWhy !== null}
                onclick={() => act({ type: "merge", branch: r.name })}
            >
                Merge into {current ?? "HEAD"}
            </button>
        </div>
    {/each}

    {#if form}
        <form class="row" onsubmit={submit}>
            <label class="name">
                <span>
                    {form.kind === "create"
                        ? `New branch at ${shortId(commit)}`
                        : `Rename ${form.from} to`}
                </span>
                <input
                    title="The name for the branch, with no spaces"
                    bind:value={name}
                    {@attach focusOnOpen}
                    spellcheck="false"
                    onkeydown={formKey}
                />
            </label>
            {#if form.kind === "create"}
                <label class="check">
                    <input
                        title="Switch to the new branch right away"
                        type="checkbox"
                        bind:checked={checkout}
                    />
                    Check it out
                </label>
            {/if}
            <button
                title={form.kind === "create"
                    ? "Create the branch"
                    : "Rename the branch"}
                type="submit"
                class="primary"
                disabled={busy || !name.trim()}
            >
                {form.kind === "create" ? "Create" : "Rename"}
            </button>
            <button
                title="Close this form without changing anything"
                type="button"
                onclick={() => (form = null)}
            >
                Cancel
            </button>
        </form>
    {/if}

    {#if error}
        <ErrorBox {error} />
    {/if}
    {#if notice}
        <p class="notice" class:warn={noticeWarn} role="status">{notice}</p>
    {/if}
</section>

<style>
    .acts {
        display: grid;
        gap: 0.4rem;
        margin: 0.75rem 0;
        padding-top: 0.6rem;
        border-top: 1px solid var(--border);
    }
    .row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0.35rem;
    }
    /* The buttons of a row stay on one line; the commit panel is kept wide enough for that
       (the panel widens by itself when they would not fit). Only the branch name gives way, with
       an ellipsis. */
    .row.top,
    .row.line {
        flex-wrap: nowrap;
    }
    /* Every branch name takes the same width, so the buttons of all rows line up in columns. */
    .row.line .ref {
        flex: 0 0 9rem;
        box-sizing: border-box;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .merge {
        white-space: nowrap;
    }
    button,
    select {
        padding: 0.15rem 0.55rem;
        font-size: 0.85rem;
    }
    select {
        /* Room for the arrow drawn in app.css. */
        padding-right: 1.7rem;
    }
    .reset {
        display: inline-flex;
        gap: 0.25rem;
    }
    .ref {
        padding: 0.05rem 0.4rem;
        border-radius: 3px;
        background: var(--surface-2);
        font-size: 0.85em;
    }
    .ref.head {
        box-shadow: inset 0 0 0 1px var(--accent);
    }
    .ref.remote {
        color: var(--text-muted);
    }
    .name {
        display: grid;
        flex: 1 1 12rem;
        gap: 0.2rem;
    }
    .name span {
        color: var(--text-muted);
        font-size: 0.85em;
    }
    .check {
        display: flex;
        align-items: center;
        gap: 0.3rem;
        font-size: 0.9em;
    }
    .notice {
        margin: 0;
        color: var(--success);
        font-size: 0.9em;
    }
    .notice.warn {
        color: var(--warning);
    }
</style>
