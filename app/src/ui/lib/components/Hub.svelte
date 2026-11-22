<script lang="ts">
    import { api } from "../api";
    import type { Account } from "../bindings/Account";
    import type { AppError } from "../bindings/AppError";
    import type { ProjectEntry } from "../bindings/ProjectEntry";
    import type { ProjectSummary } from "../bindings/ProjectSummary";
    import type { Theme } from "../bindings/Theme";
    import type { ThemeProblem } from "../bindings/ThemeProblem";
    import AccountDialog from "./AccountDialog.svelte";
    import AddDialog from "./AddDialog.svelte";
    import CloneDialog from "./CloneDialog.svelte";
    import CreateDialog from "./CreateDialog.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import Modal from "./Modal.svelte";
    import ProjectCard from "./ProjectCard.svelte";

    let {
        projects,
        loading,
        error,
        themes,
        themeId,
        themeProblems,
        onopen,
        onadded,
        onremove,
        onrefresh,
        ontheme,
    }: {
        projects: ProjectSummary[];
        loading: boolean;
        error: AppError | null;
        themes: Theme[];
        themeId: string;
        themeProblems: ThemeProblem[];
        onopen: (id: string) => void;
        onadded: (p: ProjectEntry) => void;
        onremove: (id: string) => void;
        onrefresh: () => void;
        ontheme: (id: string) => void;
    } = $props();

    let dialog = $state<"add" | "clone" | "create" | "account" | null>(null);
    let account = $state<Account | null>(null);

    // Who is signed in, checked with GitHub once at start. Offline, the button just reads
    // "GitHub account" until the dialog is opened.
    api.githubStatus(true, false).then(
        (s) => (account = s.account),
        () => {},
    );
    let removing = $state<ProjectSummary | null>(null);

    function added(p: ProjectEntry) {
        dialog = null;
        onadded(p);
    }
</script>

<section class="hub" aria-label="Projects">
    <header>
        <h1>Projects</h1>
        <div class="tools">
            <button
                title="Add a project folder that is already on your computer"
                type="button"
                class="primary"
                onclick={() => (dialog = "add")}
            >
                Add existing
            </button>
            <button
                title="Copy a repository from the internet, such as GitHub, to your computer"
                type="button"
                onclick={() => (dialog = "clone")}
            >
                Clone
            </button>
            <button
                title="Start a brand-new project in a new folder"
                type="button"
                onclick={() => (dialog = "create")}
            >
                Create new
            </button>
            <button
                type="button"
                onclick={onrefresh}
                disabled={loading}
                title="Re-read every project"
            >
                Refresh
            </button>
            <button
                type="button"
                class="account"
                onclick={() => (dialog = "account")}
                title="GitHub account"
            >
                {account ? `GitHub: ${account.login}` : "Sign in to GitHub"}
            </button>
            <label class="theme">
                <span>Theme</span>
                <select
                    title="Change the colours of the app"
                    value={themeId}
                    onchange={(e) => ontheme(e.currentTarget.value)}
                >
                    {#each themes as t (t.id)}
                        <option value={t.id}>{t.name}</option>
                    {/each}
                </select>
            </label>
        </div>
    </header>

    {#if error}
        <ErrorBox {error} />
    {/if}

    {#if themeProblems.length}
        <details class="theme-problems">
            <summary title="Theme files that could not be loaded">
                {themeProblems.length === 1
                    ? "1 theme file could not be loaded"
                    : `${themeProblems.length} theme files could not be loaded`}
            </summary>
            <ul>
                {#each themeProblems as p (p.file)}
                    <li><code>{p.file}</code>: {p.message}</li>
                {/each}
            </ul>
        </details>
    {/if}

    {#if projects.length}
        <div class="grid" aria-busy={loading}>
            {#each projects as p (p.id)}
                <ProjectCard
                    project={p}
                    onopen={() => onopen(p.id)}
                    onremove={() => (removing = p)}
                />
            {/each}
        </div>
    {:else if loading}
        <p class="muted" role="status">Loading projects…</p>
    {:else}
        <div class="empty">
            <p>No projects yet.</p>
            <p class="muted">
                Add a repository that has a <code>.proj</code> file, clone one, or
                create a new project.
            </p>
        </div>
    {/if}
</section>

{#if dialog === "add"}
    <AddDialog onclose={() => (dialog = null)} onadded={added} />
{:else if dialog === "clone"}
    <CloneDialog
        onclose={() => (dialog = null)}
        onadded={added}
        onfinished={onrefresh}
    />
{:else if dialog === "create"}
    <CreateDialog onclose={() => (dialog = null)} onadded={added} />
{:else if dialog === "account"}
    <AccountDialog
        onclose={() => (dialog = null)}
        onchange={(a) => (account = a)}
    />
{/if}

{#if removing}
    {@const r = removing}
    <Modal title={`Remove ${r.name}?`} onclose={() => (removing = null)}>
        <p>
            The project is removed from the hub only. Its folder and files are
            not touched, and it can be added again later.
        </p>
        {#snippet actions()}
            <button
                title="Keep this project in the hub"
                type="button"
                onclick={() => (removing = null)}
            >
                Cancel
            </button>
            <button
                title="Remove this project from the hub. Its folder and files are not deleted"
                type="button"
                class="danger"
                onclick={() => {
                    onremove(r.id);
                    removing = null;
                }}
            >
                Remove from hub
            </button>
        {/snippet}
    </Modal>
{/if}

<style>
    .hub {
        display: grid;
        gap: 1rem;
        align-content: start;
        max-width: 72rem;
        margin: 0 auto;
        padding: 1.25rem;
    }
    header {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
        align-items: center;
        justify-content: space-between;
    }
    h1 {
        margin: 0;
        font-size: 1.4rem;
    }
    .tools {
        display: flex;
        flex-wrap: wrap;
        gap: 0.5rem;
        align-items: center;
    }
    .account {
        margin-left: 0.5rem;
    }
    .theme {
        display: flex;
        gap: 0.4rem;
        align-items: center;
        margin-left: 0.5rem;
        color: var(--text-muted);
    }
    .theme-problems {
        margin-bottom: 0.75rem;
        color: var(--warning);
    }
    .theme-problems ul {
        margin: 0.4rem 0 0;
        color: var(--text-muted);
        overflow-wrap: anywhere;
    }
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr));
        gap: 0.75rem;
    }
    .empty {
        padding: 3rem 1rem;
        text-align: center;
    }
    .muted {
        color: var(--text-muted);
    }
</style>
