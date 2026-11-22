<script lang="ts">
    // The commit graph and log: one virtualised list (only visible rows exist in the DOM, rows
    // load in chunks), with the graph drawn on a canvas beside it in graph mode. In log mode a
    // search filters the list; in graph mode it fades the commits that do not match.
    import { untrack } from "svelte";
    import { actions } from "../actions.svelte";
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { GraphInfo } from "../bindings/GraphInfo";
    import type { GraphOptions } from "../bindings/GraphOptions";
    import type { GraphRow } from "../bindings/GraphRow";
    import type { HeadState } from "../bindings/HeadState";
    import type { LogQuery } from "../bindings/LogQuery";
    import type { RefLabel } from "../bindings/RefLabel";
    import { commitCommands, commitMenu } from "../commitMenu.svelte";
    import { offerMenu } from "../contextMenu";
    import { graphShapes } from "../graphShapes";
    import {
        RowCache,
        dayEnd,
        dayStart,
        formatTime,
        graphColorVar,
        graphWidth,
        lowerBound,
        scrollToShow,
        shortId,
        visibleRange,
    } from "../history";
    import ColumnHandle from "./ColumnHandle.svelte";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        id,
        active,
        changed,
        showGraph,
        head = null,
        selected = $bindable(null),
        selectedRefs = $bindable([]),
        path = $bindable(""),
    }: {
        id: string;
        active: boolean;
        /** Bumped when the project's files change on disk. */
        changed: number;
        showGraph: boolean;
        /** Where HEAD is, for the git actions in the right-click menu. */
        head?: HeadState | null;
        /** Selected commit id, shared with the detail panel. */
        selected?: string | null;
        selectedRefs?: RefLabel[];
        /** Path filter (file history). */
        path?: string;
    } = $props();

    const uid = $props.id();

    let info = $state<GraphInfo | null>(null);
    let error = $state<AppError | null>(null);
    let options = $state<GraphOptions>({ refs: null, highlight: null });
    let cache: RowCache | null = null;
    /** Bumped when rows arrive, the theme changes or the pixel ratio changes: redraw. */
    let version = $state(0);
    let loadedFor = -1;

    let scroller = $state<HTMLDivElement>();
    let scrollTop = $state(0);
    let viewH = $state(0);
    let rowHeight = $state(26);
    let laneWidth = $state(14);

    let selectedRow = $state<number | null>(null);
    /** Commit id at `selectedRow`, to tell our own selections from outside ones. */
    let shownId: string | null = null;

    let qMessage = $state("");
    let qAuthor = $state("");
    let qSince = $state("");
    let qUntil = $state("");
    let qBranch = $state("");
    /** The date and branch filters sit in a second row, open while asked for or in use. */
    let filtersOpen = $state(false);
    const extraFilters = $derived(
        [qSince, qUntil, qBranch].filter((v) => v).length,
    );
    const showFilters = $derived(filtersOpen || extraFilters > 0);
    let searchRows = $state<number[] | null>(null);
    let searching = $state(false);
    let searchError = $state<AppError | null>(null);
    let searchRequest = 0;
    let gotoText = $state("");
    let gotoMissing = $state(false);

    async function loadGraph() {
        try {
            const next = await api.graphLoad(id, $state.snapshot(options));
            error = null;
            if (info && next.generation === info.generation) return;
            cache?.close();
            const gen = next.generation;
            cache = new RowCache(
                (start, count) => api.graphRows(id, gen, start, count),
                () => version++,
                (e) => {
                    if ((e as AppError).kind === "stale") void loadGraph();
                    else error = e as AppError;
                },
            );
            info = next;
            version++;
            selectedRow = null;
            shownId = null;
            if (selected) void locate(selected);
        } catch (e) {
            error = e as AppError;
        }
    }

    // Load when first shown and after changes on disk, only while the tab is showing.
    $effect(() => {
        if (active && loadedFor !== changed) {
            loadedFor = changed;
            untrack(() => void loadGraph());
        }
    });

    function setOptions(o: Partial<GraphOptions>) {
        options = { ...options, ...o };
        void loadGraph();
    }

    // ---- search ----

    const query = $derived<LogQuery>({
        message: qMessage.trim() || null,
        author: qAuthor.trim() || null,
        path: path.trim() || null,
        since: qSince ? dayStart(qSince) : null,
        until: qUntil ? dayEnd(qUntil) : null,
        branch: qBranch || null,
    });
    const queryActive = $derived(Object.values(query).some((v) => v !== null));

    $effect(() => {
        const q = query;
        const gen = info?.generation;
        if (gen === undefined) return;
        const mine = ++searchRequest;
        if (!queryActive) {
            searchRows = null;
            searchError = null;
            searching = false;
            untrack(() => void api.logSearchCancel(id).catch(() => {}));
            return;
        }
        const timer = setTimeout(async () => {
            searching = true;
            try {
                const r = await api.logSearch(id, gen, q);
                if (mine !== searchRequest) return;
                searchRows = r.rows;
                searchError = null;
            } catch (e) {
                if (mine !== searchRequest) return;
                const err = e as AppError;
                if (err.kind !== "cancelled" && err.kind !== "stale")
                    searchError = err;
            } finally {
                if (mine === searchRequest) searching = false;
            }
        }, 250);
        return () => clearTimeout(timer);
    });

    function clearSearch() {
        qMessage = qAuthor = qSince = qUntil = qBranch = "";
        path = "";
    }

    // ---- display ----

    const filtered = $derived(!showGraph && searchRows !== null);
    const count = $derived(
        info ? (filtered ? (searchRows?.length ?? 0) : info.rows) : 0,
    );
    const rowOf = (i: number) => (filtered && searchRows ? searchRows[i] : i);
    function displayIndexOf(row: number): number {
        if (!filtered || !searchRows) return row;
        const i = lowerBound(searchRows, row);
        return searchRows[i] === row ? i : -1;
    }
    const matches = $derived(
        showGraph && searchRows ? new Set(searchRows) : null,
    );
    // Wide enough for the "Graph" heading however few lanes there are.
    const GRAPH_MIN = 48;
    /** Width of the column header row, which is as wide as the list. */
    let listW = $state(0);
    /** The graph column's width when the user has dragged it; null follows the lane count. */
    let graphW = $state<number | null>(null);
    const gw = $derived(
        showGraph && info
            ? Math.max(
                  GRAPH_MIN,
                  graphW !== null
                      ? Math.min(graphW, Math.round(listW * 0.6))
                      : graphWidth(info.maxLanes, laneWidth),
              )
            : 0,
    );
    // Lanes squeeze together when there are more than the column has room for, so none are cut
    // off at the edge. Never thinner than 3px.
    const lw = $derived(
        info
            ? Math.min(
                  laneWidth,
                  Math.max(3, (gw - 12) / Math.max(1, info.maxLanes)),
              )
            : laneWidth,
    );
    const range = $derived(visibleRange(scrollTop, viewH, rowHeight, count, 8));
    const visible = $derived.by(() => {
        void version;
        const out: { i: number; row: GraphRow | undefined }[] = [];
        for (let i = range[0]; i <= range[1]; i++)
            out.push({ i, row: cache?.get(rowOf(i)) });
        return out;
    });

    // The graph is one SVG inside the scrolled list, so it scrolls with the rows as ordinary
    // content. (A canvas laid over the list and redrawn on scroll left stale strips of earlier
    // frames on screen in WebKitGTK.) It is placed at the first row it covers and only changes
    // when that range does, not on every scroll event.
    const graphFirst = $derived(range[0]);
    const graphLast = $derived(range[1]);
    const shapes = $derived.by(() => {
        void version;
        if (!showGraph || !info) return null;
        return graphShapes({
            first: graphFirst,
            last: graphLast,
            rowHeight,
            laneWidth: lw,
            rowAt: (i) => cache?.get(i),
            selectedRow,
            headRow: info.headRow,
            matches,
        });
    });

    // ---- columns ----

    // Widths in pixels of the Author, Date and Commit columns, which sit against the right edge
    // (the description takes the rest). Null is the default width (see the CSS).
    let authorW = $state<number | null>(null);
    let dateW = $state<number | null>(null);
    let shaW = $state<number | null>(null);
    /** No column takes more than this, so the description keeps room. */
    const maxCol = $derived(Math.max(120, Math.round(listW * 0.4)));
    const cols = $derived(
        [
            authorW !== null && `--c-author:${Math.min(authorW, maxCol)}px`,
            dateW !== null && `--c-date:${Math.min(dateW, maxCol)}px`,
            shaW !== null && `--c-sha:${Math.min(shaW, maxCol)}px`,
        ]
            .filter(Boolean)
            .join(";"),
    );

    // ---- right-click menu ----

    /** Select the commit and offer its git actions (the panel beside the list runs them). */
    function rowMenu(e: MouseEvent, r: GraphRow) {
        selectRow(r);
        const menu = commitMenu({
            commit: r.id,
            subject: r.subject,
            refs: r.refs,
            head,
            busy: !!actions.busy[id],
        });
        offerMenu(e, {
            items: menu.items,
            run: (item) => {
                const command = menu.commands[item];
                if (command.type === "copy") {
                    void navigator.clipboard.writeText(command.text);
                } else {
                    commitCommands.request = { commit: r.id, command };
                }
            },
        });
    }

    /** The menu key or Shift+F10 on the list itself: the menu of the selected commit. */
    function listMenu(e: MouseEvent) {
        if (e.target !== e.currentTarget || selectedRow === null) return;
        const r = cache?.get(selectedRow);
        if (r) rowMenu(e, r);
    }

    // ---- selection ----

    function selectRow(r: GraphRow) {
        selectedRow = r.row;
        shownId = r.id;
        selected = r.id;
        selectedRefs = r.refs;
    }

    function ensureVisible(i: number) {
        if (i < 0 || !scroller) return;
        scroller.scrollTop = scrollToShow(
            i,
            scroller.scrollTop,
            viewH,
            rowHeight,
        );
    }

    async function selectDisplay(i: number) {
        if (!cache || count === 0) return;
        const di = Math.max(0, Math.min(count - 1, i));
        const r = await cache.row(rowOf(di));
        if (!r) return;
        selectRow(r);
        ensureVisible(di);
    }

    /** Select a commit chosen elsewhere (a parent link, blame, "go to"). */
    async function locate(commit: string): Promise<boolean> {
        if (!info || !cache) return false;
        const gen = info.generation;
        const c = cache;
        const row = await api.graphFind(id, gen, commit).catch(() => null);
        if (info?.generation !== gen) return false;
        if (row === null) {
            selectedRow = null;
            shownId = commit;
            selectedRefs = [];
            return false;
        }
        const r = await c.row(row);
        if (!r) return false;
        selectRow(r);
        // Centre it when it is off screen.
        const di = displayIndexOf(row);
        if (di >= 0 && scroller) {
            const top = di * rowHeight;
            if (
                top < scroller.scrollTop ||
                top + rowHeight > scroller.scrollTop + viewH
            ) {
                scroller.scrollTop = Math.max(0, top - viewH / 2);
            }
        }
        return true;
    }

    $effect(() => {
        const s = selected;
        if (s && s !== shownId) untrack(() => void locate(s));
    });

    function onkey(e: KeyboardEvent) {
        if (!info || count === 0) return;
        const cur = selectedRow === null ? -1 : displayIndexOf(selectedRow);
        const page = Math.max(1, Math.floor(viewH / rowHeight) - 1);
        let next: number;
        switch (e.key) {
            case "ArrowDown":
            case "j":
                next = cur + 1;
                break;
            case "ArrowUp":
            case "k":
                next = cur < 0 ? 0 : cur - 1;
                break;
            case "PageDown":
                next = cur + page;
                break;
            case "PageUp":
                next = cur - page;
                break;
            case "Home":
                next = 0;
                break;
            case "End":
                next = count - 1;
                break;
            default:
                return;
        }
        e.preventDefault();
        void selectDisplay(next);
    }

    function nextMatch(step: 1 | -1) {
        if (!searchRows || searchRows.length === 0) return;
        const cur = selectedRow ?? -1;
        let k =
            step === 1
                ? lowerBound(searchRows, cur + 1)
                : lowerBound(searchRows, cur) - 1;
        if (k >= searchRows.length) k = 0;
        if (k < 0) k = searchRows.length - 1;
        void selectDisplay(displayIndexOf(searchRows[k]));
    }

    async function goto() {
        const t = gotoText.trim();
        if (!t) return;
        gotoMissing = !(await locate(t));
    }

    async function gotoHead() {
        const h = info?.headRow;
        if (h === null || h === undefined || !cache) return;
        const r = await cache.row(h);
        if (r) void locate(r.id);
    }

    /** "Show" choice for the `.proj` file's private and public branches (not a valid ref name). */
    const WORKFLOW = "@workflow";
    const showValue = $derived(
        (options.refs?.length ?? 0) > 1 ? WORKFLOW : (options.refs?.[0] ?? ""),
    );
    function setShow(v: string) {
        const refs =
            v === WORKFLOW ? (info?.workflowRefs ?? null) : v ? [v] : null;
        setOptions({ refs: refs?.length ? refs : null });
    }

    const refChoices = $derived(
        info?.refs.filter((r) => r.full !== "HEAD") ?? [],
    );
    const zooms = [
        { h: 22, label: "Compact" },
        { h: 26, label: "Normal" },
        { h: 32, label: "Large" },
    ];
