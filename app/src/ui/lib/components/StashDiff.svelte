<script lang="ts">
    // What a stash holds: its tracked changes (compared with the commit it was made on) and its
    // untracked files (kept by git in a commit of their own, shown as added), each in DiffView.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ChangedFile } from "../bindings/ChangedFile";
    import type { StashEntry } from "../bindings/StashEntry";
    import { formatTime } from "../history";
    import { stashTitle } from "../stash";
    import DiffView from "./DiffView.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import Splitter from "./Splitter.svelte";

    let {
        projectId,
        stash,
        diffMode = $bindable("unified"),
    }: {
        projectId: string;
        stash: StashEntry;
        diffMode?: "unified" | "split";
    } = $props();

    // The file list and the diff share the pane; the user can move the line between them. The size
    // is a fraction of the whole pane, measured to the line, so the list's own height is that less
    // the heading above it.
    let box = $state<HTMLDivElement>();
    let boxH = $state(0);
    let headH = $state(0);
    let listH = $state(0);
    let split = $state<number | null>(null);

    /** A file of the stash, with the commit and parent its diff is read from. */
    type Item = {
        file: ChangedFile;
        commit: string;
        parent: number | null;
        untracked: boolean;
    };

    let items = $state<Item[]>([]);
    let truncated = $state(false);
    let error = $state<AppError | null>(null);
    let loading = $state(false);
    let pickedKey = $state<string | null>(null);
    let request = 0;

    const key = (i: Item) => `${i.untracked ? "u" : "t"}:${i.file.path}`;
    const selected = $derived(
        items.find((i) => key(i) === pickedKey) ?? items[0] ?? null,
    );

    $effect(() => {
        const [id, commit, third] = [
            projectId,
            stash.commit,
            stash.untrackedCommit,
        ];
        const mine = ++request;
        loading = true;
        error = null;
        void (async () => {
            const tracked = await api.commitDetail(id, commit, 0);
            const untracked = third
                ? await api.commitDetail(id, third, null)
                : null;
            return { tracked, untracked };
        })().then(
            ({ tracked, untracked }) => {
                if (mine !== request) return;
                items = [
                    ...tracked.files.map((file) => ({
                        file,
                        commit,
                        parent: 0,
                        untracked: false,
                    })),
                    ...(untracked?.files ?? []).map((file) => ({
                        file,
                        commit: third as string,
                        parent: null,
                        untracked: true,
                    })),
                ];
                truncated =
                    tracked.filesTruncated || !!untracked?.filesTruncated;
                loading = false;
            },
            (e) => {
                if (mine !== request) return;
                error = e as AppError;
                items = [];
                loading = false;
            },
        );
    });

    // Another stash starts from its first file.
    $effect.pre(() => {
        void stash.commit;
        pickedKey = null;
    });

    const dir = (p: string) => p.slice(0, p.lastIndexOf("/") + 1);
    const base = (p: string) => p.slice(p.lastIndexOf("/") + 1);
    const LETTER = {
        added: "A",
        deleted: "D",
        modified: "M",
        renamed: "R",
        typeChanged: "T",
    } as const;
</script>

<div class="stash" bind:this={box} bind:clientHeight={boxH}>
    <div class="head" bind:clientHeight={headH}>
        <strong>{stashTitle(stash)}</strong>
        <span class="muted">
            {formatTime(stash.created)}{stash.branch
                ? ` · on ${stash.branch}`
                : ""}
        </span>
    </div>
    {#if error}
        <div class="pad"><ErrorBox {error} /></div>
    {:else if loading && !items.length}
        <p class="muted pad">Loading…</p>
    {:else if !items.length}
        <p class="muted pad">This stash holds no files.</p>
    {:else}
        <ul
            class="files"
            class:sized={split !== null}
            bind:clientHeight={listH}
            style:height={split === null
                ? undefined
                : `${Math.max(64, split * boxH - headH)}px`}
            aria-label="Files in the stash"
        >
            {#each items as i (key(i))}
                <li class:sel={selected && key(i) === key(selected)}>
                    <button
                        type="button"
                        title={i.file.path}
                        aria-current={selected && key(i) === key(selected)
                            ? "true"
                            : undefined}
                        onclick={() => (pickedKey = key(i))}
                    >
                        <span
                            class="kind {i.untracked
                                ? 'untracked'
                                : i.file.status}"
                            title={i.untracked ? "untracked" : i.file.status}
                        >
                            {i.untracked ? "U" : LETTER[i.file.status]}
                        </span>
                        <span class="name">
                            <span class="dir">{dir(i.file.path)}</span>{base(
                                i.file.path,
                            )}
                        </span>
                    </button>
                </li>
            {/each}
        </ul>
        {#if truncated}
            <p class="muted pad">More files changed than are listed.</p>
        {/if}
        <Splitter
            {box}
            axis="y"
            bind:value={split}
            current={boxH ? (headH + listH) / boxH : 0.3}
            min={0.1}
            max={0.8}
            minPx={headH + 64}
            endMinPx={160}
            label="Resize the stash file list"
            storageKey="stash.files"
        />
        <div class="diff">
            {#if selected}
                {#key key(selected)}
                    <DiffView
                        {projectId}
                        commit={selected.commit}
                        parent={selected.parent}
                        file={selected.file}
                        bind:mode={diffMode}
                    />
                {/key}
            {/if}
        </div>
    {/if}
</div>

<style>
    .stash {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
    }
    .head {
        display: flex;
        flex-wrap: wrap;
        align-items: baseline;
        gap: 0.2rem 0.75rem;
        padding: 0.5rem 0.75rem;
        border-bottom: 1px solid var(--border);
    }
    .files {
        flex: none;
        max-height: 30%;
        min-height: 0;
        margin: 0;
        padding: 0.25rem;
        overflow: auto;
        border-bottom: 1px solid var(--border);
        list-style: none;
    }
    .files.sized {
        max-height: none;
    }
    li {
        border-radius: 4px;
    }
    li:hover,
    li.sel {
        background: var(--surface-2);
    }
    li.sel {
        box-shadow: inset 2px 0 var(--accent);
    }
    li button {
        display: flex;
        align-items: center;
        gap: 0.45rem;
        width: 100%;
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
    .kind.deleted {
        color: var(--danger);
    }
    .kind.modified,
    .kind.typeChanged {
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
    .dir,
    .muted {
        color: var(--text-muted);
    }
    .diff {
        flex: 1;
        min-height: 0;
    }
    .pad {
        margin: 0;
        padding: 0.75rem;
    }
</style>
