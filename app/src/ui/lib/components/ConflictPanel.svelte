<script lang="ts">
    // Explains a conflicted file in plain words and lets the user settle it here: a button per
    // disagreement (mine, theirs, both), an editor for anything in between, and whole-file
    // choices. Saving takes a restore point first, so it can be undone from Activity.
    import { actions, outcomeNotice } from "../actions.svelte";
    import { api } from "../api";
    import type { Action } from "../bindings/Action";
    import type { AppError } from "../bindings/AppError";
    import type { ConflictInfo } from "../bindings/ConflictInfo";
    import {
        type Choice,
        explain,
        findRegions,
        resolveRegion,
        sideNames,
        steps,
        withLineEnds,
    } from "../conflicts";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        path,
        version,
        onreload,
        onopenfile,
    }: {
        projectId: string;
        path: string;
        /** Changes when the repository does, so the explanation follows edits to the file. */
        version: number;
        onreload: () => Promise<void>;
        /** Open a file in the Files tab. */
        onopenfile: (path: string) => void;
    } = $props();

    let info = $state<ConflictInfo | null>(null);
    let error = $state<AppError | null>(null);
    let notice = $state("");
    /** The text in the editor. */
    let draft = $state("");
    /** What the file held when last read; the editor follows the file only while it is unchanged. */
    let loaded = "";
    let loadedPath = "";
    let request = 0;

    $effect(() => {
        const [id, p] = [projectId, path];
        void version;
        const mine = ++request;
        api.conflictInfo(id, p).then(
            (i) => {
                if (mine !== request) return;
                info = i;
                error = null;
                const text = i.text ?? "";
                if (p !== loadedPath || draft === loaded) draft = text;
                loaded = text;
                loadedPath = p;
            },
            (e) => {
                if (mine !== request) return;
                error = e as AppError;
                info = null;
            },
        );
    });

    const text = $derived(info ? explain(info) : null);
    const names = $derived(info ? sideNames(info) : null);
    const regions = $derived(findRegions(draft));
    const edited = $derived(draft !== loaded);
    const busy = $derived(!!actions.busy[projectId]);
    const canEdit = $derived(!!info && info.text !== null);
    const hasBothSides = $derived(
        info?.kind === "bothModified" || info?.kind === "bothAdded",
    );
    const removal = $derived(
        info?.kind === "deletedByUs" ||
            info?.kind === "deletedByThem" ||
            info?.kind === "bothDeleted",
    );

    const SHOWN = 8;
    const shown = (lines: string[]) =>
        lines.length > SHOWN
            ? [...lines.slice(0, SHOWN), `… ${lines.length - SHOWN} more lines`]
            : lines;

    function choose(i: number, c: Choice) {
        draft = resolveRegion(draft, i, c);
    }

    async function act(action: Action) {
        error = null;
        notice = "";
        try {
            const outcome = await actions.run(projectId, action);
            if (outcome) notice = outcomeNotice(outcome).text;
        } catch (e) {
            error = e as AppError;
        } finally {
            await onreload();
        }
    }

    const save = () =>
        act({
            type: "resolve",
            path,
            keep: { type: "edited", content: withLineEnds(draft, loaded) },
        });
</script>

