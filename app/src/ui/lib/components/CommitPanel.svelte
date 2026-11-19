<script lang="ts">
    // The selected commit: message, people, parents, refs and changed files; the chosen file's
    // diff or blame beside it.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ChangedFile } from "../bindings/ChangedFile";
    import type { CommitDetail } from "../bindings/CommitDetail";
    import type { FileStatus } from "../bindings/FileStatus";
    import type { RefLabel } from "../bindings/RefLabel";
    import { formatOffset, formatTime, shortId } from "../history";
    import BlameView from "./BlameView.svelte";
    import DiffView from "./DiffView.svelte";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        commit,
        refs,
        diffMode = $bindable("unified"),
        onjump,
        onhistory,
    }: {
        projectId: string;
        commit: string;
        refs: RefLabel[];
        diffMode?: "unified" | "split";
        onjump: (commit: string) => void;
        /** Show the history of one file. */
        onhistory: (path: string) => void;
    } = $props();

    let detail = $state<CommitDetail | null>(null);
    let error = $state<AppError | null>(null);
    let parent = $state<number | null>(null);
    let file = $state<ChangedFile | null>(null);
    let tab = $state<"diff" | "blame">("diff");
    let request = 0;

    // A new commit starts from its first parent.
    $effect.pre(() => {
        void commit;
        parent = null;
    });

    $effect(() => {
        const [p, c, par] = [projectId, commit, parent];
        const mine = ++request;
        error = null;
        api.commitDetail(p, c, par).then(
            (d) => {
                if (mine !== request) return;
                const keep = file && d.files.find((f) => f.path === file?.path);
                detail = d;
                file = keep ?? d.files[0] ?? null;
            },
            (e) => {
                if (mine === request) {
                    error = e as AppError;
                    detail = null;
                }
            },
        );
    });

    const STATUS: Record<FileStatus, [string, string]> = {
        added: ["A", "Added"],
        deleted: ["D", "Deleted"],
        modified: ["M", "Modified"],
        renamed: ["R", "Renamed"],
        typeChanged: ["T", "Type changed"],
    };
    const body = $derived(
        detail
            ? detail.message
                  .slice(detail.subject.length)
                  .replace(/^\s*\n/, "")
                  .trimEnd()
            : "",
    );
    const sameCommitter = $derived(
        detail !== null &&
            detail.author.name === detail.committer.name &&
            detail.author.email === detail.committer.email &&
            detail.author.time === detail.committer.time,
    );
</script>

