<script lang="ts">
    // Private → public staging: the preview (what would move, what blocks it), a one-time allow
    // per blocked file, an explicit per-file override, the sync (a task), a verify and the recent
    // records. Everything is driven from the project's live files, never cached in the component.
    import { api, toAppError } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ChangedFile } from "../bindings/ChangedFile";
    import type { StagePreview } from "../bindings/StagePreview";
    import type { StagingRecord } from "../bindings/StagingRecord";
    import type { VerifyReport } from "../bindings/VerifyReport";
    import { tasks } from "../tasks.svelte";
    import DiffView from "./DiffView.svelte";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        changed,
        active,
    }: {
        projectId: string;
        /** Bumped when the project's files change on disk. */
        changed: number;
        active: boolean;
    } = $props();

    let preview = $state<StagePreview | null>(null);
    let records = $state<StagingRecord[]>([]);
    let report = $state<VerifyReport | null>(null);
    let error = $state<AppError | null>(null);
    let loading = $state(false);
    let syncing = $state(false);
    let syncNote = $state("");
    let verifying = $state(false);
    /** Files allowed by the next sync despite a blocking finding. */
    let allow = $state<string[]>([]);
    let message = $state("");
    let push = $state(false);
    let loadedFor = -1;
    /** The file whose diff is expanded in the preview list. */
    let expanded = $state<string | null>(null);

    const statusLabel: Record<string, string> = {
        added: "new",
        changed: "changed",
        removed: "deleted",
        unchanged: "kept",
    };
    const statusIcon: Record<string, string> = {
        added: "+",
        changed: "~",
        removed: "−",
        unchanged: "·",
    };

    function bytes(n: bigint | number): string {
        const v = Number(n);
        if (v < 1024) return `${v} B`;
        if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KB`;
        return `${(v / (1024 * 1024)).toFixed(1)} MB`;
    }

    function when(seconds: bigint | number): string {
        const d = new Date(Number(seconds) * 1000);
        return d.toLocaleString(undefined, {
            month: "short",
            day: "numeric",
            hour: "2-digit",
            minute: "2-digit",
        });
    }

    async function load() {
        loading = true;
        try {
            [preview, records] = await Promise.all([
                api.stagingPreview(projectId),
                api.stagingRecords(projectId),
            ]);
            report = null;
            error = null;
        } catch (e) {
            error = toAppError(e);
        } finally {
            loading = false;
        }
    }

    async function verify() {
        verifying = true;
        try {
            report = await api.stagingVerify(projectId);
            error = null;
        } catch (e) {
            error = toAppError(e);
        } finally {
            verifying = false;
        }
    }

    async function sync() {
        if (syncing || loading) return;
        syncing = true;
        syncNote = "";
        try {
            const id = await api.stagingSync(
                projectId,
                allow,
                message.trim() ? message.trim() : null,
                push,
            );
            const out = await tasks.wait(id);
            if (out.status === "ok") {
                const o = out.value as { title?: string };
                syncNote = o?.title ?? "Synced.";
                allow = [];
                message = "";
            } else if (out.status === "failed") {
                error = out.error;
                syncNote = "";
            }
        } catch (e) {
            error = toAppError(e);
        } finally {
            syncing = false;
            await load();
        }
    }

    async function setOverride(path: string, kind: string) {
        try {
            await api.stagingSetOverride(projectId, path, kind);
            if (kind === "clear") allow = allow.filter((p) => p !== path);
            await load();
        } catch (e) {
            error = toAppError(e);
        }
    }

    $effect(() => {
        if (active && loadedFor !== changed) {
            loadedFor = changed;
            void load();
        }
    });

    const shown = $derived(
        preview?.files.filter((f) => f.status !== "unchanged") ?? [],
    );

    /** The display object DiffView needs for a staging file. */
    function stagingFile(path: string): ChangedFile {
        return {
            path,
            oldPath: null,
            status: "modified",
            additions: null,
            deletions: null,
            binary: false,
            similarity: null,
        };
    }
</script>

<div class="staging">
    <header>
        <h2>Staging</h2>
        <span class="spacer"></span>
        <button
            type="button"
            title="Reload the preview and the records"
            onclick={load}
            disabled={loading}
        >
            {loading ? "Loading…" : "Refresh"}
        </button>
        <button
            type="button"
            title="Check that the public side matches the private side"
            onclick={verify}
            disabled={verifying || loading}
        >
            {verifying ? "Verifying…" : "Verify"}
        </button>
    </header>

    {#if error}
        <ErrorBox {error} />
    {/if}

    {#if report}
        <p class:ok={report.ok} class:bad={!report.ok} role="status">
            {report.ok
                ? "Public side matches the private side."
                : "Public side differs:"}
        </p>
        {#if !report.ok}
            <ul class="verify-issues">
                {#each report.issues as issue (issue)}
                    <li>{issue}</li>
                {/each}
            </ul>
        {/if}
    {/if}

    <section class="preview" aria-label="What a sync would move">
        <h3>
            What would move
            {#if preview}
                <span class="muted">
                    {shown.length} file{shown.length === 1 ? "" : "s"},
                    {preview.blockers} blocked
                    {#if preview.truncated}· preview truncated{/if}
                </span>
            {/if}
        </h3>
        {#if shown.length === 0 && !loading}
            <p class="muted">
                Nothing to move: the public side already matches the private
                side.
            </p>
        {/if}
        {#each shown as f (f.path)}
            <div class="file" class:blocking={f.issues.some((i) => i.blocking)}>
                <div class="row">
                    <span
                        class="status"
                        title={statusLabel[f.status] ?? f.status}
                        aria-label={statusLabel[f.status] ?? f.status}
                    >
                        {statusIcon[f.status] ?? "?"}
                    </span>
                    <span class="path" title={f.path}>{f.path}</span>
                    <span class="meta">
                        {bytes(f.bytes)}{f.lfs ? " · LFS" : ""}
                    </span>
                    {#if f.issues.some((i) => i.blocking)}
                        <button
                            type="button"
                            title="Allow this file to stage despite the finding"
                            onclick={() => {
                                if (!allow.includes(f.path))
                                    allow = [...allow, f.path];
                            }}
                            disabled={allow.includes(f.path)}
                        >
                            {allow.includes(f.path) ? "Allowed" : "Allow"}
                        </button>
                    {/if}
                    <button
                        type="button"
                        title="Always include this file, even when the default excludes it"
                        onclick={() => setOverride(f.path, "include")}
                    >
                        +
                    </button>
                    <button
                        type="button"
                        title="Never include this file"
                        onclick={() => setOverride(f.path, "exclude")}
                    >
                        −
                    </button>
                    {#if f.path.endsWith(".md")}
                        <button
                            type="button"
                            title="Clear the per-file override"
                            onclick={() => setOverride(f.path, "clear")}
                        >
                            ✕
                        </button>
                    {/if}
                    <button
                        type="button"
                        title="Show or hide the diff for this file"
                        class:on={expanded === f.path}
                        aria-pressed={expanded === f.path}
                        onclick={() =>
                            (expanded = expanded === f.path ? null : f.path)}
                    >
                        Diff
                    </button>
                </div>
                {#each f.issues as issue (issue.kind + issue.detail)}
                    <div class:blocking={issue.blocking} class="finding">
                        <span class="tag">
                            {issue.blocking ? "Blocked" : "Warning"}
                        </span>
                        <span class="kind">{issue.kind}</span>
                        <span class="detail">{issue.detail}</span>
                    </div>
                {/each}
                {#if expanded === f.path}
                    <div class="diff-panel">
                        <DiffView
                            {projectId}
                            stagingPath={f.path}
                            file={stagingFile(f.path)}
                        />
                    </div>
                {/if}
            </div>
        {/each}
    </section>

    <section class="run" aria-label="Sync">
        <h3>Sync</h3>
        <p class="muted">
            Copies the private side into the public side. Files blocked by a
            finding (API key, .env, private key, decompiled code, oversize) stay
            out until you allow them; an allow is remembered for this run only —
            use + or − to change the rule permanently.
        </p>
        <div class="controls">
            <input
                class="message"
                type="text"
                title="What to put in the commit (a message based on the changes by default)"
                placeholder="Commit message (generated by default)"
                bind:value={message}
                aria-label="Commit message"
            />
            <label class="push" title="Push the public side after the commit">
                <input
                    type="checkbox"
                    title="Push the public side after the commit"
                    bind:checked={push}
                />
                Push
            </label>
            <button
                type="button"
                class="primary"
                title="Move the allowlisted files to the public side"
                onclick={sync}
                disabled={syncing || loading || shown.length === 0}
            >
                {syncing
                    ? "Syncing…"
                    : `Sync ${shown.length} file${shown.length === 1 ? "" : "s"}`}
            </button>
        </div>
        {#if syncNote}
            <p class="notice" role="status">{syncNote}</p>
        {/if}
    </section>

    <section class="records" aria-label="Recent staging records">
        <h3>Recent records</h3>
        {#if records.length === 0}
            <p class="muted">No syncs yet.</p>
        {:else}
            <ul>
                {#each [...records].reverse() as r (r.id)}
                    <li>
                        <span class="msg" title={r.message}>{r.message}</span>
                        <span class="when">{when(r.at)}</span>
                        <span class="muted">
                            {r.files.length} file{r.files.length === 1
                                ? ""
                                : "s"}
                        </span>
                        {#if r.allowed.length}
                            <span class="muted">
                                allowed: {r.allowed.length}
                            </span>
                        {/if}
                        {#if r.commit}
                            <code class="head" title={r.commit}>
                                {r.commit.slice(0, 10)}
                            </code>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}
    </section>
</div>

<style>
    .staging {
        display: grid;
        gap: 0.85rem;
        align-content: start;
        min-width: 0;
    }
    header {
        display: flex;
        align-items: center;
        gap: 0.5rem;
    }
    h2,
    h3 {
        margin: 0;
        font-size: 1rem;
    }
    h2 {
        font-size: 1.2rem;
    }
    .spacer {
        flex: 1;
    }
    section {
        display: grid;
        gap: 0.45rem;
    }
    .file {
        display: grid;
        gap: 0.2rem;
        padding: 0.45rem 0.55rem;
        border-left: 3px solid var(--border);
        border-radius: 4px;
        background: var(--surface);
        min-width: 0;
    }
    .file.blocking {
        border-left-color: var(--warning);
    }
    .row {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        min-width: 0;
    }
    .status {
        flex: none;
        width: 1.1rem;
        text-align: center;
        color: var(--text-muted);
        font-weight: 700;
    }
    .path {
        flex: 1 1 auto;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-family: var(--mono, monospace);
        font-size: 0.85em;
    }
    .meta {
        flex: none;
        color: var(--text-muted);
        font-size: 0.8em;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .finding {
        display: flex;
        gap: 0.4rem;
        padding-left: 1.6rem;
        color: var(--text-muted);
        font-size: 0.8em;
    }
    .finding.blocking {
        color: var(--warning);
    }
    .tag {
        flex: none;
        text-transform: uppercase;
        font-size: 0.7em;
        font-weight: 700;
        letter-spacing: 0.04em;
        opacity: 0.9;
    }
    .kind {
        flex: none;
        font-weight: 600;
    }
    .detail {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .row button.on {
        border-color: var(--text);
        color: var(--text);
    }
    .diff-panel {
        margin-top: 0.2rem;
        border-top: 1px solid var(--border);
        padding-top: 0.35rem;
        min-width: 0;
    }
    /* DiffView sizes itself to its panel; here it must not stretch to a fixed height. */
    .diff-panel :global(.diff) {
        height: auto;
        border: none;
        border-radius: 0;
        background: transparent;
    }
    .run {
        display: grid;
        gap: 0.4rem;
    }
    .controls {
        display: flex;
        gap: 0.45rem;
        align-items: center;
    }
    input.message {
        flex: 1 1 auto;
        min-width: 0;
        padding: 0.4rem 0.55rem;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--surface-2);
        color: var(--text);
        font: inherit;
    }
    .push {
        display: inline-flex;
        align-items: center;
        gap: 0.3rem;
        color: var(--text-muted);
        white-space: nowrap;
    }
    .notice {
        color: var(--success);
        margin: 0;
    }
    .verify-issues {
        margin: 0;
        padding-left: 1.2rem;
        color: var(--danger);
    }
    p.ok {
        color: var(--success);
        margin: 0;
    }
    p.bad {
        color: var(--danger);
        margin: 0;
    }
    .records ul {
        display: grid;
        gap: 0.25rem;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .records li {
        display: flex;
        align-items: baseline;
        gap: 0.6rem;
        padding: 0.3rem 0.5rem;
        border-radius: 4px;
        background: var(--surface);
        min-width: 0;
    }
    .records .msg {
        flex: 1 1 auto;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 0.9em;
    }
    .records .when,
    .records .head {
        color: var(--text-muted);
        font-size: 0.8em;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .muted {
        color: var(--text-muted);
        font-size: 0.85em;
    }
    p {
        margin: 0;
    }
</style>