</script>

<div class="history-view">
    <div class="toolbar">
        <div class="group">
            <div class="side">
                {#if showGraph}
                    <label>
                        Show
                        <select
                            title="Choose which branches are drawn in the graph"
                            value={showValue}
                            onchange={(e) => setShow(e.currentTarget.value)}
                        >
                            <option value="">All branches</option>
                            {#if info?.workflowRefs.length}
                                <option value={WORKFLOW}>
                                    Private and public branches
                                </option>
                            {/if}
                            {#each refChoices as r (r.full)}
                                <option value={r.full}>{r.name}</option>
                            {/each}
                        </select>
                    </label>
                    <label>
                        Highlight
                        <select
                            title="Colour the commits of one branch so they stand out"
                            value={options.highlight ?? ""}
                            onchange={(e) =>
                                setOptions({
                                    highlight: e.currentTarget.value || null,
                                })}
                        >
                            <option value="">Nothing</option>
                            {#each info?.refs ?? [] as r (r.full)}
                                <option value={r.full}>{r.name}</option>
                            {/each}
                        </select>
                    </label>
                    <label class="lane">
                        Lanes
                        <input
                            title="Make the graph lines wider or narrower"
                            class="lanes"
                            type="range"
                            min="8"
                            max="28"
                            step="2"
                            aria-label="Lane width"
                            bind:value={laneWidth}
                        />
                    </label>
                {/if}
                <select
                    title="How tall each commit row is"
                    aria-label="Row size"
                    bind:value={rowHeight}
                >
                    {#each zooms as z (z.h)}
                        <option value={z.h}>{z.label}</option>
                    {/each}
                </select>
                <form
                    class="goto"
                    onsubmit={(e) => {
                        e.preventDefault();
                        void goto();
                    }}
                >
                    <input
                        type="search"
                        placeholder="Commit id"
                        title="Go to a commit by its id"
                        aria-label="Go to commit id"
                        aria-invalid={gotoMissing}
                        bind:value={gotoText}
                        oninput={() => (gotoMissing = false)}
                    />
                </form>
                {#if info?.headRow !== null && info?.headRow !== undefined}
                    <button
                        type="button"
                        onclick={gotoHead}
                        title="Select the checked-out commit"
                    >
                        HEAD
                    </button>
                {/if}
            </div>
            <div class="side">
                <input
                    title="Only show commits whose message contains this text"
                    type="search"
                    placeholder="Message"
                    aria-label="Search messages"
                    bind:value={qMessage}
                />
                <input
                    title="Only show commits by this author (name or email)"
                    type="search"
                    placeholder="Author"
                    aria-label="Search authors"
                    bind:value={qAuthor}
                />
                <input
                    type="search"
                    placeholder="Path"
                    title="Only commits that change this file or folder"
                    aria-label="Only commits that change this path"
                    bind:value={path}
                />
                <button
                    type="button"
                    class:on={showFilters}
                    aria-expanded={showFilters}
                    disabled={extraFilters > 0}
                    title={extraFilters > 0
                        ? "Clear the date and branch filters to hide them"
                        : "Filter by date or branch"}
                    onclick={() => (filtersOpen = !filtersOpen)}
                >
                    Filters{extraFilters > 0 ? ` (${extraFilters})` : ""}
                </button>
                {#if queryActive}
                    <button
                        title="Clear the search and show every commit again"
                        type="button"
                        onclick={clearSearch}
                    >
                        Clear
                    </button>
                {/if}
                <span class="status" aria-live="polite">
                    {#if searching}
                        Searching…
                    {:else if searchRows}
                        {searchRows.length} match{searchRows.length === 1
                            ? ""
                            : "es"}
                    {/if}
                </span>
                {#if showGraph && searchRows && searchRows.length > 0}
                    <button
                        title="Jump to the previous match"
                        type="button"
                        onclick={() => nextMatch(-1)}
                        aria-label="Previous match"
                    >
                        ↑
                    </button>
                    <button
                        title="Jump to the next match"
                        type="button"
                        onclick={() => nextMatch(1)}
                        aria-label="Next match"
                    >
                        ↓
                    </button>
                {/if}
            </div>
        </div>
        {#if showFilters}
            <div class="group filters">
                <label>
                    From <input
                        title="Only show commits made on or after this date"
                        type="date"
                        aria-label="From date"
                        bind:value={qSince}
                    />
                </label>
                <label>
                    to <input
                        title="Only show commits made on or before this date"
                        type="date"
                        aria-label="To date"
                        bind:value={qUntil}
                    />
                </label>
                <select
                    title="Only show commits on this branch"
                    aria-label="Only commits on this branch"
                    bind:value={qBranch}
                >
                    <option value="">Any branch</option>
                    {#each refChoices as r (r.full)}
                        <option value={r.full}>{r.name}</option>
                    {/each}
                </select>
            </div>
        {/if}
    </div>

    {#if error}
        <div class="pad"><ErrorBox {error} /></div>
    {/if}
    {#if searchError}
        <div class="pad"><ErrorBox error={searchError} /></div>
    {/if}

    <div
        class="columns"
        class:nograph={!showGraph}
        style:--gw="{gw}px"
        style={cols}
        bind:clientWidth={listW}
    >
        {#if showGraph}
            <span>
                Graph
                <ColumnHandle
                    bind:value={graphW}
                    edge="right"
                    min={GRAPH_MIN}
                    max={Math.max(GRAPH_MIN, Math.round(listW * 0.6))}
                    label="Resize the Graph column"
                    storageKey="history.graph"
                />
            </span>
        {/if}
        <span>Description</span>
        <span class="author">
            Author
            <ColumnHandle
                bind:value={authorW}
                min={60}
                max={maxCol}
                label="Resize the Author column"
                storageKey="history.author"
            />
        </span>
        <span>
            Date
            <ColumnHandle
                bind:value={dateW}
                min={70}
                max={maxCol}
                label="Resize the Date column"
                storageKey="history.date"
            />
        </span>
        <span>
            Commit
            <ColumnHandle
                bind:value={shaW}
                min={44}
                max={maxCol}
                label="Resize the Commit column"
                storageKey="history.sha"
            />
        </span>
    </div>
    <div class="viewport">
        <div
            class="list"
            class:nograph={!showGraph}
            bind:this={scroller}
            bind:clientHeight={viewH}
            onscroll={() => (scrollTop = scroller?.scrollTop ?? 0)}
            onkeydown={onkey}
            oncontextmenu={listMenu}
            tabindex="0"
            role="listbox"
            aria-label="Commits"
            aria-activedescendant={selectedRow !== null &&
            displayIndexOf(selectedRow) >= 0
                ? `${uid}-r${displayIndexOf(selectedRow)}`
                : undefined}
            style:--gw="{gw}px"
            style={cols}
            style:--rh="{rowHeight}px"
        >
            <div class="spacer" style:height="{count * rowHeight}px">
                {#each visible as v (v.i)}
                    <!-- svelte-ignore a11y_click_events_have_key_events (the list handles keys) -->
                    <div
                        title="Click to see this commit's details. Right-click for more actions"
                        id="{uid}-r{v.i}"
                        tabindex="-1"
                        class="row"
                        class:selected={v.row !== undefined &&
                            v.row.row === selectedRow}
                        class:faded={v.row?.dim ||
                            (matches !== null &&
                                v.row &&
                                !matches.has(v.row.row))}
                        role="option"
                        aria-selected={v.row !== undefined &&
                            v.row.row === selectedRow}
                        style:top="{v.i * rowHeight}px"
                        onclick={() => v.row && selectRow(v.row)}
                        oncontextmenu={(e) => v.row && rowMenu(e, v.row)}
                    >
                        {#if showGraph}<span></span>{/if}
                        {#if v.row}
                            <span class="subject">
                                {#each v.row.refs as r (r.full)}
                                    <span
                                        class="ref {r.kind}"
                                        class:head={r.isHead}
                                    >
                                        {r.name}
                                    </span>
                                {/each}
                                {#each v.row.branched as b (b.name)}
                                    <span
                                        class="fork"
                                        style:color={`var(${graphColorVar(b.color)})`}
                                        title={`${b.name} was made from this commit and nothing else has been committed here since, so both share one line. Its commits above are drawn in this colour.`}
                                    >
                                        ⑂ {b.name} branched here
                                    </span>
                                {/each}
                                {v.row.subject}
                            </span>
                            <span class="author" title={v.row.authorEmail}>
                                {v.row.authorName}
                            </span>
                            <span class="date">
                                {formatTime(v.row.authorTime)}
                            </span>
                            <span class="sha">{shortId(v.row.id)}</span>
                        {:else}
                            <span class="loading">…</span>
                        {/if}
                    </div>
                {/each}
                {#if shapes}
                    <svg
                        class="graph"
                        style:top="{graphFirst * rowHeight}px"
                        width={gw}
                        height={shapes.height}
                        aria-hidden="true"
                    >
                        <g
                            fill="none"
                            stroke-width={shapes.lineWidth}
                            stroke-linecap="round"
                        >
                            {#each shapes.edges as e, k (k)}
                                <path
                                    d={e.d}
                                    style:stroke={e.color}
                                    opacity={e.faded ? 0.3 : 1}
                                />
                            {/each}
                            {#each shapes.nodes as n, k (k)}
                                <g opacity={n.faded ? 0.3 : 1}>
                                    <circle
                                        cx={n.cx}
                                        cy={n.cy}
                                        r={n.r}
                                        style:stroke={n.hollow
                                            ? n.color
                                            : "none"}
                                        style:fill={n.hollow
                                            ? "var(--bg)"
                                            : n.color}
                                    />
                                    {#if n.ring}
                                        <circle
                                            cx={n.cx}
                                            cy={n.cy}
                                            r={n.r + shapes.ringGap}
                                            style:stroke={n.ring === "selected"
                                                ? "var(--text)"
                                                : n.color}
                                        />
                                    {/if}
                                </g>
                            {/each}
                        </g>
                    </svg>
                {/if}
            </div>
        </div>
        {#if info && info.rows === 0}
            <p class="empty">No commits yet.</p>
        {:else if info && count === 0 && searchRows}
            <p class="empty">No commits match.</p>
        {/if}
    </div>
</div>

<style>
    .history-view {
        container-type: inline-size;
        display: flex;
        flex-direction: column;
        min-height: 0;
        height: 100%;
    }
    /* One row: view controls, then the search fields and Filters. It holds everything on one line and the
       text fields give up width first, so the window's minimum width is what keeps it from wrapping. */
    .toolbar {
        display: flex;
        flex-direction: column;
        gap: 0.4rem;
        padding: 0.5rem 0.75rem;
        border-bottom: 1px solid var(--border);
        background: var(--surface);
    }
    .group {
        display: flex;
        flex-wrap: nowrap;
        gap: 0.3rem;
        align-items: center;
        min-width: 0;
    }
    /* The view controls lean left and the search controls lean right, with the free space between. */
    .group:not(.filters) {
        justify-content: space-between;
    }
    .side {
        min-width: 0;
        display: flex;
        flex-wrap: nowrap;
        gap: 0.3rem;
        align-items: center;
    }
    .toolbar select {
        min-width: 5.5rem;
        max-width: 7.5rem;
    }
    /* "All branches" has to show in full. */
    .toolbar label:first-child select {
        max-width: 8.75rem;
    }
    /* Text fields give up width before anything wraps to another line. */
    .toolbar input[type="search"] {
        flex: 1 1 6rem;
        min-width: 6rem;
        max-width: 9rem;
    }
    /* "Commit id" has to fit in full. */
    .toolbar .goto input[type="search"] {
        min-width: 7rem;
    }
    .lanes {
        width: 3.5rem;
        padding: 0;
        border: none;
        background: none;
    }
    .group.filters {
        flex-wrap: wrap;
    }
    .toolbar button.on {
        border-color: var(--accent);
        color: var(--accent);
    }
    .toolbar button.on:disabled {
        opacity: 1;
    }
    .toolbar label {
        display: flex;
        gap: 0.3rem;
        align-items: center;
        color: var(--text-muted);
    }
    .toolbar button {
        padding: 0.4rem 0.45rem;
    }
    /* Labels and their selects give up width down to the selects' minimum; buttons never do. */
    .toolbar label {
        flex-shrink: 1;
        min-width: 0;
    }
    /* The slider has a fixed width, so its label does not shrink around it. */
    .toolbar label.lane {
        flex-shrink: 0;
    }
    .toolbar label,
    .status {
        white-space: nowrap;
    }
    .status {
        flex-shrink: 0;
    }
    .status {
        color: var(--text-muted);
    }
    .goto {
        display: contents;
    }
    .pad {
        padding: 0.5rem 0.75rem 0;
    }
    .columns,
    .row {
        display: grid;
        grid-template-columns:
            var(--gw) minmax(0, 1fr) var(--c-author, 10rem)
            var(--c-date, 11rem) var(--c-sha, 5.5rem);
        gap: 0 0.6rem;
        align-items: center;
    }
    /* Log view: no graph cell, so no graph column and a normal left gutter. */
    .columns.nograph,
    .nograph .row {
        grid-template-columns:
            minmax(0, 1fr) var(--c-author, 10rem) var(--c-date, 11rem)
            var(--c-sha, 5.5rem);
        padding-left: 0.6rem;
    }
    /* Narrow: drop the author column (its name stays in the commit panel). */
    @container (max-width: 50rem) {
        .columns,
        .row {
            grid-template-columns:
                var(--gw) minmax(0, 1fr) var(--c-date, 11rem)
                var(--c-sha, 5.5rem);
        }
        .columns.nograph,
        .nograph .row {
            grid-template-columns: minmax(0, 1fr) var(--c-date, 11rem) var(
                    --c-sha,
                    5.5rem
                );
        }
        .author {
            display: none;
        }
    }
    .columns {
        padding: 0.25rem 0.6rem 0.25rem 0;
        border-bottom: 1px solid var(--border);
        color: var(--text-muted);
        font-size: 12px;
    }
    .columns > span {
        position: relative;
    }
    .columns > :first-child {
        padding-left: 0.6rem;
    }
    .columns.nograph > :first-child {
        padding-left: 0;
    }
    .viewport {
        position: relative;
        flex: 1;
        min-height: 0;
    }
    .list {
        position: absolute;
        inset: 0;
        overflow-y: auto;
        overflow-x: hidden;
    }
    .list:focus-visible {
        outline-offset: -2px;
    }
    .spacer {
        position: relative;
    }
    .row {
        position: absolute;
        right: 0;
        left: 0;
        height: var(--rh);
        padding-right: 0.6rem;
        white-space: nowrap;
        cursor: default;
    }
    .row:hover {
        background: var(--surface);
    }
    .row.selected {
        background: var(--surface-2);
    }
    .row.faded .subject,
    .row.faded .author,
    .row.faded .date,
    .row.faded .sha {
        opacity: 0.45;
    }
    .subject,
    .author {
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .date,
    .sha {
        color: var(--text-muted);
    }
    .sha {
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-size: 12px;
    }
    .loading {
        grid-column: 2;
        color: var(--text-muted);
    }
    .ref {
        display: inline-block;
        margin-right: 0.35rem;
        padding: 0 0.4rem;
        border: 1px solid var(--accent);
        border-radius: 9px;
        color: var(--accent);
        font-size: 11.5px;
        line-height: 1.45;
        vertical-align: 1px;
    }
    /* A long branch name is cut inside its pill, so the pills never push the subject out. */
    .ref {
        max-width: 11rem;
        overflow: hidden;
        text-overflow: ellipsis;
        vertical-align: bottom;
    }
    .fork {
        display: inline-block;
        margin-right: 0.35rem;
        padding: 0 0.4rem;
        border: 1px dashed currentColor;
        border-radius: 9px;
        font-size: 11.5px;
        line-height: 1.45;
        vertical-align: 1px;
    }
    .ref.remote {
        border-color: var(--text-muted);
        color: var(--text-muted);
    }
    .ref.tag {
        border-color: var(--warning);
        color: var(--warning);
    }
    .ref.head {
        background: var(--accent);
        color: var(--accent-text);
        font-weight: 600;
    }
    /* Inside the spacer, so it scrolls with the rows; above their backgrounds. */
    .graph {
        position: absolute;
        left: 0;
        overflow: hidden;
        pointer-events: none;
    }
    .empty {
        position: absolute;
        top: 1rem;
        right: 0;
        left: 0;
        margin: 0;
        color: var(--text-muted);
        text-align: center;
    }
</style>
