<script lang="ts">
    // Uncommitted work: conflicted, staged and unstaged files with stage, unstage and discard,
    // the selected file's diff, and the commit message editor (with amend). Every write goes
    // through the action runner, so discard and amend are confirmed and backed up first.
    import { onMount, untrack } from "svelte";
    import { actions, outcomeNotice } from "../actions.svelte";
    import { api } from "../api";
    import type { Action } from "../bindings/Action";
    import type { AppError } from "../bindings/AppError";
    import type { ChangedFile } from "../bindings/ChangedFile";
    import type { GitVersion } from "../bindings/GitVersion";
    import type { Hunk } from "../bindings/Hunk";
    import type { RepoState } from "../bindings/RepoState";
    import type { StashEntry } from "../bindings/StashEntry";
    import type { WorkSide } from "../bindings/WorkSide";
    import {
        changeMenu,
        fileName,
        ignorePatterns,
        type ChangeCommand,
    } from "../changeMenu";
    import { offerMenu } from "../contextMenu";
    import { operationLabel } from "../format";
    import {
        changeRows,
        commitProblem,
        followSelection,
        kindLetter,
        type ChangeRow,
        type Selection,
    } from "../changes";
    import { stashStillListed } from "../stash";
    import ConflictPanel from "./ConflictPanel.svelte";
    import DiffView from "./DiffView.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import StashDiff from "./StashDiff.svelte";
    import Splitter from "./Splitter.svelte";
    import StashPanel from "./StashPanel.svelte";

    let {
        projectId,
        repo,
        changed,
        message = $bindable(""),
        amend = $bindable(false),
        diffMode = $bindable("unified"),
        onreload,
        onopenfile,
    }: {
        projectId: string;
        repo: RepoState;
        /** Bumped when the project's files change on disk. */
        changed: number;
        message?: string;
        amend?: boolean;
        diffMode?: "unified" | "split";
        onreload: () => Promise<void>;
        /** Open a file in the Files tab. */
        onopenfile: (path: string) => void;
    } = $props();

    /** What the user clicked; `selected` follows the file as it moves between lists. */
    let picked = $state<Selection | null>(null);
    let error = $state<AppError | null>(null);
    let notice = $state("");
    /** The stash shown on the right, by commit id (its position changes as stashes come and go). */
    let stashPicked = $state<string | null>(null);
    let stashes = $state<StashEntry[]>([]);
    let gitVersion = $state<GitVersion | null>(null);

    // Pane sizes, as fractions of their container so they follow the window. Null is the default;
    // a size the user drags to is remembered (Splitter).
    let box = $state<HTMLDivElement>();
    let boxW = $state(0);
    let sideCol = $state<HTMLDivElement>();
    let sideH = $state(0);
    let stashBox = $state<HTMLDivElement>();
    let stashBoxH = $state(0);
    /** Width of the file list beside the diff. */
    let sideW = $state<number | null>(null);
    /** Height of the stash section under the file lists. */
    let stashH = $state<number | null>(null);
    // The file list never gets narrower than its rows and buttons need, nor does the diff.
    const STASH_MIN = 96;
    const LISTS_MIN = 160;
    const COMMIT_MIN = 150;
    const SIDE_MIN = 256;
    const DIFF_MIN = 320;
    const SIDE_DEFAULT = `clamp(${SIDE_MIN}px, 30%, 28rem)`;

    // Only the staged-only option depends on it; unknown means "let git decide".
    onMount(() => {
        api.gitCheck().then(
            (g) => (gitVersion = g.version),
            () => {},
        );
    });
    const stash = $derived(
        stashStillListed(stashPicked, stashes)
            ? (stashes.find((s) => s.commit === stashPicked) ?? null)
            : null,
    );

    const rows = $derived(changeRows(repo.status));
    const conflicted = $derived(rows.filter((r) => r.side === "conflicted"));
    const staged = $derived(rows.filter((r) => r.side === "staged"));
    const unstaged = $derived(rows.filter((r) => r.side === "unstaged"));
    const busy = $derived(!!actions.busy[projectId]);
    const unborn = $derived(!repo.head.commit);
    // git refuses to amend in the middle of a merge, and so does the backend.
    const merging = $derived(repo.inProgress === "merge");
    const mergeMessage = $derived(repo.mergeMessage);
    const stashHint = $derived(
        unborn
            ? "There is no commit yet to stash against"
            : merging
              ? "Finish or abort the merge first"
              : conflicted.length
                ? "Resolve the conflicts first"
                : !rows.length
                  ? "There are no changes to stash"
                  : "",
    );

    // A merge starts the editor from git's prepared message, unless something is typed already.
    $effect(() => {
        const m = mergeMessage;
        if (m && !untrack(() => message.trim())) message = m;
    });
    $effect(() => {
        if (merging) untrack(() => (amend = false));
    });
    const problem = $derived(
        commitProblem(message, amend, staged.length, conflicted.length),
    );

    const selected = $derived(followSelection(picked, rows));

    // Lines added and removed per file, counted by the backend after each change of the lists. A
    // binary or very large file has no count and shows none.
    let counts = $state<Record<string, { add: number; del: number }>>({});
    let statsRequest = 0;
    const countKey = (side: string, path: string) => `${side}:${path}`;
    $effect(() => {
        const files = rows
            .filter((r) => r.side !== "conflicted")
            .map((r): [string, WorkSide] => [
                r.path,
                r.side === "staged" ? "staged" : "unstaged",
            ]);
        const mine = ++statsRequest;
        if (!files.length) {
            counts = {};
            return;
        }
        api.workStats(projectId, files).then(
            (list) => {
                if (mine !== statsRequest) return;
                const next: typeof counts = {};
                for (const s of list) {
                    if (s.additions !== null && s.deletions !== null)
                        next[countKey(s.side, s.path)] = {
                            add: s.additions,
                            del: s.deletions,
                        };
                }
                counts = next;
            },
            () => {
                if (mine === statsRequest) counts = {};
            },
        );
    });

    const selectedFile = $derived.by((): ChangedFile | null => {
        if (!selected) return null;
        return {
            path: selected.path,
            oldPath: null,
            status: "modified",
            additions: null,
            deletions: null,
            binary: false,
            similarity: null,
        };
    });
    const selectedSide = $derived<WorkSide>(
        selected?.side === "staged" ? "staged" : "unstaged",
    );

    async function act(action: Action, done?: string) {
        error = null;
        notice = "";
        try {
            const outcome = await actions.run(projectId, action);
            if (!outcome) return false;
            notice = outcomeNotice(outcome, done).text;
            return true;
        } catch (e) {
            error = e as AppError;
            return false;
        } finally {
            await onreload();
        }
    }

    const paths = (list: ChangeRow[]) => list.map((r) => r.path);
    const stage = (p: string[]) => act({ type: "stage", paths: p });
    const unstage = (p: string[]) => act({ type: "unstage", paths: p });
    const discard = (p: string[]) => act({ type: "discard", paths: p });

    /** Stage or unstage one hunk of the selected file; conflicts are resolved whole. */
    const hunkAction = $derived.by(() => {
        if (!selected || selected.side === "conflicted") return null;
        const path = selected.path;
        const unstaging = selected.side === "staged";
        return {
            label: unstaging ? "Unstage hunk" : "Stage hunk",
            title: unstaging
                ? "Take this hunk out of the staging area; the file on disk is kept"
                : "Stage only this hunk",
            disabled: busy,
            run: (h: Hunk) =>
                act({
                    type: unstaging ? "unstageHunks" : "stageHunks",
                    path,
                    hunks: [h.id],
                }),
        };
    });

    /** Right-click on a file: select it and offer its actions. */
    function rowMenu(e: MouseEvent, r: ChangeRow) {
        picked = { path: r.path, side: r.side };
        stashPicked = null;
        offerMenu(e, {
            items: changeMenu({
                path: r.path,
                side: r.side,
                busy,
                untracked: r.kind === "untracked",
            }),
            run: (id) => {
                const cmd = id as ChangeCommand;
                if (cmd === "stage" || cmd === "resolved") void stage([r.path]);
                else if (cmd === "unstage") void unstage([r.path]);
                else if (cmd === "discard") void discard([r.path]);
                else if (cmd === "open-file") onopenfile(r.path);
                else if (cmd === "ignore-file" || cmd === "ignore-ext") {
                    const p = ignorePatterns(r.path);
                    const pattern = cmd === "ignore-file" ? p.file : p.ext;
                    if (pattern)
                        void act(
                            { type: "ignore", pattern },
                            `Added ${pattern} to .gitignore.`,
                        );
                } else if (cmd === "show-folder")
                    api.openFolder(projectId, r.path).catch(
                        (e) => (error = e as AppError),
                    );
                else if (cmd === "copy-path")
                    void navigator.clipboard.writeText(r.path);
                else if (cmd === "copy-name")
                    void navigator.clipboard.writeText(fileName(r.path));
            },
        });
    }

    async function commit() {
        if (problem || busy) return;
        const ok = await act(
            { type: "commit", message, amend },
            amend ? "Commit amended." : "Committed.",
        );
        if (ok) {
            message = "";
            amend = false;
        }
    }

    /** Amending starts from the commit's own message when nothing is typed yet. */
    async function toggleAmend() {
        if (!amend || message.trim() || !repo.head.commit) return;
        try {
            const d = await api.commitDetail(projectId, repo.head.commit, null);
            if (amend && !message.trim()) message = d.message.trimEnd();
        } catch (e) {
            error = e as AppError;
        }
    }

    function editorKey(e: KeyboardEvent) {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            void commit();
        }
    }

    const subject = $derived(message.split("\n", 1)[0]);
    const dir = (p: string) => p.slice(0, p.lastIndexOf("/") + 1);
    const base = (p: string) => p.slice(p.lastIndexOf("/") + 1);
    const isSel = (r: ChangeRow) =>
        selected?.path === r.path && selected.side === r.side;
