<script lang="ts">
    // Add an existing repository. When it has no .proj file, offer to write a starter one.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { ProjectEntry } from "../bindings/ProjectEntry";
    import type { WorkflowMode } from "../bindings/WorkflowMode";
    import ErrorBox from "./ErrorBox.svelte";
    import FolderField from "./FolderField.svelte";
    import Modal from "./Modal.svelte";
    import ModeField from "./ModeField.svelte";

    let {
        onclose,
        onadded,
    }: { onclose: () => void; onadded: (p: ProjectEntry) => void } = $props();

    let path = $state("");
    let mode = $state<WorkflowMode>("two-repo");
    let error = $state<AppError | null>(null);
    let busy = $state(false);
    const needsProj = $derived(error?.kind === "noProjFile");

    $effect(() => {
        void path;
        error = null;
    });

    async function add() {
        busy = true;
        try {
            onadded(await api.projectAdd(path, needsProj ? mode : null));
        } catch (e) {
            error = e as AppError;
        } finally {
            busy = false;
        }
    }
</script>

<Modal title="Add existing project" {onclose}>
    <p class="hint">
        Pick the top folder of a git repository that holds a <code>.proj</code> file.
    </p>
    <FolderField
        label="Repository folder"
        title="Choose a repository"
        bind:value={path}
    />
    {#if error}
        <ErrorBox {error} />
    {/if}
    {#if needsProj}
        <p>
            Create a starter <code>.proj</code> file in this folder and add it?
        </p>
        <ModeField bind:value={mode} />
    {/if}
    {#snippet actions()}
        <button type="button" onclick={onclose}>Cancel</button>
        <button
            type="button"
            class="primary"
            disabled={!path || busy}
            onclick={add}
        >
            {needsProj ? "Create .proj and add" : "Add"}
        </button>
    {/snippet}
</Modal>

<style>
    .hint {
        margin: 0;
        color: var(--text-muted);
    }
    p {
        margin: 0;
    }
</style>
