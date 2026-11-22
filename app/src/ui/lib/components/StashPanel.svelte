<script lang="ts">
    // The stash list in the Changes view: set the current work aside (message, untracked files,
    // or only what is staged), look at a stash, apply or pop it, drop one or clear them all.
    // Every write goes through `act`, so drop and clear are confirmed and backed up first.
    import { api } from "../api";
    import type { Action } from "../bindings/Action";
    import type { AppError } from "../bindings/AppError";
    import type { GitVersion } from "../bindings/GitVersion";
    import type { StashEntry } from "../bindings/StashEntry";
    import { formatDay, formatTime } from "../history";
    import { gitAtLeast, stashMeta, stashTitle } from "../stash";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        changed,
        busy,
        canStash,
        stashHint,
        selected,
        list = $bindable([]),
        onselect,
        act,
        gitVersion,
    }: {
        projectId: string;
        /** Bumped when the project's files change on disk; the list reloads. */
        changed: number;
        busy: boolean;
        /** There is something to stash (and nothing stops it). */
        canStash: boolean;
        /** Why stashing is not possible now, shown as the button's tooltip. */
        stashHint: string;
        /** The stash shown on the right, by commit id. */
        selected: string | null;
        list?: StashEntry[];
        onselect: (commit: string | null) => void;
        act: (action: Action, done?: string) => Promise<boolean>;
        gitVersion: GitVersion | null;
    } = $props();

    let error = $state<AppError | null>(null);
    let open = $state(false);
    let message = $state("");
    let untracked = $state(false);
    let stagedOnly = $state(false);
    let request = 0;

    const stagedSupported = $derived(gitAtLeast(gitVersion));

    $effect(() => {
        void changed;
        const id = projectId;
        const mine = ++request;
        api.stashList(id).then(
            (l) => {
                if (mine !== request) return;
                list = l;
                error = null;
            },
            (e) => {
                if (mine === request) error = e as AppError;
            },
        );
    });

    // Staged only cannot take untracked files along; picking one clears the other.
    function setUntracked(v: boolean) {
        untracked = v;
        if (v) stagedOnly = false;
    }
    function setStagedOnly(v: boolean) {
        stagedOnly = v;
        if (v) untracked = false;
    }

    async function push() {
        const ok = await act({
            type: "stashPush",
            message: message.trim() || null,
            includeUntracked: untracked,
            stagedOnly,
        });
        if (ok) {
            message = "";
            open = false;
        }
    }

    function key(e: KeyboardEvent) {
        if (e.key === "Enter") {
            e.preventDefault();
            void push();
        } else if (e.key === "Escape") {
            open = false;
        }
    }

    const apply = (s: StashEntry, index: boolean) =>
        act({ type: "stashApply", stash: s.commit, index });
    const pop = (s: StashEntry, index: boolean) =>
        act({ type: "stashPop", stash: s.commit, index });
    const drop = (s: StashEntry) => act({ type: "stashDrop", stash: s.commit });
    const clear = () => act({ type: "stashClear" });

    /** Apply and pop bring the staged changes back as staged when asked to. */
    let withIndex = $state(false);
</script>

