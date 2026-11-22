<script lang="ts">
    // The operation log: every git command and backup the app ran, newest first, with the exact
    // command, result and git's error output. Shows this project's entries, or all of them.
    import { listen } from "@tauri-apps/api/event";
    import { api, toAppError } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { BackupRecord } from "../bindings/BackupRecord";
    import type { ActionOutcome } from "../bindings/ActionOutcome";
    import type { RestorePreview } from "../bindings/RestorePreview";
    import type { OpEntry } from "../bindings/OpEntry";
    import { pathTail } from "../format";
    import { formatTime } from "../history";
    import { previewLines, restoreLabel } from "../restorePoints";
    import { tasks } from "../tasks.svelte";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        root,
        active,
        onreload,
    }: {
        projectId: string;
        /** The project's path, as the log records it. */
        root: string | undefined;
        active: boolean;
        /** Reload the project's state after a restore changed it. */
        onreload: () => Promise<void>;
    } = $props();

    /** Entries kept on screen, like the backend's log (actions/oplog.rs CAPACITY). */
    const KEEP = 500;

    let entries = $state.raw<OpEntry[]>([]);
    let all = $state(false);
    let error = $state<AppError | null>(null);
    let loaded = false;

    // Restore points: what the app saved before something risky, each one a way back.
    let points = $state<BackupRecord[]>([]);
    let choosing = $state<RestorePreview | null>(null);
    let restoring = $state(false);
    let notice = $state("");
    let pointsError = $state<AppError | null>(null);

    async function loadPoints() {
        try {
            points = await api.backupsList(projectId);
            pointsError = null;
        } catch (e) {
            pointsError = toAppError(e);
        }
    }

    async function review(id: string) {
        notice = "";
        pointsError = null;
        try {
            choosing = await api.backupPreview(projectId, id);
        } catch (e) {
            pointsError = toAppError(e);
        }
    }

    async function restore() {
        if (!choosing || restoring) return;
        const id = choosing.backup.id;
        restoring = true;
        pointsError = null;
        try {
            const out = await tasks.wait(
                await api.backupRestore(projectId, id),
            );
            if (out.status === "ok") {
                const o = out.value as ActionOutcome;
                notice = `Restored. A restore point of how things were just before is saved too (${o.backup?.id ?? ""}).`;
                choosing = null;
            } else if (out.status === "failed") {
                pointsError = out.error;
            }
        } catch (e) {
            pointsError = toAppError(e);
        } finally {
            restoring = false;
            await loadPoints();
            await load();
            await onreload();
        }
    }

    async function load() {
        try {
            entries = await api.opLog();
            error = null;
        } catch (e) {
            error = toAppError(e);
        }
    }

    $effect(() => {
        if (active && !loaded) {
            loaded = true;
            void load();
        }
        if (active) void loadPoints();
    });

    $effect(() => {
        const off = listen<OpEntry>("op-log", (e) => {
            if (!loaded) return;
            // A backup was just taken (or restored): the list of restore points changed.
            if (e.payload.kind === "backup" && active) void loadPoints();
            // Already in a list loaded while the event was on its way.
            const last = entries.at(-1);
            if (last && e.payload.seq <= last.seq) return;
            const next = [...entries, e.payload];
            entries = next.length > KEEP ? next.slice(-KEEP) : next;
        });
        return () => void off.then((f) => f());
    });

    async function clear() {
        try {
            await api.opLogClear();
            entries = [];
        } catch (e) {
            error = toAppError(e);
        }
    }

    const shown = $derived(
        entries.filter((e) => all || e.repo === root).reverse(),
    );
    const time = (ms: number) =>
        new Date(ms).toLocaleTimeString(undefined, {
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
        });
    const duration = (ms: number) =>
        ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
</script>

