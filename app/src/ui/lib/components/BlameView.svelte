<script lang="ts">
    // Who last changed each line of a file, as of a commit. Clicking a commit selects it.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { Blame } from "../bindings/Blame";
    import { highlight } from "../highlight";
    import {
        formatDay,
        formatTime,
        langForPath,
        shortId,
        type Token,
    } from "../history";
    import CodeText from "./CodeText.svelte";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        commit,
        path,
        onjump,
    }: {
        projectId: string;
        commit: string;
        path: string;
        onjump: (commit: string) => void;
    } = $props();

    const STEP = 3000;
    let blame = $state<Blame | null>(null);
    let error = $state<AppError | null>(null);
    let tokens = $state<Token[][] | null>(null);
    let limit = $state(STEP);
    let request = 0;

    $effect(() => {
        const [p, c, f] = [projectId, commit, path];
        const mine = ++request;
        blame = null;
        error = null;
        tokens = null;
        limit = STEP;
        api.fileBlame(p, c, f).then(
            async (b) => {
                if (mine !== request) return;
                blame = b;
                const t = await highlight(
                    b.lines.join("\n"),
                    langForPath(b.path),
                );
                if (mine === request) tokens = t;
            },
            (e) => {
                if (mine === request) error = e as AppError;
            },
        );
    });

    /** For each line, the hunk it belongs to and whether it starts that hunk. */
    const owners = $derived.by(() => {
        if (!blame) return [];
        const out: { hunk: number; first: boolean }[] = [];
        blame.hunks.forEach((h, hi) => {
            for (let i = 0; i < h.lines; i++)
                out[h.start - 1 + i] = { hunk: hi, first: i === 0 };
        });
        return out;
    });
</script>

<div class="blame">
    {#if error}
        <ErrorBox {error} />
    {:else if !blame}
        <p class="note">Loading…</p>
    {:else if blame.binary}
        <p class="note">Binary file; nothing to blame.</p>
    {:else if blame.tooLarge}
        <p class="note">This file is over 1 MiB; blame is not shown.</p>
    {:else}
        <div class="code">
            {#each blame.lines.slice(0, limit) as text, i (i)}
                {@const o = owners[i]}
                {@const h = o ? blame.hunks[o.hunk] : undefined}
                <div
                    class="line"
                    class:start={o?.first}
                    class:odd={o && o.hunk % 2 === 1}
                >
                    <span class="who">
                        {#if h && o?.first}
                            <button
                                type="button"
                                class="link"
                                title="{h.subject}{h.sourcePath
                                    ? ` (as ${h.sourcePath})`
                                    : ''}"
                                onclick={() => onjump(h.commit)}
                            >
                                {shortId(h.commit)}
                            </button>
                            <span class="author">{h.authorName}</span>
                            <span class="date" title={formatTime(h.authorTime)}>
                                {formatDay(h.authorTime)}
                            </span>
                        {/if}
                    </span>
                    <span class="no">{i + 1}</span>
                    <span class="text">
                        <CodeText {text} tokens={tokens?.[i]} />
                    </span>
                </div>
            {/each}
        </div>
        {#if blame.lines.length > limit}
            <p class="note">
                Showing {limit} of {blame.lines.length} lines.
                <button
                    title="Load the next lines of this file"
                    type="button"
                    onclick={() => (limit += STEP)}
                >
                    Show more
                </button>
            </p>
        {/if}
    {/if}
</div>

<style>
    .blame {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
    }
    .note {
        margin: 0;
        padding: 0.6rem;
        color: var(--text-muted);
    }
    .code {
        flex: 1;
        min-height: 0;
        overflow: auto;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-size: 12.5px;
        line-height: 1.45;
    }
    .line {
        display: grid;
        grid-template-columns: 16rem 3.2rem max-content;
        min-width: 100%;
        width: max-content;
        white-space: pre;
    }
    .line.odd .who {
        background: var(--surface);
    }
    .line.start {
        border-top: 1px solid var(--border);
    }
    .who {
        display: flex;
        gap: 0.6rem;
        overflow: hidden;
        padding: 0 0.5rem;
        font-family: system-ui, sans-serif;
        font-size: 12px;
    }
    .author {
        overflow: hidden;
        flex: 1;
        text-overflow: ellipsis;
    }
    .date {
        color: var(--text-muted);
    }
    .link {
        padding: 0;
        border: none;
        background: none;
        color: var(--accent);
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
    }
    .no {
        padding-right: 0.6rem;
        color: var(--text-muted);
        text-align: right;
        user-select: none;
    }
    .text {
        padding-right: 1rem;
    }
</style>
