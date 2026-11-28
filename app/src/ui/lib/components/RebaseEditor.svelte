<script lang="ts">
    // The commit editor (interactive rebase): the current branch's commits from one on, oldest
    // first, to reorder, squash, fix up, drop or reword. Applying runs one rebase through the
    // action runner, so it is confirmed with its impact and backed up first (docs/actions.md).
    import { actions } from "../actions.svelte";
    import type { ActionOutcome } from "../bindings/ActionOutcome";
    import type { AppError } from "../bindings/AppError";
    import type { RebaseRange } from "../bindings/RebaseRange";
    import type { RepoSide } from "../bindings/RepoSide";
    import type { TodoOp } from "../bindings/TodoOp";
    import {
        OP_HINT,
        OP_LABEL,
        defaultMessage,
        finalMessage,
        groupOf,
        hasChanges,
        isReworded,
        move,
        setMessage,
        setOp,
        startRows,
        summary,
        toTodo,
        todoProblem,
        type TodoRow,
    } from "../rebaseTodo";
    import ErrorBox from "./ErrorBox.svelte";
    import Modal from "./Modal.svelte";

    let {
        projectId,
        side = "private",
        range,
        onclose,
        ondone,
    }: {
        projectId: string;
        /** Which repository of a two-repo project this rebase runs on. */
        side?: RepoSide;
        range: RebaseRange;
        onclose: () => void;
        /** The rebase ran (finished, or stopped on a conflict). */
        ondone: (outcome: ActionOutcome) => void;
    } = $props();

    // The editor is opened afresh for each range, so its first list is all it needs.
    // svelte-ignore state_referenced_locally
    let rows = $state<TodoRow[]>(startRows(range));
    /** Commits whose message editor is open, by id. */
    let editing = $state<Record<string, boolean>>({});
    let error = $state<AppError | null>(null);

    const OPS: TodoOp[] = ["pick", "squash", "fixup", "drop"];

    const busy = $derived(!!actions.busy[projectId]);
    const changed = $derived(hasChanges(range, rows));
    const problem = $derived(todoProblem(range.branch, rows));
    const groups = $derived(groupOf(rows));
    const pushed = $derived(range.commits.filter((c) => c.pushed).length);
    const applyWhy = $derived(
        busy
            ? "Another action is running on this project"
            : !changed
              ? "Nothing to apply yet: reorder, squash, drop or reword a commit first"
              : problem,
    );

    function setRowOp(i: number, op: TodoOp) {
        rows = setOp(rows, i, op);
        if (op !== "pick") editing[rows[i].commit.id] = false;
    }

    async function apply() {
        error = null;
        try {
            const outcome = await actions.run(
                projectId,
                {
                    type: "rebaseInteractive",
                    base: range.base,
                    todo: toTodo(rows),
                },
                false,
                side,
            );
            // Null: the confirmation was turned down. The list stays as it is.
            if (outcome) ondone(outcome);
        } catch (e) {
            error = e as AppError;
        }
    }
</script>

