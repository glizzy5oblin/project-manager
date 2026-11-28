<script lang="ts">
    // The selected commit: message, people, parents, refs and changed files; the chosen file's
    // diff or blame beside it.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ChangedFile } from "../bindings/ChangedFile";
    import type { CommitDetail } from "../bindings/CommitDetail";
    import type { FileStatus } from "../bindings/FileStatus";
    import type { RefLabel } from "../bindings/RefLabel";
    import type { RepoSide } from "../bindings/RepoSide";
    import { formatOffset, formatTime, shortId } from "../history";
    import type { HeadState } from "../bindings/HeadState";
    import BlameView from "./BlameView.svelte";
    import CommitActions from "./CommitActions.svelte";
    import DiffView from "./DiffView.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import Splitter from "./Splitter.svelte";

    let {
        projectId,
        side = "private",
        commit,
        refs,
        head,
        diffMode = $bindable("unified"),
        onjump,
        onhistory,
        onclose,
    }: {
        projectId: string;
        /** Which repository of a two-repo project this commit belongs to. */
        side?: RepoSide;
        commit: string;
        refs: RefLabel[];
        /** The repository's HEAD, for the write actions. */
        head: HeadState | null;
        diffMode?: "unified" | "split";
        onjump: (commit: string) => void;
        /** Show the history of one file. */
        onhistory: (path: string) => void;
        /** Close the panel. */
        onclose: () => void;
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
        const [p, c, par, s] = [projectId, commit, parent, side];
        const mine = ++request;
        error = null;
        api.commitDetail(p, c, par, s).then(
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
    // Pane sizes as fractions of the panel (Splitter): the width of the details beside the diff,
    // or their height above it when the panel is narrow. Null is the default.
    let layout = $state<HTMLDivElement>();
    let layoutW = $state(0);
    let layoutH = $state(0);
    let infoPxW = $state(0);
    let infoPxH = $state(0);
    // The details never get narrower than their label column and text need, nor does the diff.
    const INFO_MIN = 480;
    // A size the user dragged to may go well below that: the details scroll when they are cut.
    const INFO_DRAG_MIN = 220;
    const VIEW_MIN = 240;
    let infoW = $state<number | null>(null);
    // The details widen by themselves when a button row or text field would be cut off. This is
    // the width the content asked for; the 480px minimum stays as the floor.
    let infoEl = $state<HTMLDivElement>();
    let needW = $state(0);
    const infoMin = $derived(Math.max(INFO_MIN, needW));
    $effect(() => {
        // Measure again when the commit, its branches or the width change.
        void [commit, refs, detail, infoPxW];
        const el = infoEl;
        if (!el) return;
        const frame = requestAnimationFrame(() => {
            const over = el.scrollWidth - el.clientWidth;
            if (over > 1) needW = Math.max(needW, el.clientWidth + over);
        });
        return () => cancelAnimationFrame(frame);
    });
    // A different commit starts from the minimum again.
    $effect(() => {
        void commit;
        needW = 0;
    });
    let infoH = $state<number | null>(null);

    const sameCommitter = $derived(
        detail !== null &&
            detail.author.name === detail.committer.name &&
            detail.author.email === detail.committer.email &&
            detail.author.time === detail.committer.time,
    );
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions (Escape closes the panel from anywhere inside it, except while a form is open) -->
<div
    class="panel"
    role="region"
    aria-label="Commit details"
    onkeydown={(e) => {
        if (e.key === "Escape" && !e.defaultPrevented) onclose();
    }}
>
    <button
        type="button"
        class="close"
        title="Close the commit details (Esc)"
        aria-label="Close the commit details"
        onclick={onclose}
    >
        ✕
    </button>
    <div
        class="layout"
        bind:this={layout}
        bind:clientWidth={layoutW}
        bind:clientHeight={layoutH}
        style:--info={infoW === null
            ? `clamp(${infoMin}px, 42%, 32rem)`
            : `clamp(${INFO_DRAG_MIN}px, ${infoW * 100}%, calc(100% - ${VIEW_MIN}px))`}
    >
        <div
            class="info"
            bind:this={infoEl}
            class:sized={infoH !== null}
            bind:clientWidth={infoPxW}
            bind:clientHeight={infoPxH}
            style:height={infoH === null ? undefined : `${infoH * layoutH}px`}
        >
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
                                title={`Go to the parent commit ${shortId(p)}`}
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
                <CommitActions
                    {projectId}
                    {side}
                    commit={detail.id}
                    {refs}
                    {head}
                />
                {#if detail.parents.length > 1}
                    <label class="compare">
                        Compare with
                        <select
                            title="Choose which parent this merge commit is compared with"
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
                                title={`Show the changes made to ${f.path}`}
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
                                        ? `${f.oldPath} → ${f.path}`
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

        <!-- Side by side the line is vertical; stacked (a narrow panel) it is horizontal. -->
        <Splitter
            box={layout}
            axis="x"
            bind:value={infoW}
            current={layoutW ? infoPxW / layoutW : 0.4}
            min={0.15}
            max={0.9}
            minPx={INFO_DRAG_MIN}
            endMinPx={VIEW_MIN}
            label="Resize the commit details"
            storageKey="commit.info"
        />
        <Splitter
            box={layout}
            axis="y"
            bind:value={infoH}
            current={layoutH ? infoPxH / layoutH : 0.4}
            min={0.1}
            max={0.8}
            minPx={128}
            endMinPx={160}
            label="Resize the commit details"
            storageKey="commit.info.stacked"
        />

        <div class="view">
            {#if file && detail}
                <div class="tabs" role="tablist">
                    <button
                        title="Show what this commit changed in the file"
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
                        title="List every commit that changed this file"
                        type="button"
                        onclick={() => file && onhistory(file.path)}
                    >
                        File history
                    </button>
                </div>
                {#if tab === "blame" && file.status !== "deleted"}
                    <BlameView
                        {projectId}
                        {side}
                        commit={detail.id}
                        path={file.path}
                        {onjump}
                    />
                {:else}
                    <DiffView
                        {projectId}
                        {side}
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
    .close {
        position: absolute;
        z-index: 2;
        top: 0.3rem;
        right: 0.6rem;
        padding: 0.05rem 0.4rem;
        border-color: transparent;
        background: var(--surface);
        color: var(--text-muted);
        font-size: 0.85rem;
    }
    .close:hover {
        color: var(--text);
    }
    .panel {
        position: relative;
        container-type: inline-size;
        height: 100%;
        min-height: 0;
    }
    .layout {
        display: grid;
        grid-template-columns: var(--info) 6px minmax(0, 1fr);
        grid-template-areas: "info line view";
        height: 100%;
        min-height: 0;
    }
    .info {
        grid-area: info;
        min-height: 0;
        min-width: 0;
        padding: 0.6rem 0.75rem;
        overflow: auto;
    }
    .layout > :global(.splitter.x) {
        grid-area: line;
    }
    .layout > :global(.splitter.y) {
        display: none;
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
        grid-area: view;
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
            grid-template-columns: minmax(0, 1fr);
            grid-template-rows: auto 6px minmax(0, 1fr);
            grid-template-areas: "info" "line" "view";
        }
        .info {
            max-height: 40%;
        }
        .info.sized {
            max-height: 80%;
        }
        .layout > :global(.splitter.x) {
            display: none;
        }
        .layout > :global(.splitter.y) {
            display: block;
            grid-area: line;
        }
    }
</style>
