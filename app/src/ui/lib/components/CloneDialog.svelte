<script lang="ts">
    // Clone a repository. The clone runs as a background task: closing this dialog does not stop
    // it, and the result is still handled.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { CloneResult } from "../bindings/CloneResult";
    import type { ProjectEntry } from "../bindings/ProjectEntry";
    import type { WorkflowMode } from "../bindings/WorkflowMode";
    import { nameFromUrl, projectNameProblem } from "../names";
    import { tasks } from "../tasks.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import FolderField from "./FolderField.svelte";
    import Modal from "./Modal.svelte";
    import ModeField from "./ModeField.svelte";

    let {
        onclose,
        onadded,
        onfinished,
    }: {
        onclose: () => void;
        onadded: (p: ProjectEntry) => void;
        /** Called when the clone ends, even if the dialog was closed. */
        onfinished: () => void;
    } = $props();

    let url = $state("");
    let parent = $state("");
    let name = $state("");
    let mode = $state<WorkflowMode>("two-repo");
    let taskId = $state<number | null>(null);
    let error = $state<AppError | null>(null);
    /** A finished clone without a usable .proj file. */
    let cloned = $state<CloneResult | null>(null);

    const suggested = $derived(nameFromUrl(url));
    const nameProblem = $derived(name ? projectNameProblem(name) : null);
    const running = $derived(tasks.running.find((t) => t.id === taskId));

    async function start() {
        error = null;
        try {
            taskId = await api.projectClone(url, parent, name.trim() || null);
        } catch (e) {
            error = e as AppError;
            return;
        }
        const outcome = await tasks.wait(taskId);
        taskId = null;
        onfinished();
        if (outcome.status === "ok") {
            const result = outcome.value as CloneResult;
            if (result.project) onadded(result.project);
            else cloned = result;
        } else if (outcome.status === "failed") {
            error = outcome.error;
        }
    }

    async function addWithProj() {
        if (!cloned) return;
        try {
            onadded(await api.projectAdd(cloned.path, mode));
        } catch (e) {
            error = e as AppError;
        }
    }
</script>

<Modal title="Clone project" {onclose}>
    {#if cloned}
        <p>Cloned to <code>{cloned.path}</code>, but it was not added:</p>
        {#if cloned.notAdded}<ErrorBox error={cloned.notAdded} />{/if}
        {#if cloned.notAdded?.kind === "noProjFile"}
            <p>Create a starter <code>.proj</code> file in it and add it?</p>
            <ModeField bind:value={mode} />
        {/if}
        {#if error}<ErrorBox {error} />{/if}
    {:else}
        <label class="field">
            <span>Repository URL</span>
            <!-- svelte-ignore a11y_autofocus -->
            <input
                bind:value={url}
                autofocus
                spellcheck="false"
                placeholder="https://github.com/owner/repo.git"
                disabled={taskId !== null}
            />
        </label>
        <FolderField
            label="Clone into"
            title="Choose where to clone"
            bind:value={parent}
        />
        <label class="field">
            <span>Folder name</span>
            <input
                bind:value={name}
                spellcheck="false"
                placeholder={suggested || "folder name"}
                disabled={taskId !== null}
                aria-invalid={!!nameProblem}
            />
            {#if nameProblem}<small class="problem">{nameProblem}</small>{/if}
        </label>
        {#if taskId !== null}
            <div class="progress">
                <span>{running?.phase || running?.message || "Starting…"}</span>
                <progress max="100" value={running?.percent ?? undefined}
                ></progress>
            </div>
        {/if}
        {#if error}<ErrorBox {error} />{/if}
    {/if}
    {#snippet actions()}
        {#if cloned}
            <button type="button" onclick={onclose}>Close</button>
            {#if cloned.notAdded?.kind === "noProjFile"}
                <button type="button" class="primary" onclick={addWithProj}>
                    Create .proj and add
                </button>
            {/if}
        {:else if taskId !== null}
            <button type="button" onclick={onclose}>Hide</button>
            <button
                type="button"
                onclick={() => taskId !== null && api.taskCancel(taskId)}
            >
                Cancel clone
            </button>
        {:else}
            <button type="button" onclick={onclose}>Cancel</button>
            <button
                type="button"
                class="primary"
                disabled={!url.trim() ||
                    !parent ||
                    !!nameProblem ||
                    (!name && !suggested)}
                onclick={start}
            >
                Clone
            </button>
        {/if}
    {/snippet}
</Modal>

<style>
    .field {
        display: grid;
        gap: 0.3rem;
    }
    .field > span {
        color: var(--text-muted);
        font-size: 0.9em;
    }
    .problem {
        color: var(--danger);
    }
    .progress {
        display: grid;
        gap: 0.3rem;
    }
    progress {
        width: 100%;
    }
    p {
        margin: 0;
    }
    code {
        word-break: break-all;
    }
</style>