<div class="panel">
    <div class="layout">
        <div class="info">
            {#if error}
                <ErrorBox {error} />
            {:else if detail}
                <h3>{detail.subject}</h3>
                {#if body}<pre class="body">{body}</pre>{/if}
                <dl>
                    <dt>Commit</dt>
                    <dd><code class="sel">{detail.id}</code></dd>
                    <dt>Author</dt>
                    <dd>
                        {detail.author.name} &lt;{detail.author.email}&gt;<br />
                        <span class="muted">
                            {formatTime(detail.author.time)} ({formatOffset(
                                detail.author.offset,
                            )})
                        </span>
                    </dd>
                    {#if !sameCommitter}
                        <dt>Committer</dt>
                        <dd>
                            {detail.committer.name} &lt;{detail.committer
                                .email}&gt;<br />
                            <span class="muted">
                                {formatTime(detail.committer.time)}
                            </span>
                        </dd>
                    {/if}
                    <dt>
                        {detail.parents.length === 1 ? "Parent" : "Parents"}
                    </dt>
                    <dd>
                        {#each detail.parents as p (p)}
                            <button
                                type="button"
                                class="link"
                                onclick={() => onjump(p)}
                            >
                                {shortId(p)}
                            </button>
                        {:else}
                            none (first commit)
                        {/each}
                    </dd>
                    {#if refs.length}
                        <dt>Refs</dt>
                        <dd>{refs.map((r) => r.name).join(", ")}</dd>
                    {/if}
                </dl>
                {#if detail.parents.length > 1}
                    <label class="compare">
                        Compare with
                        <select
                            value={detail.comparedParent ?? 0}
                            onchange={(e) => (parent = +e.currentTarget.value)}
                        >
                            {#each detail.parents as p, i (p)}
                                <option value={i}>
                                    parent {i + 1} ({shortId(p)})
                                </option>
                            {/each}
                        </select>
                    </label>
                {/if}
                <div class="files-head">
                    {detail.files.length}{detail.filesTruncated ? "+" : ""} changed
                    file{detail.files.length === 1 ? "" : "s"}
                    {#if detail.statsSkipped}<span class="muted">
                            (too many for line counts)
                        </span>{/if}
                </div>
                <ul class="files" aria-label="Changed files">
                    {#each detail.files as f (f.path)}
                        <li>
                            <button
                                type="button"
                                class="file"
                                class:current={file?.path === f.path}
                                aria-current={file?.path === f.path
                                    ? "true"
                                    : undefined}
                                onclick={() => (file = f)}
                            >
                                <span
                                    class="st {f.status}"
                                    title={STATUS[f.status][1]}
                                >
                                    {STATUS[f.status][0]}
                                </span>
                                <span
                                    class="name"
                                    title={f.oldPath
                                        ? `${f.oldPath} → $<bdi>{f.path}</bdi>`
                                        : f.path}
                                >
                                    {f.path}
                                </span>
                                {#if f.binary}
                                    <span class="muted">bin</span>
                                {:else if f.additions !== null}
                                    <span class="add">+{f.additions}</span>
                                    <span class="del">−{f.deletions}</span>
                                {/if}
                            </button>
                        </li>
                    {/each}
                </ul>
            {:else}
                <p class="muted">Loading…</p>
            {/if}
        </div>

        <div class="view">
            {#if file && detail}
                <div class="tabs" role="tablist">
                    <button
                        type="button"
                        role="tab"
                        aria-selected={tab === "diff"}
                        class:on={tab === "diff"}
                        onclick={() => (tab = "diff")}
                    >
                        Diff
                    </button>
                    <button
                        type="button"
                        role="tab"
                        aria-selected={tab === "blame"}
                        class:on={tab === "blame"}
                        disabled={file.status === "deleted"}
                        title={file.status === "deleted"
                            ? "The file was deleted here"
                            : undefined}
                        onclick={() => (tab = "blame")}
                    >
                        Blame
                    </button>
                    <button
                        type="button"
                        onclick={() => file && onhistory(file.path)}
                    >
                        File history
                    </button>
                </div>
                {#if tab === "blame" && file.status !== "deleted"}
                    <BlameView
                        {projectId}
                        commit={detail.id}
                        path={file.path}
                        {onjump}
                    />
                {:else}
                    <DiffView
                        {projectId}
                        commit={detail.id}
                        parent={detail.comparedParent}
                        {file}
                        bind:mode={diffMode}
                    />
                {/if}
            {:else if detail}
                <p class="muted pad">No files changed.</p>
            {/if}
        </div>
    </div>
</div>

<style>
    /* Lays out by its own width, not the window's: the project sidebar takes part of the window. */
    .panel {
        container-type: inline-size;
        height: 100%;
        min-height: 0;
    }
    .layout {
        display: grid;
        grid-template-columns: clamp(15rem, 42%, 28rem) minmax(0, 1fr);
        height: 100%;
        min-height: 0;
    }
    .info {
        min-height: 0;
        padding: 0.6rem 0.75rem;
        overflow: auto;
        border-right: 1px solid var(--border);
    }
    h3 {
        margin: 0 0 0.4rem;
        font-size: 1rem;
        word-break: break-word;
    }
    .body {
        margin: 0 0 0.6rem;
        font-family: inherit;
        white-space: pre-wrap;
        word-break: break-word;
    }
    dl {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.25rem 0.75rem;
        margin: 0 0 0.6rem;
    }
    dt {
        color: var(--text-muted);
    }
    dd {
        margin: 0;
        word-break: break-word;
    }
    .sel {
        font-size: 12px;
        word-break: break-all;
        user-select: all;
    }
    .muted {
        color: var(--text-muted);
    }
    .pad {
        padding: 0.6rem;
    }
    .link {
        margin-right: 0.5rem;
        padding: 0;
        border: none;
        background: none;
        color: var(--accent);
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
    }
    .compare {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 0.25rem 0.5rem;
        margin-bottom: 0.6rem;
    }
    .files-head {
        margin-bottom: 0.25rem;
        font-weight: 600;
    }
    .files {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .file {
        display: flex;
        gap: 0.5rem;
        width: 100%;
        padding: 0.15rem 0.35rem;
        border: none;
        border-radius: 3px;
        background: none;
        text-align: left;
    }
    .file:hover {
        background: var(--surface-2);
    }
    .file.current {
        background: var(--surface-2);
        box-shadow: inset 2px 0 var(--accent);
    }
    .st {
        width: 1ch;
        font-family: ui-monospace, monospace;
        font-weight: 700;
    }
    .st.added {
        color: var(--success);
    }
    .st.deleted {
        color: var(--danger);
    }
    .st.modified,
    .st.typeChanged {
        color: var(--warning);
    }
    .st.renamed {
        color: var(--accent);
    }
    .name {
        flex: 1;
        overflow: hidden;
        direction: rtl;
        text-align: left;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .add {
        color: var(--success);
    }
    .del {
        color: var(--danger);
    }
    .view {
        display: flex;
        flex-direction: column;
        min-width: 0;
        min-height: 0;
    }
    .tabs {
        display: flex;
        gap: 0.25rem;
        padding: 0.35rem 0.5rem 0;
    }
    .tabs button {
        padding: 0.2rem 0.7rem;
    }
    .tabs button.on {
        border-color: var(--accent);
        color: var(--accent);
    }
    .view > :global(:last-child) {
        flex: 1;
        min-height: 0;
    }
    @container (max-width: 34rem) {
        .layout {
            grid-template-columns: 1fr;
            grid-template-rows: auto 1fr;
        }
        .info {
            max-height: 40%;
            border-right: none;
            border-bottom: 1px solid var(--border);
        }
    }
</style>