<section class="stashes">
    <h3>
        <span>Stashes <span class="n">{list.length}</span></span>
        <span class="group">
            {#if list.length > 1}
                <button
                    title="Delete every stash. A backup is taken first"
                    type="button"
                    disabled={busy}
                    onclick={clear}
                >
                    Clear all
                </button>
            {/if}
            <button
                type="button"
                disabled={busy || (!canStash && !open)}
                title={canStash ? "Set the current changes aside" : stashHint}
                aria-expanded={open}
                onclick={() => (open = !open)}
            >
                Stash…
            </button>
        </span>
    </h3>

    {#if open}
        <div class="form">
            <input
                title="A note to remember this stash by (optional)"
                type="text"
                bind:value={message}
                onkeydown={key}
                placeholder="Message (optional)"
                aria-label="Stash message"
            />
            <label>
                <input
                    title="Also stash new files that git is not tracking yet"
                    type="checkbox"
                    checked={untracked}
                    onchange={(e) => setUntracked(e.currentTarget.checked)}
                />
                Include untracked files
            </label>
            {#if stagedSupported}
                <label title="Set aside only what is staged; the rest stays">
                    <input
                        title="Stash only the changes you have staged"
                        type="checkbox"
                        checked={stagedOnly}
                        onchange={(e) => setStagedOnly(e.currentTarget.checked)}
                    />
                    Only the staged changes
                </label>
            {/if}
            <div class="buttons">
                <button
                    title="Close without stashing anything"
                    type="button"
                    onclick={() => (open = false)}
                >
                    Cancel
                </button>
                <button
                    type="button"
                    class="primary"
                    disabled={busy || !canStash}
                    title={canStash ? "" : stashHint}
                    onclick={push}
                >
                    Stash
                </button>
            </div>
        </div>
    {/if}

    {#if error}
        <ErrorBox {error} />
    {/if}
    {#if list.length}
        <label
            class="index"
            title="Stage again what was staged when it was stashed"
        >
            <input
                title="Also put the files back as staged, as they were when stashed"
                type="checkbox"
                bind:checked={withIndex}
            />
            Restore the staged state when applying
        </label>
    {/if}
    <ul>
        {#each list as s (s.commit)}
            <li class:sel={selected === s.commit}>
                <button
                    type="button"
                    class="entry"
                    aria-current={selected === s.commit ? "true" : undefined}
                    title={`${stashTitle(s)}\n${formatTime(s.created)}`}
                    onclick={() =>
                        onselect(selected === s.commit ? null : s.commit)}
                >
                    <span class="title">{stashTitle(s)}</span>
                    <span class="meta">
                        {stashMeta(s)} · {formatDay(s.created)}
                    </span>
                </button>
                {#if selected === s.commit}
                    <span class="row-actions">
                        <button
                            type="button"
                            disabled={busy}
                            title="Apply: bring the changes back and keep the stash"
                            aria-label={`Apply stash ${stashTitle(s)}`}
                            onclick={() => apply(s, withIndex)}
                        >
                            Apply
                        </button>
                        <button
                            type="button"
                            disabled={busy}
                            title="Pop: apply, then remove the stash"
                            aria-label={`Pop stash ${stashTitle(s)}`}
                            onclick={() => pop(s, withIndex)}
                        >
                            Pop
                        </button>
                        <button
                            type="button"
                            disabled={busy}
                            title="Drop: delete the stash (a backup is taken)"
                            aria-label={`Drop stash ${stashTitle(s)}`}
                            onclick={() => drop(s)}
                        >
                            Drop
                        </button>
                    </span>
                {/if}
            </li>
        {:else}
            <li class="empty">No stashes.</li>
        {/each}
    </ul>
</section>

<style>
    h3 {
        display: flex;
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
    .form {
        display: grid;
        gap: 0.4rem;
        margin-bottom: 0.5rem;
        padding: 0.5rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--surface);
    }
    .form input[type="text"] {
        padding: 0.3rem 0.45rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--bg);
        color: var(--text);
        font: inherit;
    }
    label {
        display: flex;
        align-items: center;
        gap: 0.35rem;
        color: var(--text-muted);
        font-size: 0.9em;
    }
    .index {
        margin-bottom: 0.3rem;
        font-size: 0.85em;
    }
    .buttons {
        display: flex;
        justify-content: flex-end;
        gap: 0.4rem;
    }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        flex-wrap: wrap;
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
    .entry {
        display: grid;
        flex: 1;
        min-width: 0;
        padding: 0.25rem 0.4rem;
        border: none;
        background: none;
        text-align: left;
    }
    .title,
    .meta {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .meta {
        color: var(--text-muted);
        font-size: 0.8em;
    }
    .row-actions {
        display: flex;
        gap: 2px;
        padding: 0 0.25rem 0.25rem 0.4rem;
        width: 100%;
    }
    .row-actions button {
        padding: 0.05rem 0.5rem;
        font-size: 0.8em;
    }
</style>
