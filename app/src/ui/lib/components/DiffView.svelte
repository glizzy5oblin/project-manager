<script lang="ts">
    // One file's changes in a commit (or, with `work`, its uncommitted change), unified or side
    // by side, with syntax colors and changed words marked. Binary, oversized, mode-only and renamed files get a short explanation.
    import { untrack } from "svelte";
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ChangedFile } from "../bindings/ChangedFile";
    import type { DiffLine } from "../bindings/DiffLine";
    import type { FileDiff } from "../bindings/FileDiff";
    import type { Hunk } from "../bindings/Hunk";
    import type { WorkSide } from "../bindings/WorkSide";
    import { highlight } from "../highlight";
    import { langForPath, splitRows, type Token } from "../history";
    import CodeText from "./CodeText.svelte";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        commit = "",
        parent = null,
        work = null,
        version = 0,
        file,
        mode = $bindable("unified"),
        hunkAction = null,
    }: {
        projectId: string;
        commit?: string;
        parent?: number | null;
        /** Show the file's staged or unstaged change instead of a commit's. */
        work?: WorkSide | null;
        /** Bump to load again (the working tree changed). */
        version?: number;
        file: ChangedFile;
        mode?: "unified" | "split";
        /**
         * A button on each hunk header of a modified text file (staging one hunk). The diff
         * reloads once `run` settles.
         */
        hunkAction?: {
            label: string;
            title: string;
            disabled: boolean;
            run: (hunk: Hunk) => Promise<unknown>;
        } | null;
    } = $props();

    /** Lines shown before "Show more"; a huge forced diff would otherwise freeze the view. */
    const STEP = 3000;
    const HARD_LIMIT = 16 * 1024 * 1024;
    /** Over this the backend wants `force` (history/diff.rs LARGE_FILE). */
    const LARGE = 1024 * 1024;

    // Raw, not deep state: colorize keys tokens by the line objects the template iterates.
    let diff = $state.raw<FileDiff | null>(null);
    let error = $state<AppError | null>(null);
    let loading = $state(false);
    let limit = $state(STEP);
    let tokens = $state.raw(new Map<DiffLine, Token[]>());
    let request = 0;

    async function load(force: boolean) {
        const mine = ++request;
        loading = true;
        error = null;
        try {
            const d = work
                ? await api.workDiff(projectId, file.path, work, force)
                : await api.fileDiff(
                      projectId,
                      commit,
                      parent,
                      file.path,
                      file.oldPath,
                      force,
                  );
            if (mine !== request) return;
            diff = d;
            limit = STEP;
            tokens = new Map();
            void colorize(d, mine);
        } catch (e) {
            if (mine === request) error = e as AppError;
        } finally {
            if (mine === request) loading = false;
        }
    }

    /** Highlight each hunk's old and new sides as blocks, so multi-line constructs mostly work. */
    async function colorize(d: FileDiff, mine: number) {
        const lang = langForPath(d.path);
        if (!lang || d.hunks.length === 0) return;
        // Built here, then swapped into state whole; never mutated while reactive.
        // eslint-disable-next-line svelte/prefer-svelte-reactivity
        const map = new Map<DiffLine, Token[]>();
        for (const h of d.hunks) {
            for (const side of ["old", "new"] as const) {
                const lines = h.lines.filter((l) =>
                    side === "old" ? l.kind !== "added" : l.kind !== "deleted",
                );
                const toks = await highlight(
                    lines.map((l) => l.text).join("\n"),
                    lang,
                );
                if (mine !== request) return;
                if (!toks) return;
                lines.forEach((l, i) => {
                    // Context lines appear on both sides; keep the new side's tokens.
                    if (toks[i]) map.set(l, toks[i]);
                });
            }
        }
        tokens = map;
    }

    $effect(() => {
        void [projectId, commit, parent, work, file.path, file.oldPath];
        diff = null;
        void load(false);
    });
    // A working-tree change reloads in place, keeping what is on screen until the new diff
    // arrives (and keeping a forced large diff forced).
    let seen = untrack(() => version);
    $effect(() => {
        if (version === seen) return;
        seen = version;
        if (work) void load(diff ? !diff.tooLarge && big > LARGE : false);
    });

    const total = $derived(
        diff?.hunks.reduce((n, h) => n + h.lines.length, 0) ?? 0,
    );

    /** Hunks cut down to the first `limit` lines; `whole` when none of a hunk is hidden. */
    const shown = $derived.by(() => {
        if (!diff) return [];
        let left = limit;
        const out = [];
        for (const h of diff.hunks) {
            if (left <= 0) break;
            out.push({
                hunk: h,
                lines: h.lines.slice(0, left),
                whole: h.lines.length <= left,
            });
            left -= h.lines.length;
        }
        return out;
    });

    const LINK = "120000";
    /** Only a modified text file can be staged in parts (actions/hunks.rs refuses the rest). */
    const partial = $derived(
        !!hunkAction &&
            diff?.status === "modified" &&
            diff.oldMode !== LINK &&
            diff.newMode !== LINK,
    );

    async function runHunk(h: Hunk) {
        if (!hunkAction) return;
        try {
            await hunkAction.run(h);
        } finally {
            await load(!!diff && !diff.tooLarge && big > LARGE);
        }
    }

    const sizeText = (n: number | null) =>
        n === null
            ? "—"
            : n < 1024
              ? `${n} B`
              : `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KiB`;
    const big = $derived(Math.max(diff?.oldSize ?? 0, diff?.newSize ?? 0));
    const sign = { context: " ", added: "+", deleted: "−" } as const;
