<script lang="ts">
    import { open, save } from "@tauri-apps/plugin-dialog";
    import { openUrl } from "@tauri-apps/plugin-opener";
    import { onDestroy, onMount } from "svelte";
    import { api, toAppError } from "../api";
    import type { Account } from "../bindings/Account";
    import type { AppError } from "../bindings/AppError";
    import type { Diagnostics } from "../bindings/Diagnostics";
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
    import PreferencesDialog from "./PreferencesDialog.svelte";
    import ProjectCard from "./ProjectCard.svelte";

    let {
        projects,
        loading,
        error,
        themes,
        themeId,
        backups,
        themeProblems,
        onopen,
        onadded,
        onremove,
        onrefresh,
        ontheme,
        onbackups,
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
        backups: boolean;
        onbackups: (backups: boolean) => void;
    } = $props();

    let dialog = $state<
        "add" | "clone" | "create" | "account" | "preferences" | null
    >(null);
    let account = $state<Account | null>(null);

    // Who is signed in, checked with GitHub once at start. Offline, the button just reads
    // "GitHub account" until the dialog is opened.
    api.githubStatus(true, false).then(
        (s) => (account = s.account),
        () => {},
    );
    // Ctrl/Cmd+, toggles the preferences dialog, but only when nothing else is open.
    onMount(() => {
        function keydown(e: KeyboardEvent) {
            if ((e.ctrlKey || e.metaKey) && e.key === ",") {
                if (dialog === null || dialog === "preferences") {
                    e.preventDefault();
                    dialog = dialog === "preferences" ? null : "preferences";
                }
            }
        }
        // The command palette asks the hub to open preferences.
        function openPreferences() {
            if (dialog === null) dialog = "preferences";
        }
        window.addEventListener("keydown", keydown);
        window.addEventListener("pm:open-preferences", openPreferences);
        return () => {
            window.removeEventListener("keydown", keydown);
            window.removeEventListener("pm:open-preferences", openPreferences);
        };
    });

    let removing = $state<ProjectSummary | null>(null);
    let diagnostics = $state<Diagnostics | null>(null);
    let copied = $state(false);
    let saveNote = $state("");
    let saveError = $state<AppError | null>(null);
    let backupNote = $state("");
    let backupError = $state<AppError | null>(null);
    let restoreArmed = $state(false);

    // Disarm the pending restore if the hub unmounts (navigated away) mid-confirm.
    onDestroy(() => {
        restoreArmed = false;
    });

    // Diagnostics for the collapsible card: version, platform, counts and the log tail.
    // Never blocks the hub; a failure just leaves the card empty.
    api.diagnostics().then(
        (d) => (diagnostics = d),
        () => {},
    );

    async function copyDiagnostics() {
        if (!diagnostics) return;
        try {
            await navigator.clipboard.writeText(diagnostics.text);
            copied = true;
            setTimeout(() => (copied = false), 1500);
        } catch {
            // clipboard unavailable: the tail is still readable in the card
        }
    }

    async function backupAppData() {
        restoreArmed = false;
        backupError = null;
        try {
            const path = await save({
                defaultPath: "project-manager-data.zip",
                title: "Back up app data",
                filters: [{ name: "Zip archive", extensions: ["zip"] }],
            });
            if (path) {
                await api.appDataExport(path);
                backupNote = "Backed up";
                setTimeout(() => (backupNote = ""), 1500);
            }
        } catch (e) {
            backupError = toAppError(e);
        }
    }

    async function restoreAppData() {
        if (!restoreArmed) {
            restoreArmed = true;
            return;
        }
        restoreArmed = false;
        backupError = null;
        try {
            const path = await open({
                title: "Restore app data",
                filters: [{ name: "Zip archive", extensions: ["zip"] }],
            });
            if (path) {
                const restored = await api.appDataRestore(path);
                backupNote = `Restored ${restored.length} file${restored.length === 1 ? "" : "s"} — restart the app to apply`;
                setTimeout(() => (backupNote = ""), 4000);
            }
        } catch (e) {
            backupError = toAppError(e);
        }
    }

    async function saveDiagnostics() {
        restoreArmed = false;
        if (!diagnostics) return;
        try {
            const path = await save({
                defaultPath: "project-manager-diagnostics.txt",
                title: "Save diagnostics",
                filters: [{ name: "Text", extensions: ["txt"] }],
            });
            if (path) {
                await api.diagnosticsSave(path);
                saveNote = "Saved";
                setTimeout(() => (saveNote = ""), 1500);
            }
        } catch (e) {
            saveError = toAppError(e);
        }
    }

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
            <button
                title="Open the project's documentation in your browser"
                type="button"
                onclick={() =>
                    void openUrl(
                        "https://github.com/glizzy5oblin/project-manager/tree/main/docs",
                    )}
            >
                Guide
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
            <button
                type="button"
                class="account"
                onclick={() => (dialog = "preferences")}
                title="Open preferences (Ctrl+,)"
            >
                Preferences
            </button>
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

    {#if diagnostics}
        <details class="diagnostics">
            <summary
                title="What the app is running — copyable, with the log tail"
            >
                About this app — v{diagnostics.version} ({diagnostics.platform})
            </summary>
            <div class="diagnostics-meta">
                <span>
                    {diagnostics.projects} project{diagnostics.projects === 1
                        ? ""
                        : "s"}
                </span>
                <span title="The log file, read-only">
                    {diagnostics.logFile}
                </span>
                <button
                    type="button"
                    title="Copy the diagnostics report to the clipboard"
                    onclick={copyDiagnostics}
                >
                    {copied ? "Copied" : "Copy diagnostics"}
                </button>
                <button
                    type="button"
                    title="Save the diagnostics report to a file"
                    onclick={saveDiagnostics}
                >
                    {saveNote || "Save diagnostics"}
                </button>
                <button
                    type="button"
                    title="Export the registry, session, settings and operation log as a zip (overwrites nothing)"
                    onclick={backupAppData}
                >
                    {backupNote || "Back up app data"}
                </button>
                <button
                    type="button"
                    title="Restore the registry, session, settings and operation log from a zip. The current data is overwritten"
                    onclick={restoreAppData}
                >
                    {restoreArmed ? "Confirm restore" : "Restore app data"}
                </button>
            </div>
            {#if backupError}
                <p class="diag-error" role="alert">{backupError.message}</p>
            {/if}
            {#if saveError}
                <p class="diag-error" role="alert">{saveError.message}</p>
            {/if}
            {#if diagnostics.logTail.length}
                <pre class="log-tail">{diagnostics.logTail.join("\n")}</pre>
            {:else}
                <p class="muted">No log lines yet.</p>
            {/if}
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
            <h2>Welcome to Project Manager</h2>
            <p class="muted">
                A cross-platform home for your git projects: track local and
                remote repositories, stage private work to public and ship
                releases — all from the hub above.
            </p>
            <ul class="quick">
                <li>
                    Add an existing repository that has a <code>.proj</code> file.
                </li>
                <li>Clone one from a URL, such as GitHub.</li>
                <li>
                    Create a new project and let Project Manager set up the
                    folders.
                </li>
            </ul>
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
{:else if dialog === "preferences"}
    <PreferencesDialog
        {themes}
        {themeId}
        {backups}
        {ontheme}
        {onbackups}
        onclose={() => (dialog = null)}
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
    .diagnostics {
        margin-bottom: 0.75rem;
    }
    .diagnostics summary {
        cursor: pointer;
        color: var(--text-muted);
        font-size: 0.85rem;
    }
    .diagnostics-meta {
        display: flex;
        flex-wrap: wrap;
        gap: 0.5rem;
        align-items: center;
        margin: 0.5rem 0;
        color: var(--text-muted);
        font-size: 0.8rem;
    }
    .diag-error {
        margin: 0.4rem 0 0;
        color: var(--danger);
        font-size: 0.85em;
    }
    .diagnostics-meta span:last-of-type {
        overflow-wrap: anywhere;
        flex: 1;
    }
    .log-tail {
        margin: 0;
        padding: 0.5rem;
        background: var(--surface-alt, rgba(0, 0, 0, 0.25));
        border-radius: 0.4rem;
        color: var(--text-muted);
        font-size: 0.72rem;
        line-height: 1.5;
        overflow-x: auto;
        max-height: 14rem;
        overflow-y: auto;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
    }
    .grid {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr));
        gap: 0.75rem;
    }
    .empty {
        max-width: 34rem;
        margin: 2rem auto;
        padding: 1.5rem;
        border: 1px dashed var(--border);
        border-radius: 12px;
        text-align: center;
    }
    .empty h2 {
        margin: 0 0 0.5rem;
        font-size: 1.35rem;
    }
    .empty .quick {
        text-align: left;
        margin: 1rem auto 0;
        padding-left: 1.4rem;
        display: flex;
        flex-direction: column;
        gap: 0.35rem;
    }
    .muted {
        color: var(--text-muted);
    }
</style>
