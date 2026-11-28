<script lang="ts">
    // One project tab. It stays mounted while other tabs show, so its own state (scroll, loaded
    // data, later the graph) survives switching tabs; it never reads another tab's state.
    import { limits, lockSelection } from "../paneSize";
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { RefLabel } from "../bindings/RefLabel";
    import type { ProjectSummary } from "../bindings/ProjectSummary";
    import type { ProjectView } from "../bindings/ProjectView";
    import type { RepoSide } from "../bindings/RepoSide";
    import type { RepoState } from "../bindings/RepoState";
    import { operationLabel } from "../format";
    import { VIEWS } from "../tabs";
    import ActivityView from "./ActivityView.svelte";
    import ReleaseView from "./ReleaseView.svelte";
    import ChangesView from "./ChangesView.svelte";
    import StagingView from "./StagingView.svelte";
    import CommitPanel from "./CommitPanel.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import FilesView from "./FilesView.svelte";
    import HistoryView from "./HistoryView.svelte";

    let {
        id,
        summary,
        view,
        side,
        active,
        changed,
        onview,
        onside,
    }: {
        id: string;
        summary: ProjectSummary | undefined;
        view: ProjectView;
        /** Which repository of a two-repo project is showing (always "private" otherwise). */
        side: RepoSide;
        active: boolean;
        /** Bumped when the project's files change on disk. */
        changed: number;
        onview: (v: ProjectView) => void;
        onside: (s: RepoSide) => void;
    } = $props();

    // A two-repo project has a separate public repo to switch to; single-repo has only one side.
    const twoRepo = $derived(summary?.mode === "two-repo");
    // The side actually in effect: "public" only makes sense for a two-repo project.
    const repoSide = $derived<RepoSide>(twoRepo ? side : "private");

    let repo = $state<RepoState | null>(null);
    let error = $state<AppError | null>(null);
    let loadedAt = -1;
    let loadedSide: RepoSide | null = null;

    async function reload() {
        await load(changed);
    }

    async function load(tick: number) {
        loadedAt = tick;
        loadedSide = repoSide;
        const want = repoSide;
        try {
            const next = await api.projectState(id, want);
            // A side switch mid-flight would otherwise show the wrong repo's state.
            if (repoSide === want) {
                repo = next;
                error = null;
            }
        } catch (e) {
            if (repoSide === want) error = e as AppError;
        }
    }

    // Load when first shown, again on changes, and whenever the side switches, but only while
    // this tab is showing.
    $effect(() => {
        if (active && (loadedAt !== changed || loadedSide !== repoSide))
            void load(changed);
    });

    // The graph and the log share one history view (and its selection); it is created the first
    // time either is opened and then kept, hidden, so switching views keeps the scroll position.
    const isHistory = $derived(view === "graph" || view === "log");
    let historyOpened = $state(false);
    $effect(() => {
        if (isHistory) historyOpened = true;
    });
    let selected = $state<string | null>(null);
    let selectedRefs = $state<RefLabel[]>([]);
    let historyPath = $state("");
    /** The file the Files tab should open, set by "Open in Files" elsewhere. */
    let filePath = $state("");
    let diffMode = $state<"unified" | "split">("unified");
    // The commit draft outlives switching views.
    let draft = $state("");
    let amend = $state(false);
    /** Share of the height given to the commit list when a commit is open. */
    let split = $state(0.45);
    let splitBox = $state<HTMLElement>();
    // The list and the details each keep room for their own controls, however the line is dragged.
    const LIST_MIN = 224;
    const PANEL_MIN = 256;
    const range = (h: number) => limits(h, 0.15, 0.85, LIST_MIN, PANEL_MIN);

    function dragSplit(e: PointerEvent) {
        const box = splitBox;
        if (!box) return;
        const handle = e.currentTarget as HTMLElement;
        handle.setPointerCapture(e.pointerId);
        e.preventDefault();
        const unlock = lockSelection();
        const move = (ev: PointerEvent) => {
            const r = box.getBoundingClientRect();
            const [lo, hi] = range(r.height);
            split = Math.min(hi, Math.max(lo, (ev.clientY - r.top) / r.height));
        };
        const up = () => {
            unlock();
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
            handle.removeEventListener("pointercancel", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
    }

    function splitKey(e: KeyboardEvent) {
        const [lo, hi] = range(splitBox?.clientHeight ?? 0);
        if (e.key === "ArrowUp") split = Math.max(lo, split - 0.05);
        else if (e.key === "ArrowDown") split = Math.min(hi, split + 0.05);
        else return;
        e.preventDefault();
    }

    const head = $derived(repo?.branches.find((b) => b.isHead));
    const placeholders: Partial<Record<ProjectView, string>> = {
        settings:
            "Project settings (editing the .proj file) arrive in a later phase.",
    };
</script>

<!-- Esc closes the commit details wherever focus is, unless something else used the key: a text
     field, a dialog, the context menu or an open name form. -->
<svelte:window
    onkeydown={(e) => {
        if (e.key !== "Escape" || e.defaultPrevented) return;
        if (!active || !isHistory || !selected) return;
        const t = e.target as HTMLElement | null;
        if (t?.closest("input, textarea, select, dialog")) return;
        selected = null;
    }}
/>

<div class="project">
    <nav class="side" aria-label="Project views">
        <div class="title" title={summary?.path}>{summary?.name ?? id}</div>
        {#if twoRepo}
            <div
                class="sides"
                role="group"
                aria-label="Repository side"
                title="Switch between this project's private working repo and its public release repo"
            >
                <button
                    title="Show this project's private working repo"
                    type="button"
                    class:current={repoSide === "private"}
                    aria-pressed={repoSide === "private"}
                    onclick={() => onside("private")}
                >
                    Private
                </button>
                <button
                    title="Show this project's public release repo"
                    type="button"
                    class:current={repoSide === "public"}
                    aria-pressed={repoSide === "public"}
                    onclick={() => onside("public")}
                >
                    Public
                </button>
            </div>
        {/if}
        {#each VIEWS as v (v.id)}
            <button
                title={v.hint}
                type="button"
                class:current={view === v.id}
                aria-current={view === v.id ? "page" : undefined}
                onclick={() => onview(v.id)}
            >
                {v.label}
            </button>
        {/each}
    </nav>

    <div class="content">
        {#if twoRepo && repoSide === "public"}
            <p class="banner">
                Public release repo — changes here are normally made from
                Staging.
            </p>
        {/if}
        {#if historyOpened}
            <section
                class="main flush"
                hidden={!isHistory}
                bind:this={splitBox}
                style:grid-template-rows={selected
                    ? `minmax(${LIST_MIN}px, ${split * 100}%) 6px minmax(${PANEL_MIN}px, 1fr)`
                    : "minmax(0, 1fr)"}
            >
                <HistoryView
                    {id}
                    side={repoSide}
                    active={active && isHistory}
                    {changed}
                    showGraph={view === "graph"}
                    head={repo?.head ?? null}
                    bind:selected
                    bind:selectedRefs
                    bind:path={historyPath}
                />
                {#if selected}
                    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (a focusable separator is the ARIA splitter pattern) -->
                    <div
                        class="splitter"
                        role="separator"
                        title="Drag to resize, or use the up and down arrow keys"
                        aria-orientation="horizontal"
                        aria-label="Resize the commit details"
                        aria-valuenow={Math.round(split * 100)}
                        aria-valuemin={15}
                        aria-valuemax={85}
                        tabindex="0"
                        onpointerdown={dragSplit}
                        onkeydown={splitKey}
                    ></div>
                    <CommitPanel
                        projectId={id}
                        side={repoSide}
                        commit={selected}
                        refs={selectedRefs}
                        head={repo?.head ?? null}
                        bind:diffMode
                        onjump={(c) => (selected = c)}
                        onclose={() => (selected = null)}
                        onhistory={(p) => {
                            historyPath = p;
                            onview("log");
                        }}
                    />
                {/if}
            </section>
        {/if}
        {#if view === "changes"}
            <section class="main flush changes">
                {#if error}
                    <div class="pad"><ErrorBox {error} /></div>
                {:else if repo}
                    <ChangesView
                        projectId={id}
                        side={repoSide}
                        {repo}
                        {changed}
                        bind:message={draft}
                        bind:amend
                        bind:diffMode
                        onreload={reload}
                        onopenfile={(p) => {
                            filePath = p;
                            onview("files");
                        }}
                    />
                {:else}
                    <p class="muted pad">Loading…</p>
                {/if}
            </section>
        {/if}
        {#if view === "files"}
            <section class="main flush changes">
                <FilesView
                    projectId={id}
                    side={repoSide}
                    {changed}
                    bind:path={filePath}
                />
            </section>
        {/if}
        <section
            class="main"
            hidden={isHistory || view === "changes" || view === "files"}
        >
            {#if view === "activity"}
                <ActivityView
                    projectId={id}
                    side={repoSide}
                    root={repo?.root ?? summary?.path}
                    {active}
                    onreload={reload}
                />
            {:else if view === "staging"}
                <StagingView projectId={id} {changed} {active} />
            {:else if view === "overview"}
                <h2>Overview</h2>
                {#if error}
                    <ErrorBox {error} />
                {:else if repo}
                    {#if !repo.trusted}
                        <p class="warn">
                            This repository is owned by another user. Project
                            Manager will ask before writing to it.
                        </p>
                    {/if}
                    {#if repo.inProgress}
                        <p class="warn">
                            {operationLabel(repo.inProgress)} in progress.
                        </p>
                    {/if}
                    {#if repo.hasSubmodules}
                        <p class="warn">
                            This repository uses submodules; Project Manager
                            does not manage them.
                        </p>
                    {/if}
                    <dl>
                        <dt>Folder</dt>
                        <dd>
                            <code>{repo.root}</code>
                            <button
                                type="button"
                                class="folder"
                                title="Show this project's folder in your file manager"
                                onclick={() =>
                                    api
                                        .openFolder(id, null, repoSide)
                                        .catch((e) => (error = e as AppError))}
                            >
                                Open folder
                            </button>
                        </dd>
                        <dt>Branch</dt>
                        <dd>
                            {#if repo.head.detached}
                                detached at <code>
                                    {repo.head.commit?.slice(0, 10)}
                                </code>
                            {:else}
                                <code>{repo.head.branch}</code>
                                {#if !repo.head.commit}(no commits yet){/if}
                            {/if}
                        </dd>
                        <dt>Upstream</dt>
                        <dd>
                            {#if head?.upstream}
                                <code>{head.upstream}</code>
                                {#if head.ahead !== null}· {head.ahead} to push, {head.behind}
                                    to pull{/if}
                            {:else}
                                none
                            {/if}
                        </dd>
                        {#if repo.status}
                            <dt>Changes</dt>
                            <dd>
                                {repo.status.staged.length} staged, {repo.status
                                    .unstaged.length} modified,
                                {repo.status.untracked.length} new{#if repo.status.conflicted.length},
                                    <span class="bad">
                                        {repo.status.conflicted.length} conflicted
                                    </span>{/if}
                            </dd>
                        {/if}
                        <dt>Branches</dt>
                        <dd>
                            {repo.branches.length} local, {repo.remoteBranches
                                .length} remote
                        </dd>
                        <dt>Tags</dt>
                        <dd>{repo.tags.length}</dd>
                        <dt>Remotes</dt>
                        <dd>
                            {#each repo.remotes as r (r.name)}
                                <div>
                                    <code>{r.name}</code>
                                    {r.fetchUrl ?? ""}
                                </div>
                            {:else}
                                none
                            {/each}
                        </dd>
                    </dl>
                {:else}
                    <p class="muted">Loading…</p>
                {/if}
            {:else if view === "releases"}
                <ReleaseView projectId={id} {changed} {active} />
            {:else}
                <h2>{VIEWS.find((v) => v.id === view)?.label}</h2>
                <p class="muted">{placeholders[view]}</p>
            {/if}
        </section>
    </div>
</div>

<style>
    .project {
        display: grid;
        grid-template-columns: 10rem minmax(0, 1fr);
        height: 100%;
        min-height: 0;
    }
    .side {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 0.75rem 0.5rem;
        overflow-y: auto;
        border-right: 1px solid var(--border);
        background: var(--surface);
    }
    .title {
        overflow: hidden;
        margin: 0 0.5rem 0.6rem;
        font-weight: 600;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .sides {
        display: flex;
        gap: 2px;
        margin: 0 0.25rem 0.6rem;
        padding: 2px;
        border-radius: 6px;
        background: var(--surface-2);
    }
    .side .sides button {
        flex: 1;
        padding: 0.25rem 0.4rem;
        border-radius: 4px;
        color: var(--text-muted);
        text-align: center;
    }
    .side .sides button.current {
        box-shadow: none;
        background: var(--bg);
        color: var(--text);
    }
    .content {
        display: flex;
        flex-direction: column;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }
    .content > .main {
        flex: 1 1 auto;
    }
    .banner {
        flex: 0 0 auto;
        margin: 0;
        padding: 0.4rem 1.5rem;
        border-bottom: 1px solid var(--border);
        background: var(--surface);
        color: var(--text-muted);
        font-size: 0.85rem;
    }
    .side button {
        padding: 0.4rem 0.6rem;
        border: none;
        border-radius: 4px;
        background: none;
        color: var(--text-muted);
        font: inherit;
        text-align: left;
        cursor: pointer;
    }
    .side button:hover {
        background: var(--surface-2);
        color: var(--text);
    }
    .side button.current {
        background: var(--surface-2);
        color: var(--text);
        box-shadow: inset 3px 0 var(--accent);
    }
    .main {
        min-width: 0;
        padding: 1.25rem 1.5rem;
        overflow: auto;
    }
    .main.flush {
        display: grid;
        min-height: 0;
        padding: 0;
        overflow: hidden;
    }
    .main.changes {
        container-type: inline-size;
    }
    .pad {
        padding: 1rem;
    }
    /* After .flush, which would otherwise win and keep the hidden history view on screen. */
    .main[hidden] {
        display: none;
    }
    .splitter {
        border-top: 1px solid var(--border);
        background: var(--surface);
        cursor: row-resize;
    }
    .splitter:hover,
    .splitter:focus-visible {
        background: var(--accent);
    }
    h2 {
        margin: 0 0 1rem;
        font-size: 1.2rem;
    }
    dl {
        display: grid;
        grid-template-columns: max-content 1fr;
        gap: 0.4rem 1.25rem;
        margin: 0;
    }
    dt {
        color: var(--text-muted);
    }
    dd {
        margin: 0;
        word-break: break-word;
    }
    .folder {
        margin-left: 0.5rem;
        padding: 0.1rem 0.55rem;
        font-size: 0.85rem;
    }
    .muted {
        color: var(--text-muted);
    }
    .warn {
        color: var(--warning);
    }
    .bad {
        color: var(--danger);
    }
    @media (max-width: 40rem) {
        .project {
            grid-template-columns: 1fr;
            grid-template-rows: auto 1fr;
        }
        .side {
            flex-direction: row;
            overflow-x: auto;
            border-right: none;
            border-bottom: 1px solid var(--border);
        }
        .title {
            display: none;
        }
    }
</style>
