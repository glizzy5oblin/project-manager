<script lang="ts">
    // One project tab. It stays mounted while other tabs show, so its own state (scroll, loaded
    // data, later the graph) survives switching tabs; it never reads another tab's state.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { RefLabel } from "../bindings/RefLabel";
    import type { ProjectSummary } from "../bindings/ProjectSummary";
    import type { ProjectView } from "../bindings/ProjectView";
    import type { RepoState } from "../bindings/RepoState";
    import { operationLabel } from "../format";
    import { VIEWS } from "../tabs";
    import CommitPanel from "./CommitPanel.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import HistoryView from "./HistoryView.svelte";

    let {
        id,
        summary,
        view,
        active,
        changed,
        onview,
    }: {
        id: string;
        summary: ProjectSummary | undefined;
        view: ProjectView;
        active: boolean;
        /** Bumped when the project's files change on disk. */
        changed: number;
        onview: (v: ProjectView) => void;
    } = $props();

    let repo = $state<RepoState | null>(null);
    let error = $state<AppError | null>(null);
    let loadedAt = -1;

    async function load(tick: number) {
        loadedAt = tick;
        try {
            repo = await api.projectState(id);
            error = null;
        } catch (e) {
            error = e as AppError;
        }
    }

    // Load when first shown, and again on changes, but only while this tab is showing.
    $effect(() => {
        if (active && loadedAt !== changed) void load(changed);
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
    let diffMode = $state<"unified" | "split">("unified");
    /** Share of the height given to the commit list when a commit is open. */
    let split = $state(0.45);
    let splitBox = $state<HTMLElement>();

    function dragSplit(e: PointerEvent) {
        const box = splitBox;
        if (!box) return;
        const handle = e.currentTarget as HTMLElement;
        handle.setPointerCapture(e.pointerId);
        const move = (ev: PointerEvent) => {
            const r = box.getBoundingClientRect();
            split = Math.min(
                0.85,
                Math.max(0.15, (ev.clientY - r.top) / r.height),
            );
        };
        const up = () => {
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
    }

    function splitKey(e: KeyboardEvent) {
        if (e.key === "ArrowUp") split = Math.max(0.15, split - 0.05);
        else if (e.key === "ArrowDown") split = Math.min(0.85, split + 0.05);
        else return;
        e.preventDefault();
    }

    const head = $derived(repo?.branches.find((b) => b.isHead));
    const placeholders: Partial<Record<ProjectView, string>> = {
        staging: "Private to public staging arrives in Phase 4.",
        releases: "The release suite arrives in Phase 5.",
        settings:
            "Project settings (editing the .proj file) arrive in a later phase.",
    };
</script>

<div class="project">
    <nav class="side" aria-label="Project views">
        <div class="title" title={summary?.path}>{summary?.name ?? id}</div>
        {#each VIEWS as v (v.id)}
            <button
                type="button"
                class:current={view === v.id}
                aria-current={view === v.id ? "page" : undefined}
                onclick={() => onview(v.id)}
            >
                {v.label}
            </button>
        {/each}
    </nav>

    {#if historyOpened}
        <section
            class="main flush"
            hidden={!isHistory}
            bind:this={splitBox}
            style:grid-template-rows={selected
                ? `minmax(0, ${split}fr) 6px minmax(0, ${1 - split}fr)`
                : "minmax(0, 1fr)"}
        >
            <HistoryView
                {id}
                active={active && isHistory}
                {changed}
                showGraph={view === "graph"}
                bind:selected
                bind:selectedRefs
                bind:path={historyPath}
            />
            {#if selected}
                <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (a focusable separator is the ARIA splitter pattern) -->
                <div
                    class="splitter"
                    role="separator"
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
                    commit={selected}
                    refs={selectedRefs}
                    bind:diffMode
                    onjump={(c) => (selected = c)}
                    onhistory={(p) => {
                        historyPath = p;
                        onview("log");
                    }}
                />
            {/if}
        </section>
    {/if}
    <section class="main" hidden={isHistory}>
        {#if view === "overview"}
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
                        This repository uses submodules; Project Manager does
                        not manage them.
                    </p>
                {/if}
                <dl>
                    <dt>Folder</dt>
                    <dd><code>{repo.root}</code></dd>
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
                            <div><code>{r.name}</code> {r.fetchUrl ?? ""}</div>
                        {:else}
                            none
                        {/each}
                    </dd>
                </dl>
            {:else}
                <p class="muted">Loading…</p>
            {/if}
        {:else}
            <h2>{VIEWS.find((v) => v.id === view)?.label}</h2>
            <p class="muted">{placeholders[view]}</p>
        {/if}
    </section>
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
