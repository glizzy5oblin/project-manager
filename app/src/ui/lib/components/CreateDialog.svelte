<script lang="ts">
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ProjectEntry } from "../bindings/ProjectEntry";
    import type { WorkflowMode } from "../bindings/WorkflowMode";
    import { projectNameProblem } from "../names";
    import ErrorBox from "./ErrorBox.svelte";
    import FolderField from "./FolderField.svelte";
    import Modal from "./Modal.svelte";
    import ModeField from "./ModeField.svelte";

    let {
        onclose,
        onadded,
    }: { onclose: () => void; onadded: (p: ProjectEntry) => void } = $props();

    let name = $state("");
    let parent = $state("");
    let mode = $state<WorkflowMode>("two-repo");
    let error = $state<AppError | null>(null);
    let busy = $state(false);
    const nameProblem = $derived(name ? projectNameProblem(name) : null);
    // The folders create_project makes, shown before the user commits to them.
    const target = $derived.by(() => {
        if (!name || nameProblem || !parent) return null;
        const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
        const dirs = mode === "two-repo" ? ["private", "public"] : ["private"];
        return dirs.map((d) => name + sep + d).join(" and ");
    });

    async function create() {
        busy = true;
        error = null;
        try {
            onadded(await api.projectCreate(parent, name, mode));
        } catch (e) {
            error = e as AppError;
        } finally {
            busy = false;
        }
    }
</script>

<Modal title="Create new project" {onclose}>
    <label class="field">
        <span>Project name</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
            title="The name of the new project's folder"
            bind:value={name}
            autofocus
            spellcheck="false"
            aria-invalid={!!nameProblem}
        />
        {#if nameProblem}<small class="problem">{nameProblem}</small>{/if}
    </label>
    <FolderField
        label="Create it in"
        title="Choose where to create the project"
        bind:value={parent}
    />
    <ModeField bind:value={mode} />
    {#if target}
        <p class="target">Creates {target} in this folder.</p>
    {/if}
    {#if error}
        <ErrorBox {error} />
    {/if}
    {#snippet actions()}
        <button
            title="Close without creating anything"
            type="button"
            onclick={onclose}
        >
            Cancel
        </button>
        <button
            title="Create the project and add it to your list of projects"
            type="button"
            class="primary"
            disabled={!name || !!nameProblem || !parent || busy}
            onclick={create}
        >
            Create
        </button>
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
    .target {
        margin: 0;
        color: var(--text-muted);
        font-size: 0.9em;
        overflow-wrap: anywhere;
    }
</style>
