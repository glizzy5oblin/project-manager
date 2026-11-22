<script lang="ts">
    import type { ProjectSummary } from "../bindings/ProjectSummary";
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import { offerMenu } from "../contextMenu";
    import { operationLabel, pathTail } from "../format";

    let {
        project,
        onopen,
        onremove,
    }: { project: ProjectSummary; onopen: () => void; onremove: () => void } =
        $props();

    const p = $derived(project);
    const changes = $derived(
        p.staged + p.unstaged + p.untracked + p.conflicted,
    );
    const broken = $derived(p.repoError ?? p.projError);
    let folderError = $state<AppError | null>(null);

    function menu(e: MouseEvent) {
        offerMenu(e, {
            items: [
                {
                    id: "folder",
                    label: "Open folder",
                    keys: "",
                    disabled: false,
                    hint: "Show the project's folder in your file manager",
                },
                {
                    id: "path",
                    label: "Copy path",
                    keys: "",
                    disabled: false,
                    hint: "Copy the project's folder path to the clipboard",
                },
            ],
            run: (id) => {
                if (id === "path") void navigator.clipboard.writeText(p.path);
                else
                    api.openFolder(p.id).then(
                        () => (folderError = null),
                        (err) => (folderError = err as AppError),
                    );
            },
        });
    }
</script>

<article class="card" class:broken oncontextmenu={menu}>
    <button
        title={p.repoError
            ? "This repository has a problem, so it cannot be opened"
            : "Open this project"}
        type="button"
        class="open"
        onclick={onopen}
        disabled={!!p.repoError}
    >
        <span class="title">
            <span class="name">{p.name}</span>
            {#if p.version}<span class="version">v{p.version}</span>{/if}
        </span>

        {#if broken}
            <span class="problem">{broken.message}</span>
        {:else}
            <span class="row">
                <span class="branch" title="Current branch">
                    {p.detached ? "detached HEAD" : (p.branch ?? "no branch")}
                </span>
                {#if p.ahead !== null && p.behind !== null}
                    <span
                        class="sync"
                        title={`${p.ahead} to push, ${p.behind} to pull (${p.upstream})`}
                    >
                        ↑{p.ahead} ↓{p.behind}
                    </span>
                {:else if p.branch}
                    <span class="muted" title="This branch has no upstream">
                        not published
                    </span>
                {/if}
            </span>
            <span class="row">
                {#if p.conflicted}
                    <span class="bad">{p.conflicted} conflicted</span>
                {/if}
                {#if changes}
                    <span
                        class="warn"
                        title={`${p.staged} staged, ${p.unstaged} modified, ${p.untracked} new`}
                    >
                        {changes} changed
                    </span>
                {:else}
                    <span class="good">clean</span>
                {/if}
                {#if p.inProgress}
                    <span class="bad">
                        {operationLabel(p.inProgress)} in progress
                    </span>
                {/if}
            </span>
            <span class="row muted">
                {p.lastRelease
                    ? `Last release ${p.lastRelease}`
                    : "No releases yet"}
                {#if p.mode}· {p.mode === "two-repo"
                        ? "two repos"
                        : "one repo"}{/if}
            </span>
            {#if !p.trusted}
                <span class="warn">
                    Owned by another user: writes will ask first
                </span>
            {/if}
        {/if}
        <span class="path" title={p.path}>{pathTail(p.path)}</span>
    </button>
    <button
        type="button"
        class="remove"
        onclick={onremove}
        title="Remove from hub"
        aria-label={`Remove ${p.name} from hub`}
    >
        ✕
    </button>
    {#if folderError}
        <p class="problem" role="alert">{folderError.message}</p>
    {/if}
</article>

<style>
    .card {
        position: relative;
        min-width: 0;
        border: 1px solid var(--border);
        border-radius: 8px;
        background: var(--surface);
    }
    .card:hover,
    .card:focus-within {
        border-color: var(--accent);
    }
    .card.broken {
        border-style: dashed;
    }
    .open {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: 0.35rem;
        white-space: normal;
        width: 100%;
        padding: 0.9rem 1rem;
        border: none;
        background: none;
        color: inherit;
        font: inherit;
        text-align: left;
        cursor: pointer;
    }
    .open:disabled {
        cursor: default;
    }
    .title {
        display: flex;
        gap: 0.5rem;
        align-items: baseline;
        padding-right: 1.5rem;
    }
    .version {
        flex-shrink: 0;
    }
    .name {
        min-width: 0;
        overflow: hidden;
        font-weight: 600;
        font-size: 1.05rem;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .version,
    .muted,
    .path {
        color: var(--text-muted);
    }
    .row {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
    }
    .branch {
        font-family: ui-monospace, monospace;
        overflow-wrap: anywhere;
    }
    .path {
        overflow: hidden;
        font-size: 0.8em;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .good {
        color: var(--success);
    }
    .warn {
        color: var(--warning);
    }
    .bad,
    .problem {
        color: var(--danger);
    }
    .problem {
        overflow-wrap: anywhere;
    }
    .remove {
        position: absolute;
        top: 0.5rem;
        right: 0.5rem;
        padding: 0.1rem 0.4rem;
        border: none;
        background: none;
        color: var(--text-muted);
        opacity: 0;
    }
    .card:hover .remove,
    .remove:focus-visible {
        opacity: 1;
    }
    .remove:hover {
        color: var(--danger);
    }
</style>