<div class="conflict">
    {#if error}
        <ErrorBox {error} />
    {/if}
    {#if info && text && names}
        <h3>Conflict in <code>{info.path}</code></h3>
        <p class="headline">{text.headline}</p>
        <p>{text.why}</p>

        {#if hasBothSides || removal || info.kind === "addedByUs" || info.kind === "addedByThem"}
            <div class="whole">
                {#if info.kind !== "deletedByUs" && info.kind !== "bothDeleted" && info.kind !== "addedByThem"}
                    <button
                        type="button"
                        disabled={busy}
                        title={`Use the whole file as it is on ${names.you}, and mark it resolved. A restore point is taken first`}
                        onclick={() =>
                            act({
                                type: "resolve",
                                path,
                                keep: { type: "ours" },
                            })}
                    >
                        Keep my file
                    </button>
                {/if}
                {#if info.kind !== "deletedByThem" && info.kind !== "bothDeleted" && info.kind !== "addedByUs"}
                    <button
                        type="button"
                        disabled={busy}
                        title={`Use the whole file as it is on ${names.them}, and mark it resolved. A restore point is taken first`}
                        onclick={() =>
                            act({
                                type: "resolve",
                                path,
                                keep: { type: "theirs" },
                            })}
                    >
                        Keep their file
                    </button>
                {/if}
                {#if removal}
                    <button
                        type="button"
                        disabled={busy}
                        title="Delete the file and mark it resolved. A restore point is taken first"
                        onclick={() =>
                            act({
                                type: "resolve",
                                path,
                                keep: { type: "delete" },
                            })}
                    >
                        Delete the file
                    </button>
                {/if}
            </div>
        {/if}

        {#each regions as r, i (i)}
            <section class="region" aria-label={`Disagreement ${i + 1}`}>
                <h4>Disagreement {i + 1} of {regions.length}</h4>
                <div class="sides">
                    <div class="side you">
                        <strong>You ({names.you})</strong>
                        <pre>{r.ours.length
                                ? shown(r.ours).join("\n")
                                : "(nothing)"}</pre>
                    </div>
                    <div class="side them">
                        <strong>{names.them}</strong>
                        <pre>{r.theirs.length
                                ? shown(r.theirs).join("\n")
                                : "(nothing)"}</pre>
                    </div>
                </div>
                <div class="choices">
                    <button
                        type="button"
                        title="Use your lines here and drop the other side's"
                        onclick={() => choose(i, "ours")}
                    >
                        Keep mine
                    </button>
                    <button
                        type="button"
                        title="Use the other side's lines here and drop yours"
                        onclick={() => choose(i, "theirs")}
                    >
                        Keep theirs
                    </button>
                    <button
                        type="button"
                        title="Keep both: your lines, then theirs"
                        onclick={() => choose(i, "both")}
                    >
                        Keep both
                    </button>
                    <button
                        type="button"
                        title="Keep both: their lines, then yours"
                        onclick={() => choose(i, "theirsFirst")}
                    >
                        Both, theirs first
                    </button>
                </div>
            </section>
        {/each}

        {#if canEdit}
            <h4>The file</h4>
            <p class="muted">
                {#if regions.length}
                    {regions.length === 1
                        ? "1 disagreement is"
                        : `${regions.length} disagreements are`}
                    still marked below. Settle {regions.length === 1
                        ? "it"
                        : "each"} with the buttons above, or edit the text by hand
                    and delete the lines starting with &lt;&lt;&lt;&lt;&lt;&lt;&lt;,
                    ======= and &gt;&gt;&gt;&gt;&gt;&gt;&gt;.
                {:else}
                    Nothing is left to settle. Check the text, then save.
                {/if}
            </p>
            <textarea
                bind:value={draft}
                spellcheck="false"
                wrap="off"
                aria-label={`Contents of ${info.path}`}
                title="The file's text. Edit it freely; nothing is written until you press Save"
            ></textarea>
            <div class="choices">
                <button
                    type="button"
                    class="primary"
                    disabled={busy || regions.length > 0}
                    title={regions.length
                        ? "Settle every disagreement first: marker lines are still in the text"
                        : "Write the file as shown and mark it resolved. A restore point is taken first"}
                    onclick={save}
                >
                    Save and mark resolved
                </button>
                <button
                    type="button"
                    disabled={!edited}
                    title="Put the text back as it is on disk"
                    onclick={() => (draft = loaded)}
                >
                    Undo my edits
                </button>
                <button
                    type="button"
                    title="Open this file in the Files tab, a bigger editor. Save there, then come back and mark it resolved"
                    onclick={() => onopenfile(info!.path)}
                >
                    Open in Files tab
                </button>
            </div>
        {:else if hasBothSides}
            <p class="muted">
                This file can't be edited here (it is not plain text, or it is
                too large). Use Keep my file or Keep their file.
            </p>
        {/if}
        {#if notice}
            <p class="muted" role="status">{notice}</p>
        {/if}

        <h4>What to do</h4>
        <ol>
            {#each steps(info) as step (step)}
                <li>{step}</li>
            {/each}
        </ol>
    {:else if !error}
        <p class="muted">Loading…</p>
    {/if}
</div>

<style>
    .conflict {
        height: 100%;
        padding: 0.75rem 1rem;
        overflow: auto;
    }
    h3 {
        margin: 0 0 0.4rem;
        font-size: 1rem;
    }
    h4 {
        margin: 0.9rem 0 0.3rem;
        font-size: 0.9rem;
    }
    p {
        margin: 0.3rem 0;
    }
    .headline {
        font-weight: 600;
    }
    .region {
        margin-top: 0.6rem;
    }
    .region h4 {
        margin-top: 0;
        color: var(--text-muted);
    }
    .sides {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 0.5rem;
    }
    .side {
        min-width: 0;
        padding: 0.4rem 0.6rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--surface);
    }
    .side.you {
        border-top: 2px solid var(--accent);
    }
    .side.them {
        border-top: 2px solid var(--warning);
    }
    pre {
        margin: 0.3rem 0 0;
        overflow: auto;
        font-size: 0.85em;
    }
    .whole,
    .choices {
        display: flex;
        flex-wrap: wrap;
        gap: 0.4rem;
        margin: 0.6rem 0;
    }
    textarea {
        width: 100%;
        min-height: 14rem;
        resize: vertical;
        font-family: var(--mono, monospace);
        font-size: 0.85em;
        tab-size: 4;
    }
    ol {
        margin: 0.3rem 0;
        padding-left: 1.3rem;
    }
    li {
        margin: 0.25rem 0;
    }
    .muted {
        color: var(--text-muted);
    }
</style>