<Modal title={`Edit commits of ${range.branch}`} {onclose} wide>
    <p class="intro">
        Oldest first. Keep, squash or fix up each commit into the kept commit
        above it, or drop it; move commits to change their order and open a kept
        commit's message to reword it. Applying rewrites them, from the first
        change on, with new ids. A backup is taken first.
    </p>
    {#if pushed}
        <p class="warn">
            {pushed === 1
                ? "1 of these commits is"
                : `${pushed} of these commits are`} already on a remote branch: publishing
            the edited ones will need a force push.
        </p>
    {/if}
    <ol class="rows">
        {#each rows as row, i (row.commit.id)}
            {@const c = row.commit}
            {@const folded = row.op === "squash" || row.op === "fixup"}
            {@const orphan = folded && groups[i] === null}
            <li
                class="row"
                class:folded
                class:dropped={row.op === "drop"}
                class:orphan
            >
                <span class="move">
                    <button
                        type="button"
                        title="Move this commit up (earlier)"
                        aria-label={`Move ${c.shortId} up`}
                        disabled={i === 0}
                        onclick={() => (rows = move(rows, i, -1))}
                    >
                        ↑
                    </button>
                    <button
                        type="button"
                        title="Move this commit down (later)"
                        aria-label={`Move ${c.shortId} down`}
                        disabled={i === rows.length - 1}
                        onclick={() => (rows = move(rows, i, 1))}
                    >
                        ↓
                    </button>
                </span>
                <select
                    value={row.op}
                    title={OP_HINT[row.op]}
                    aria-label={`What happens to ${c.shortId}`}
                    onchange={(e) =>
                        setRowOp(i, e.currentTarget.value as TodoOp)}
                >
                    {#each OPS as op (op)}
                        <option value={op} title={OP_HINT[op]}>
                            {OP_LABEL[op]}
                        </option>
                    {/each}
                </select>
                <code class="sha" title={c.id}>{c.shortId}</code>
                <span class="subject" title={c.message.trimEnd()}>
                    {#if folded}<span class="into" aria-hidden="true">
                            ↳
                        </span>{/if}
                    {row.op === "pick" && isReworded(rows, i)
                        ? (finalMessage(rows, i) ?? "").split("\n")[0]
                        : c.subject}
                </span>
                {#if row.op === "pick" && isReworded(rows, i)}
                    <span class="tag" title={`Was: ${c.subject}`}>
                        reworded
                    </span>
                {/if}
                {#if c.pushed}
                    <span
                        class="tag"
                        title="Already on a remote branch: publishing an edited version needs a force push"
                    >
                        pushed
                    </span>
                {/if}
                <span class="author" title={c.author}>{c.author}</span>
                <button
                    type="button"
                    class="msg"
                    title={row.op === "pick"
                        ? editing[c.id]
                            ? "Close the message editor"
                            : "Write a new message for this commit (and the commits folded into it)"
                        : "Only a kept commit can get a new message"}
                    aria-expanded={!!editing[c.id]}
                    disabled={row.op !== "pick"}
                    onclick={() => (editing[c.id] = !editing[c.id])}
                >
                    Message…
                </button>
                {#if row.op === "pick" && editing[c.id]}
                    <div class="editor">
                        <textarea
                            rows="5"
                            title="The commit's message: a first line, a blank line, then any details"
                            aria-label={`Message for ${c.shortId}`}
                            value={row.message ?? defaultMessage(rows, i)}
                            oninput={(e) =>
                                (rows = setMessage(
                                    rows,
                                    i,
                                    e.currentTarget.value,
                                ))}></textarea>
                        <div class="editor-foot">
                            <span class="muted">
                                {#if row.message === null}
                                    Squashed commits' messages are added below
                                    this one's.
                                {:else}
                                    This replaces the whole message, squashed
                                    commits' included.
                                {/if}
                            </span>
                            <button
                                type="button"
                                title="Go back to the commit's own message, with the squashed commits' messages added"
                                disabled={row.message === null}
                                onclick={() =>
                                    (rows = setMessage(rows, i, null))}
                            >
                                Reset message
                            </button>
                        </div>
                    </div>
                {/if}
            </li>
        {/each}
    </ol>
    <p class="summary" role="status">
        {summary(range, rows)}
    </p>
    {#if problem}
        <p class="problem" role="alert">{problem}</p>
    {/if}
    {#if error}
        <ErrorBox {error} />
    {/if}
    {#snippet actions()}
        <button
            type="button"
            title="Close the editor. Nothing is changed"
            onclick={onclose}
        >
            Cancel
        </button>
        <button
            type="button"
            class="danger"
            title={applyWhy ??
                "See what this changes, then take a backup and rewrite the commits"}
            disabled={applyWhy !== null}
            onclick={apply}
        >
            Apply…
        </button>
    {/snippet}
</Modal>

<style>
    .intro,
    .warn,
    .summary,
    .problem {
        margin: 0;
        font-size: 0.9em;
    }
    .intro,
    .summary,
    .muted {
        color: var(--text-muted);
    }
    .warn,
    .problem {
        color: var(--warning);
    }
    .rows {
        display: grid;
        max-height: min(28rem, 55vh);
        margin: 0;
        padding: 0;
        overflow: auto;
        border: 1px solid var(--border);
        border-radius: 4px;
        list-style: none;
    }
    .row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0.4rem;
        padding: 0.3rem 0.5rem;
        border-bottom: 1px solid var(--border);
        font-size: 0.9rem;
    }
    .row:last-child {
        border-bottom: none;
    }
    .row.folded .subject {
        padding-left: 1rem;
        color: var(--text-muted);
    }
    .row.dropped .sha,
    .row.dropped .subject {
        color: var(--text-muted);
        text-decoration: line-through;
    }
    .row.orphan {
        box-shadow: inset 3px 0 0 var(--warning);
    }
    .move {
        display: inline-flex;
        gap: 0.15rem;
    }
    button,
    select {
        padding: 0.1rem 0.45rem;
        font-size: 0.85rem;
    }
    select {
        /* Room for the arrow drawn in app.css. */
        padding-right: 1.7rem;
    }
    .sha {
        color: var(--text-muted);
    }
    .subject {
        flex: 1 1 12rem;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .into {
        margin-right: 0.3rem;
    }
    .tag {
        padding: 0 0.35rem;
        border-radius: 3px;
        background: var(--surface-2);
        color: var(--text-muted);
        font-size: 0.8em;
    }
    .author {
        max-width: 9rem;
        overflow: hidden;
        color: var(--text-muted);
        font-size: 0.85em;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .editor {
        display: grid;
        flex: 1 0 100%;
        gap: 0.3rem;
    }
    /* As the commit box in ChangesView: app.css leaves textareas to their views. */
    textarea {
        box-sizing: border-box;
        width: 100%;
        padding: 0.4rem 0.5rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--bg);
        color: var(--text);
        resize: vertical;
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-size: 0.85rem;
    }
    .editor-foot {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 0.5rem;
        font-size: 0.85em;
    }
</style>