</script>

{#snippet fileRow(r: ChangeRow)}
    <li class:sel={isSel(r)} oncontextmenu={(e) => rowMenu(e, r)}>
        <button
            type="button"
            class="file"
            aria-current={isSel(r) ? "true" : undefined}
            title={r.path}
            onclick={() => {
                picked = { path: r.path, side: r.side };
                stashPicked = null;
            }}
        >
            <span class="kind {r.kind}" title={r.kind}>
                {kindLetter(r.kind)}
            </span>
            <span class="name">
                <span class="dir">{dir(r.path)}</span>{base(r.path)}
            </span>
            {#if counts[countKey(r.side, r.path)]}
                {@const c = counts[countKey(r.side, r.path)]}
                <span
                    class="lines"
                    title={`${c.add} ${c.add === 1 ? "line" : "lines"} added, ${c.del} removed`}
                >
                    <span class="add">+{c.add}</span>
                    <span class="del">−{c.del}</span>
                </span>
            {/if}
        </button>
        <span class="row-actions">
            {#if r.side === "staged"}
                <button
                    type="button"
                    disabled={busy}
                    title="Unstage"
                    aria-label={`Unstage ${r.path}`}
                    onclick={() => unstage([r.path])}
                >
                    −
                </button>
            {:else if r.side === "conflicted"}
                <button
                    type="button"
                    disabled={busy}
                    title="Mark resolved (stage the file as it is now)"
                    aria-label={`Mark ${r.path} resolved`}
                    onclick={() => stage([r.path])}
                >
                    ✓
                </button>
            {:else}
                <button
                    type="button"
                    disabled={busy}
                    title="Discard changes"
                    aria-label={`Discard changes to ${r.path}`}
                    onclick={() => discard([r.path])}
                >
                    ↺
                </button>
                <button
                    type="button"
                    disabled={busy}
                    title="Stage"
                    aria-label={`Stage ${r.path}`}
                    onclick={() => stage([r.path])}
                >
                    +
                </button>
            {/if}
        </span>
    </li>
{/snippet}

<div
    class="changes"
    bind:this={box}
    bind:clientWidth={boxW}
    style:--side={sideW === null
        ? SIDE_DEFAULT
        : `clamp(${SIDE_MIN}px, ${sideW * 100}%, calc(100% - ${DIFF_MIN}px))`}
>
    <div class="side" bind:this={sideCol} bind:clientHeight={sideH}>
        <div class="lists">
            {#if repo.inProgress}
                <div class="banner">
                    <span>
                        {operationLabel(repo.inProgress)} in progress.
                        {#if conflicted.length}
                            Git could not combine every change. Click each
                            conflicted file to see why and settle it right here,
                            then commit.
                        {:else}
                            Commit to finish it.
                        {/if}
                    </span>
                    {#if repo.inProgress === "merge"}
                        <button
                            title="Cancel the merge and go back to how things were before it started. A backup is taken first"
                            type="button"
                            disabled={busy}
                            onclick={() => act({ type: "abortMerge" })}
                        >
                            Abort merge
                        </button>
                    {/if}
                </div>
            {/if}
            {#if error}
                <ErrorBox {error} />
            {/if}
            {#if notice}
                <p class="notice" role="status">{notice}</p>
            {/if}

            {#if conflicted.length}
                <section>
                    <h3>
                        <span>
                            Conflicted <span class="n">
                                {conflicted.length}
                            </span>
                        </span>
                    </h3>
                    <ul>
                        {#each conflicted as r (r.path)}
                            {@render fileRow(r)}
                        {/each}
                    </ul>
                </section>
            {/if}
            <section>
                <h3>
                    <span>Staged <span class="n">{staged.length}</span></span>
                    {#if staged.length}
                        <button
                            title="Take every file out of the next commit. Your changes stay in the files"
                            type="button"
                            disabled={busy}
                            onclick={() => unstage(paths(staged))}
                        >
                            Unstage all
                        </button>
                    {/if}
                </h3>
                <ul>
                    {#each staged as r (r.path)}
                        {@render fileRow(r)}
                    {:else}
                        <li class="empty">Nothing staged.</li>
                    {/each}
                </ul>
            </section>
            <section>
                <h3>
                    <span>
                        Changes <span class="n">{unstaged.length}</span>
                    </span>
                    {#if unstaged.length}
                        <span class="group">
                            <button
                                title="Throw away every change you have not staged. A backup is taken first"
                                type="button"
                                disabled={busy}
                                onclick={() => discard(paths(unstaged))}
                            >
                                Discard all
                            </button>
                            <button
                                title="Add every changed file to the next commit"
                                type="button"
                                disabled={busy}
                                onclick={() => stage(paths(unstaged))}
                            >
                                Stage all
                            </button>
                        </span>
                    {/if}
                </h3>
                <ul>
                    {#each unstaged as r (r.path)}
                        {@render fileRow(r)}
                    {:else}
                        <li class="empty">No changes.</li>
                    {/each}
                </ul>
            </section>
        </div>

        <Splitter
            box={sideCol}
            axis="y"
            from="end"
            bind:value={stashH}
            current={sideH ? stashBoxH / sideH : 0.3}
            min={0.1}
            max={0.7}
            minPx={STASH_MIN}
            endMinPx={LISTS_MIN + COMMIT_MIN}
            label="Resize the stash list"
            storageKey="changes.stash"
        />
        <div
            class="stashbox"
            class:sized={stashH !== null}
            bind:this={stashBox}
            bind:clientHeight={stashBoxH}
            style:height={stashH === null ? undefined : `${stashH * sideH}px`}
        >
            <StashPanel
                {projectId}
                {changed}
                {busy}
                canStash={!stashHint}
                {stashHint}
                selected={stash?.commit ?? null}
                bind:list={stashes}
                onselect={(c) => (stashPicked = c)}
                {act}
                {gitVersion}
            />
        </div>

        <div class="commit">
            <textarea
                title="Describe the change. The first line is the summary shown in the history"
                bind:value={message}
                onkeydown={editorKey}
                rows="4"
                spellcheck="true"
                placeholder="Commit message (the first line is the summary)"
                aria-label="Commit message"></textarea>
            <div class="commit-bar">
                <label
                    title={unborn
                        ? "There is no commit to amend yet"
                        : merging
                          ? "Finish or abort the merge before amending"
                          : ""}
                >
                    <input
                        title="Replace the last commit with this one instead of making a new commit. Avoid this if the last commit is already pushed"
                        type="checkbox"
                        bind:checked={amend}
                        disabled={unborn || merging || busy}
                        onchange={toggleAmend}
                    />
                    Amend last commit
                </label>
                <span
                    class="count"
                    class:long={subject.length > 72}
                    title="Summary length; keep it under 72 characters"
                >
                    {subject.length}
                </span>
                <button
                    type="button"
                    class="primary"
                    disabled={!!problem || busy}
                    title={problem ?? "Ctrl+Enter"}
                    onclick={commit}
                >
                    {amend
                        ? "Amend"
                        : `Commit${repo.head.branch ? ` to ${repo.head.branch}` : ""}`}
                </button>
            </div>
        </div>
    </div>

    <Splitter
        {box}
        axis="x"
        bind:value={sideW}
        current={boxW && sideCol ? sideCol.offsetWidth / boxW : 0.3}
        min={0.15}
        max={0.7}
        minPx={SIDE_MIN}
        endMinPx={DIFF_MIN}
        label="Resize the file list"
        storageKey="changes.side"
    />

    <div class="diff">
        {#if stash}
            <StashDiff {projectId} {stash} bind:diffMode />
        {:else if selected?.side === "conflicted"}
            <ConflictPanel
                {projectId}
                path={selected.path}
                version={changed}
                {onreload}
                {onopenfile}
            />
        {:else if selectedFile}
            <DiffView
                {projectId}
                work={selectedSide}
                version={changed}
                file={selectedFile}
                bind:mode={diffMode}
                {hunkAction}
            />
        {:else}
            <p class="muted pad">
                {rows.length
                    ? "Select a file to see its changes."
                    : "The working tree is clean."}
            </p>
        {/if}
    </div>
</div>

<style>
    .changes {
        display: grid;
        grid-template-columns: var(--side) 6px minmax(0, 1fr);
        height: 100%;
        min-height: 0;
    }
    .side {
        display: flex;
        flex-direction: column;
        min-height: 0;
        min-width: 0;
    }
    /* The stash section scrolls inside its own box, which the user can resize. Unsized, it
       takes what its content needs, up to 40% of the column. */
    .stashbox {
        flex: none;
        min-height: 6rem;
        max-height: 40%;
        padding: 0 0.6rem 0.6rem;
        overflow: auto;
    }
    .stashbox.sized {
        max-height: max(6rem, calc(100% - 20rem));
    }
    .lists {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        align-content: start;
        gap: 0.75rem;
        flex: 1 1 0;
        min-height: 10rem;
        padding: 0.6rem;
        overflow: auto;
    }
    .banner {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0.6rem;
        padding: 0.5rem 0.6rem;
        border-left: 3px solid var(--warning);
        border-radius: 4px;
        background: var(--surface-2);
    }
    .banner span {
        flex: 1 1 10rem;
    }
    .notice {
        margin: 0;
        color: var(--success);
        font-size: 0.9em;
    }
    h3 {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        justify-content: space-between;
        gap: 0.5rem;
        margin: 0 0 0.3rem;
        color: var(--text-muted);
        font-size: 0.8rem;
        font-weight: 600;
        letter-spacing: 0.04em;
        text-transform: uppercase;
    }
    h3 button {
        padding: 0.1rem 0.5rem;
        font-size: 0.75rem;
        letter-spacing: normal;
        text-transform: none;
    }
    .group {
        display: flex;
        gap: 0.3rem;
    }
    .n {
        margin-left: 0.3rem;
        font-weight: 400;
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        align-items: center;
        border-radius: 4px;
    }
    li:hover,
    li.sel {
        background: var(--surface-2);
    }
    li.sel {
        box-shadow: inset 2px 0 var(--accent);
    }
    li.empty {
        padding: 0.2rem 0.4rem;
        color: var(--text-muted);
        font-size: 0.9em;
    }
    li.empty:hover {
        background: none;
    }
    .file {
        display: flex;
        flex: 1;
        align-items: center;
        gap: 0.45rem;
        min-width: 0;
        padding: 0.2rem 0.4rem;
        border: none;
        background: none;
        text-align: left;
    }
    .kind {
        flex-shrink: 0;
        width: 1rem;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-weight: 700;
        text-align: center;
    }
    .kind.added,
    .kind.untracked {
        color: var(--success);
    }
    .kind.deleted,
    .kind.conflicted {
        color: var(--danger);
    }
    .kind.modified,
    .kind.typeChange {
        color: var(--warning);
    }
    .kind.renamed {
        color: var(--accent);
    }
    .name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .dir {
        color: var(--text-muted);
    }
    /* The +/− count stays whole and at the right edge; the file name shortens instead. */
    .lines {
        display: flex;
        flex-shrink: 0;
        gap: 0.35rem;
        margin-left: auto;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-size: 0.8em;
        white-space: nowrap;
    }
    .lines .add {
        color: var(--success);
    }
    .lines .del {
        color: var(--danger);
    }
    .row-actions {
        display: flex;
        gap: 2px;
        padding-right: 0.25rem;
        visibility: hidden;
    }
    li:hover .row-actions,
    li.sel .row-actions,
    .row-actions:focus-within {
        visibility: visible;
    }
    .row-actions button {
        width: 1.6rem;
        padding: 0.05rem 0;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
    }
    .commit {
        display: grid;
        gap: 0.4rem;
        padding: 0.6rem;
        border-top: 1px solid var(--border);
        background: var(--surface);
    }
    textarea {
        min-height: 4.5rem;
        padding: 0.4rem 0.5rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--bg);
        color: var(--text);
        font: inherit;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-size: 12.5px;
        resize: vertical;
    }
    .commit-bar {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0.4rem 0.6rem;
    }
    /* A long branch name shortens the button instead of pushing it out of the column. */
    .commit-bar .primary {
        max-width: 100%;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* The label keeps its words on one line; when the button no longer fits beside it, it wraps
       to the next line. */
    .commit-bar label {
        display: flex;
        flex: 1 1 auto;
        white-space: nowrap;
        align-items: center;
        gap: 0.35rem;
        color: var(--text-muted);
        font-size: 0.9em;
    }
    .count {
        color: var(--text-muted);
        font-size: 0.85em;
        font-variant-numeric: tabular-nums;
    }
    .count.long {
        color: var(--warning);
    }
    .diff {
        min-width: 0;
        min-height: 0;
    }
    .muted {
        color: var(--text-muted);
    }
    .pad {
        margin: 0;
        padding: 1rem;
    }
    @container (max-width: 44rem) {
        .changes {
            grid-template-columns: minmax(0, 1fr);
            grid-template-rows: minmax(0, 1fr) minmax(0, 1fr);
        }
        .side {
            border-bottom: 1px solid var(--border);
        }
        /* One pane above the other: no side-by-side divider. */
        .changes > :global(.splitter.x) {
            display: none;
        }
    }
</style>
