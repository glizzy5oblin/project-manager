<script lang="ts">
    import { api } from "../api";
    import { tasks } from "../tasks.svelte";
</script>

{#if tasks.running.length}
    <footer class="tasks" aria-label="Background tasks" aria-live="polite">
        {#each tasks.running as t (t.id)}
            <div class="task">
                <span class="title">{t.title}</span>
                <span class="phase">{t.phase || t.message}</span>
                <progress max="100" value={t.percent ?? undefined}></progress>
                <button
                    title="Stop this task"
                    type="button"
                    onclick={() => api.taskCancel(t.id)}
                    aria-label={`Cancel ${t.title}`}
                >
                    Cancel
                </button>
            </div>
        {/each}
    </footer>
{/if}

<style>
    .tasks {
        display: grid;
        gap: 0.25rem;
        padding: 0.4rem 0.75rem;
        border-top: 1px solid var(--border);
        background: var(--surface);
        font-size: 0.9em;
    }
    .task {
        display: grid;
        grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 8rem auto;
        gap: 0.75rem;
        align-items: center;
    }
    .title,
    .phase {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .phase {
        color: var(--text-muted);
    }
    progress {
        width: 100%;
    }
</style>