<div class="activity">
    <header>
        <h2>Activity</h2>
        <label>
            <input
                title="Also show what was done in your other projects"
                type="checkbox"
                bind:checked={all}
            />
            All projects
        </label>
        <span class="spacer"></span>
        <button title="Reload the list" type="button" onclick={load}>
            Refresh
        </button>
        <button
            title="Empty this list. Your projects and backups are not touched"
            type="button"
            onclick={clear}
            disabled={!entries.length}
        >
            Clear log
        </button>
    </header>
    <section class="points" aria-label="Restore points">
        <h3>Undo: restore points</h3>
        <p class="muted">
            Before it stashes, discards, resets or deletes something, Project
            Manager saves how your project looked. Pick one to put everything
            back, including your files. Restoring saves a new restore point
            first, so you can undo the undo.
        </p>
        {#if pointsError}
            <ErrorBox error={pointsError} />
        {/if}
        {#if notice}
            <p class="notice" role="status">{notice}</p>
        {/if}
        {#if choosing}
            <div class="review" role="group" aria-label="Restore this point">
                <strong>
                    Go back to before {restoreLabel(
                        choosing.backup.action,
                    ).toLowerCase()}
                    ({formatTime(choosing.backup.created)})?
                </strong>
                <ul>
                    {#each previewLines(choosing) as line (line)}
                        <li>{line}</li>
                    {/each}
                </ul>
                <div class="buttons">
                    <button
                        title="Put the project back as it was at this restore point"
                        type="button"
                        class="primary"
                        disabled={restoring}
                        onclick={restore}
                    >
                        {restoring ? "Restoring…" : "Restore"}
                    </button>
                    <button
                        title="Close this without changing anything"
                        type="button"
                        disabled={restoring}
                        onclick={() => (choosing = null)}
                    >
                        Cancel
                    </button>
                </div>
            </div>
        {/if}
        {#if points.length}
            <ul class="point-list">
                {#each points as b (b.id)}
                    <li>
                        <span class="what">{restoreLabel(b.action)}</span>
                        <span class="when">{formatTime(b.created)}</span>
                        <span class="muted">
                            {b.headBranch ? `on ${b.headBranch}` : "detached"}
                        </span>
                        <span class="spacer"></span>
                        <button
                            title="See what going back to this point would change, then confirm"
                            type="button"
                            disabled={restoring}
                            onclick={() => review(b.id)}
                        >
                            Restore…
                        </button>
                    </li>
                {/each}
            </ul>
        {:else}
            <p class="muted">
                No restore points yet. One appears here whenever something risky
                is about to happen.
            </p>
        {/if}
    </section>
    <h3>Log</h3>
    <p class="muted">
        Every git command and backup Project Manager ran in this session, newest
        first.
    </p>
    {#if error}
        <ErrorBox {error} />
    {/if}
    {#if shown.length}
        <ol>
            {#each shown as e (e.seq)}
                <li class:failed={!e.ok}>
                    <div class="head">
                        <span
                            class="status"
                            title={e.ok ? "Succeeded" : "Failed"}
                        >
                            {e.ok ? "✓" : "✗"}
                        </span>
                        <span class="when">{time(e.time)}</span>
                        <span class="action">{e.action}</span>
                        {#if all}
                            <span class="repo" title={e.repo}>
                                {pathTail(e.repo, 2)}
                            </span>
                        {/if}
                        <span class="spacer"></span>
                        <span class="meta">
                            {#if e.kind === "git"}
                                exit {e.code ?? "—"} ·
                            {/if}
                            {duration(e.durationMs)}
                        </span>
                    </div>
                    <code class="cmd">{e.command}</code>
                    {#if e.stderr}
                        <details open={!e.ok}>
                            <summary title="Show what git printed">
                                {e.ok ? "Output" : "Error"}
                            </summary>
                            <pre>{e.stderr}</pre>
                        </details>
                    {/if}
                </li>
            {/each}
        </ol>
    {:else}
        <p class="muted">Nothing has run yet.</p>
    {/if}
</div>

<style>
    .activity {
        display: grid;
        gap: 0.75rem;
        align-content: start;
    }
    header {
        display: flex;
        align-items: center;
        gap: 0.75rem;
    }
    h2 {
        margin: 0;
        font-size: 1.2rem;
    }
    header label {
        display: flex;
        align-items: center;
        gap: 0.35rem;
        color: var(--text-muted);
    }
    .spacer {
        flex: 1;
    }
    h3 {
        margin: 0;
        font-size: 1rem;
    }
    .points {
        display: grid;
        gap: 0.5rem;
    }
    .point-list {
        display: grid;
        gap: 0.3rem;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .point-list li {
        display: flex;
        align-items: center;
        gap: 0.6rem;
        border-left-color: var(--accent);
    }
    .what {
        font-weight: 600;
    }
    .review {
        display: grid;
        gap: 0.4rem;
        padding: 0.6rem 0.75rem;
        border: 1px solid var(--warning);
        border-radius: 4px;
        background: var(--surface);
    }
    .review ul {
        display: grid;
        gap: 0.2rem;
        margin: 0;
        padding-left: 1.2rem;
    }
    .review li {
        padding: 0;
        border-left: none;
        background: none;
    }
    .buttons {
        display: flex;
        gap: 0.4rem;
    }
    .notice {
        color: var(--success);
    }
    p {
        margin: 0;
    }
    .muted {
        color: var(--text-muted);
    }
    ol {
        display: grid;
        gap: 0.4rem;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: grid;
        gap: 0.3rem;
        padding: 0.45rem 0.6rem;
        border-left: 3px solid var(--success);
        border-radius: 4px;
        background: var(--surface);
    }
    li.failed {
        border-left-color: var(--danger);
    }
    .head {
        display: flex;
        align-items: baseline;
        gap: 0.6rem;
        min-width: 0;
    }
    .status {
        color: var(--success);
        font-weight: 700;
    }
    .failed .status {
        color: var(--danger);
    }
    .when,
    .meta,
    .repo {
        color: var(--text-muted);
        font-size: 0.85em;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .repo {
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .action {
        font-weight: 600;
    }
    .cmd {
        font-size: 0.85em;
        overflow-wrap: anywhere;
    }
    details {
        color: var(--text-muted);
        font-size: 0.9em;
    }
    pre {
        max-height: 12rem;
        margin: 0.3rem 0 0;
        overflow: auto;
        color: var(--text);
        white-space: pre-wrap;
        word-break: break-word;
    }
</style>
