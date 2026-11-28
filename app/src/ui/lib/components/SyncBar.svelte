<script lang="ts">
    // The branch you are on against the remote branch it tracks, with fetch, pull, push and
    // force push, and pushing a tag. Every button stays visible; one that cannot be used now is
    // greyed out with the reason on hover (lib/remote.ts). Pull with rebase and force push are
    // confirmed and backed up by the action runner.
    import type { Action } from "../bindings/Action";
    import type { RepoState } from "../bindings/RepoState";
    import { aheadBehind, pushTagBlocked, syncInfo } from "../remote";

    let {
        repo,
        busy,
        act,
    }: {
        repo: RepoState;
        busy: boolean;
        act: (action: Action, done?: string) => Promise<boolean>;
    } = $props();

    const info = $derived(syncInfo(repo, busy));
    let tag = $state("");
    let tagRemote = $state("");
    const remote = $derived(tagRemote || info.pushRemote || "");
    const tagWhy = $derived(pushTagBlocked(repo, tag, remote || null, busy));
</script>

{#if repo.remotes.length || info.branch}
    <div class="sync" aria-label="Sync with the remote">
        <span class="where">
            {#if info.branch}
                <code>{info.branch}</code>
                {#if info.upstream}
                    → <code>{info.upstream}</code>
                    <span class="state" title="As of the last fetch">
                        {aheadBehind(info.ahead, info.behind)}
                    </span>
                {:else}
                    <span class="state">
                        {repo.remotes.length
                            ? "not on a remote yet"
                            : "no remote"}
                    </span>
                {/if}
            {:else}
                <span class="state">not on a branch</span>
            {/if}
        </span>
        {#each info.buttons as b (b.id)}
            <button
                type="button"
                class:danger={b.danger}
                title={b.why ?? b.hint}
                disabled={b.why !== null}
                onclick={() => act(b.action)}
            >
                {b.label}
            </button>
        {/each}
        {#if repo.tags.length && repo.remotes.length}
            <span class="tag">
                <select title="The tag to send to the remote" bind:value={tag}>
                    <option value="">Tag…</option>
                    {#each repo.tags as t (t.name)}
                        <option value={t.name}>{t.name}</option>
                    {/each}
                </select>
                {#if repo.remotes.length > 1}
                    <select
                        title="The remote to send the tag to"
                        bind:value={tagRemote}
                    >
                        <option value="">
                            {info.pushRemote ?? "Remote…"}
                        </option>
                        {#each repo.remotes as r (r.name)}
                            <option value={r.name}>{r.name}</option>
                        {/each}
                    </select>
                {/if}
                <button
                    type="button"
                    title={tagWhy ??
                        `Send the tag ${tag} to ${remote}. A tag the remote already has with another commit is refused`}
                    disabled={tagWhy !== null}
                    onclick={() =>
                        act({ type: "pushTag", tag, remote }, `Pushed ${tag}`)}
                >
                    Push tag
                </button>
            </span>
        {/if}
    </div>
{/if}

<style>
    .sync {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0.4rem;
        padding: 0.4rem 0.6rem;
        border-radius: 4px;
        background: var(--surface-2);
        font-size: 0.9em;
    }
    .where {
        flex: 1 1 12rem;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .state {
        color: var(--text-muted);
        margin-left: 0.3rem;
    }
    .tag {
        display: inline-flex;
        gap: 0.3rem;
    }
    /* A risky button here is a plain button with red text. The global button.danger fill is
       for the main button of a dialog; with red text on top of it the label disappeared. */
    button.danger {
        border-color: var(--border);
        background: var(--surface-2);
        color: var(--text);
    }
    button.danger:not(:disabled) {
        color: var(--danger);
    }
    button.danger:hover:not(:disabled) {
        border-color: var(--text-muted);
    }
</style>
