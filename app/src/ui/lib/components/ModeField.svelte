<script lang="ts">
    import type { WorkflowMode } from "../bindings/WorkflowMode";

    let { value = $bindable("two-repo") }: { value: WorkflowMode } = $props();

    const modes: { id: WorkflowMode; label: string; hint: string }[] = [
        {
            id: "two-repo",
            label: "Two repositories",
            hint: "Private work repo plus a separate public repo.",
        },
        {
            id: "single-repo",
            label: "One repository",
            hint: "Private and public history are branches of the same repo.",
        },
    ];
</script>

<fieldset>
    <legend>Workflow</legend>
    {#each modes as m (m.id)}
        <label>
            <input
                title={m.hint}
                type="radio"
                name="mode"
                value={m.id}
                bind:group={value}
            />
            <span>
                <strong>{m.label}</strong>
                <small>{m.hint}</small>
            </span>
        </label>
    {/each}
</fieldset>

<style>
    fieldset {
        display: grid;
        gap: 0.5rem;
        margin: 0;
        padding: 0;
        border: none;
    }
    legend {
        margin-bottom: 0.3rem;
        color: var(--text-muted);
        font-size: 0.9em;
    }
    label {
        display: flex;
        gap: 0.5rem;
        align-items: flex-start;
    }
    span {
        display: grid;
    }
    small {
        color: var(--text-muted);
    }
</style>