</script>

<div class="diff">
    <header>
        <span class="path" title={file.path}>
            {#if file.oldPath && file.oldPath !== file.path}
                <span class="old">{file.oldPath}</span> →
            {/if}
            {file.path}
        </span>
        {#if file.similarity !== null && file.status === "renamed"}
            <span class="muted">{file.similarity}% similar</span>
        {/if}
        <span class="spacer"></span>
        <div class="modes" role="group" aria-label="Diff layout">
            <button
                title="Show old and new lines in one column"
                type="button"
                class:on={mode === "unified"}
                aria-pressed={mode === "unified"}
                onclick={() => (mode = "unified")}
            >
                Unified
            </button>
            <button
                title="Show old lines on the left and new lines on the right"
                type="button"
                class:on={mode === "split"}
                aria-pressed={mode === "split"}
                onclick={() => (mode = "split")}
            >
                Side by side
            </button>
        </div>
    </header>

    {#if error}
        <ErrorBox {error} />
    {:else if !diff}
        <p class="muted pad">{loading ? "Loading…" : ""}</p>
    {:else}
        {#if diff.oldMode && diff.newMode && diff.oldMode !== diff.newMode}
            <p class="note">
                Mode changed from <code>{diff.oldMode}</code> to
                <code>{diff.newMode}</code>.
            </p>
        {/if}
        {#if diff.binary}
            <p class="note">
                Binary file ({sizeText(diff.oldSize)} → {sizeText(
                    diff.newSize,
                )}); no line diff.
            </p>
        {:else if diff.tooLarge}
            <p class="note">
                This file is large ({sizeText(big)}).
                {#if big <= HARD_LIMIT}
                    <button
                        title="Load this large diff even though it may be slow"
                        type="button"
                        onclick={() => load(true)}
                        disabled={loading}
                    >
                        Show the diff anyway
                    </button>
                {:else}
                    Files over 16 MiB are not shown.
                {/if}
            </p>
        {:else if diff.hunks.length === 0}
            <p class="note">No text changes.</p>
        {:else}
            <div class="code" class:split={mode === "split"}>
                {#each shown as { hunk: h, lines, whole }, hi (hi)}
                    <div class="hunk">
                        <span>
                            @@ −{h.oldStart},{h.oldLines} +{h.newStart},{h.newLines}
                            @@
                        </span>
                        {#if partial && hunkAction && whole}
                            <button
                                type="button"
                                class="hunk-act"
                                disabled={hunkAction.disabled}
                                title={hunkAction.title}
                                onclick={() => runHunk(h)}
                            >
                                {hunkAction.label}
                            </button>
                        {/if}
                    </div>
                    {#if mode === "unified"}
                        {#each lines as l, li (li)}
                            <div class="line {l.kind}">
                                <span class="no">{l.oldLine ?? ""}</span>
                                <span class="no">{l.newLine ?? ""}</span>
                                <span class="sign">{sign[l.kind]}</span>
                                <span class="text">
                                    <CodeText
                                        text={l.text}
                                        tokens={tokens.get(l)}
                                        emphasis={l.emphasis}
                                    />{#if l.noNewline}<span
                                            class="eof"
                                            title="No newline at end of file"
                                        >
                                            ⏎̸
                                        </span>{/if}
                                </span>
                            </div>
                        {/each}
                    {:else}
                        {#each splitRows(lines) as r, ri (ri)}
                            <div class="pair">
                                {#each [r.left, r.right] as l, side (side)}
                                    {#if l}
                                        <div class="line {l.kind}">
                                            <span class="no">
                                                {side === 0
                                                    ? l.oldLine
                                                    : l.newLine}
                                            </span>
                                            <span class="text">
                                                <CodeText
                                                    text={l.text}
                                                    tokens={tokens.get(l)}
                                                    emphasis={l.emphasis}
                                                />
                                            </span>
                                        </div>
                                    {:else}
                                        <div class="line empty"></div>
                                    {/if}
                                {/each}
                            </div>
                        {/each}
                    {/if}
                {/each}
            </div>
            {#if total > limit}
                <p class="note">
                    Showing {limit} of {total} lines.
                    <button
                        title="Load more of this diff"
                        type="button"
                        onclick={() => (limit += STEP)}
                    >
                        Show more
                    </button>
                </p>
            {/if}
        {/if}
    {/if}
</div>

<style>
    .diff {
        display: flex;
        flex-direction: column;
        min-height: 0;
        height: 100%;
    }
    header {
        display: flex;
        align-items: center;
        gap: 0.6rem;
        padding: 0.4rem 0.6rem;
        border-bottom: 1px solid var(--border);
    }
    .path {
        overflow: hidden;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .old {
        color: var(--text-muted);
    }
    .spacer {
        flex: 1;
    }
    .modes {
        display: flex;
        flex-shrink: 0;
    }
    .modes button {
        padding: 0.2rem 0.6rem;
        white-space: nowrap;
        border-radius: 0;
    }
    .modes button:first-child {
        border-radius: 4px 0 0 4px;
    }
    .modes button:last-child {
        border-radius: 0 4px 4px 0;
    }
    .modes button.on {
        border-color: var(--accent);
        color: var(--accent);
    }
    .muted {
        color: var(--text-muted);
    }
    .pad,
    .note {
        margin: 0;
        padding: 0.6rem;
    }
    .note {
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
    .hunk {
        position: sticky;
        left: 0;
        display: flex;
        align-items: center;
        gap: 0.75rem;
        padding: 0.15rem 0.6rem;
        background: var(--surface-2);
        color: var(--text-muted);
    }
    .hunk-act {
        padding: 0 0.5rem;
        /* The UI's font, not the code's. */
        font-family:
            system-ui,
            -apple-system,
            "Segoe UI",
            sans-serif;
        font-size: 0.8rem;
    }
    .line {
        display: grid;
        grid-template-columns: 3.2rem 3.2rem 1.2rem max-content;
        min-width: 100%;
        width: max-content;
        white-space: pre;
    }
    .split .line {
        grid-template-columns: 3.2rem 1fr;
        width: auto;
        overflow: hidden;
    }
    .pair {
        display: grid;
        grid-template-columns: 1fr 1fr;
    }
    .pair .line:first-child {
        border-right: 1px solid var(--border);
    }
    .no {
        padding-right: 0.5rem;
        color: var(--text-muted);
        text-align: right;
        user-select: none;
    }
    .sign {
        color: var(--text-muted);
        user-select: none;
    }
    .text {
        padding-right: 1rem;
    }
    .split .text {
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .added {
        background: var(--diff-added);
    }
    .deleted {
        background: var(--diff-deleted);
    }
    .empty {
        background: var(--surface);
    }
    .eof {
        color: var(--danger);
    }
</style>
