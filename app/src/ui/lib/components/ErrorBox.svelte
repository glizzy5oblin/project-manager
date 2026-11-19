<script lang="ts">
    import type { AppError } from "../bindings/AppError";

    let { error }: { error: AppError } = $props();
</script>

<div class="error" role="alert">
    <p>{error.message}</p>
    {#if error.issues.length}
        <ul>
            {#each error.issues as issue, i (i)}
                <li><code>{issue.key}</code>: {issue.message}</li>
            {/each}
        </ul>
    {/if}
    {#if error.detail}
        <details>
            <summary>Details</summary>
            <pre>{error.detail}</pre>
        </details>
    {/if}
</div>

<style>
    .error {
        padding: 0.6rem 0.75rem;
        border-left: 3px solid var(--danger);
        border-radius: 4px;
        background: var(--surface-2);
    }
    p {
        margin: 0;
    }
    ul {
        margin: 0.4rem 0 0;
        padding-left: 1.2rem;
    }
    details {
        margin-top: 0.4rem;
        color: var(--text-muted);
    }
    pre {
        max-height: 12rem;
        overflow: auto;
        white-space: pre-wrap;
        word-break: break-word;
        font-size: 0.85em;
    }
</style>
