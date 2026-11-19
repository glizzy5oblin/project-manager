<script lang="ts">
    // The window: tab bar (hub + project tabs), the showing tab, and the task bar.
    import { listen } from "@tauri-apps/api/event";
    import { onMount } from "svelte";
    import { api, toAppError } from "./lib/api";
    import type { AppError } from "./lib/bindings/AppError";
    import type { ProjectEntry } from "./lib/bindings/ProjectEntry";
    import type { ProjectSummary } from "./lib/bindings/ProjectSummary";
    import type { ProjectView } from "./lib/bindings/ProjectView";
    import type { RepoChanged } from "./lib/bindings/RepoChanged";
    import type { Session } from "./lib/bindings/Session";
    import type { ThemeList } from "./lib/bindings/ThemeList";
    import ErrorBox from "./lib/components/ErrorBox.svelte";
    import Hub from "./lib/components/Hub.svelte";
    import ProjectTab from "./lib/components/ProjectTab.svelte";
    import TabBar from "./lib/components/TabBar.svelte";
    import TaskBar from "./lib/components/TaskBar.svelte";
    import {
        closeTab,
        cycle,
        moveTab,
        openTab,
        pruneTabs,
        setView,
    } from "./lib/tabs";
    import { initTasks } from "./lib/tasks.svelte";
    import { applyTheme, pickTheme } from "./lib/theme";

    let session = $state<Session>({ tabs: [], active: null });
    let ready = $state(false);
    let projects = $state<ProjectSummary[]>([]);
    let loading = $state(true);
    let hubError = $state<AppError | null>(null);
    let gitError = $state<AppError | null>(null);
    let themes = $state<ThemeList>({ themes: [], problems: [] });
    let themeId = $state("dark");
    /** Per project: bumped when its files change, so its tab re-reads. */
    let changed = $state<Record<string, number>>({});

    const byId = $derived(new Map(projects.map((p) => [p.id, p])));
    const names = $derived(new Map(projects.map((p) => [p.id, p.name])));

    async function refresh() {
        loading = true;
        try {
            projects = await api.projectsList();
            hubError = null;
        } catch (e) {
            hubError = toAppError(e);
        } finally {
            loading = false;
        }
    }

    async function refreshOne(id: string) {
        try {
            const s = await api.projectSummary(id);
            projects = projects.map((p) => (p.id === id ? s : p));
        } catch {
            // Removed meanwhile; the next full refresh settles it.
        }
    }

    function update(next: Session) {
        session = next;
    }

    function setTheme(list: ThemeList, id: string) {
        themes = list;
        const t = pickTheme(list, id);
        if (t) applyTheme(t);
        for (const p of list.problems)
            console.warn(`theme ${p.file}: ${p.message}`);
    }

    async function chooseTheme(id: string) {
        themeId = id;
        setTheme(themes, id);
        try {
            await api.settingsSave({ theme: id });
        } catch (e) {
            hubError = toAppError(e);
        }
    }

    async function added(p: ProjectEntry) {
        await refresh();
        update(openTab(session, p.id));
    }

    async function remove(id: string) {
        try {
            await api.projectRemove(id);
        } catch (e) {
            hubError = toAppError(e);
        }
        update(closeTab(session, id));
        await refresh();
    }

    onMount(() => {
        const unlisten: Promise<() => void>[] = [];
        (async () => {
            const [settings, list, saved] = await Promise.all([
                api.settingsLoad(),
                api.themesList(),
                api.sessionLoad(),
            ]);
            themeId = settings.theme;
            setTheme(list, settings.theme);
            unlisten.push(
                listen<ThemeList>("themes-changed", (e) =>
                    setTheme(e.payload, themeId),
                ),
            );
            unlisten.push(
                listen<RepoChanged>("repo-changed", (e) => {
                    for (const id of e.payload.ids) {
                        changed[id] = (changed[id] ?? 0) + 1;
                        void refreshOne(id);
                    }
                }),
            );
            await initTasks();
            session = saved;
            await refresh();
            session = pruneTabs(session, new Set(projects.map((p) => p.id)));
            ready = true;
            api.gitCheck().catch((e) => (gitError = toAppError(e)));
        })().catch((e) => (hubError = toAppError(e)));
        const focus = () => {
            if (ready && session.active === null) void refresh();
        };
        window.addEventListener("focus", focus);
        return () => {
            window.removeEventListener("focus", focus);
            for (const u of unlisten) void u.then((f) => f());
        };
    });

    // Persist tabs (debounced) and watch exactly the open projects.
    $effect(() => {
        if (!ready) return;
        const snapshot = $state.snapshot(session);
        const timer = setTimeout(() => {
            api.sessionSave(snapshot).catch((e) =>
                console.warn("session not saved", e),
            );
        }, 300);
        return () => clearTimeout(timer);
    });
    $effect(() => {
        if (!ready) return;
        const ids = session.tabs.map((t) => t.projectId);
        api.projectsWatch(ids).catch((e) => console.warn("watch failed", e));
    });

    function keydown(e: KeyboardEvent) {
        if (document.querySelector("dialog[open]")) return;
        const mod = e.ctrlKey || e.metaKey;
        if (e.ctrlKey && e.key === "Tab") {
            update(cycle(session, e.shiftKey ? -1 : 1));
        } else if (mod && e.key === "PageDown") {
            update(cycle(session, 1));
        } else if (mod && e.key === "PageUp") {
            update(cycle(session, -1));
        } else if (
            mod &&
            e.key.toLowerCase() === "w" &&
            session.active !== null
        ) {
            update(closeTab(session, session.active));
        } else {
            return;
        }
        e.preventDefault();
    }
</script>

<svelte:window onkeydown={keydown} />

<div class="app">
    <TabBar
        {session}
        {names}
        onselect={(id) => update({ ...session, active: id })}
        onclose={(id) => update(closeTab(session, id))}
        onmove={(id, to) => update(moveTab(session, id, to))}
    />
    {#if gitError}
        <div class="banner"><ErrorBox error={gitError} /></div>
    {/if}
    <main>
        <div class="pane" hidden={session.active !== null}>
            <Hub
                {projects}
                {loading}
                error={hubError}
                themes={themes.themes}
                {themeId}
                themeProblems={themes.problems}
                onopen={(id) => update(openTab(session, id))}
                onadded={added}
                onremove={remove}
                onrefresh={refresh}
                ontheme={chooseTheme}
            />
        </div>
        {#each session.tabs as tab (tab.projectId)}
            <div
                class="pane"
                hidden={session.active !== tab.projectId}
                role="tabpanel"
            >
                <ProjectTab
                    id={tab.projectId}
                    summary={byId.get(tab.projectId)}
                    view={tab.view}
                    active={session.active === tab.projectId}
                    changed={changed[tab.projectId] ?? 0}
                    onview={(v: ProjectView) =>
                        update(setView(session, tab.projectId, v))}
                />
            </div>
        {/each}
    </main>
    <TaskBar />
</div>

<style>
    .app {
        display: flex;
        flex-direction: column;
        height: 100vh;
    }
    main {
        flex: 1;
        min-height: 0;
    }
    .pane {
        height: 100%;
        overflow: auto;
    }
    .pane[hidden] {
        display: none;
    }
    .banner {
        padding: 0.5rem 0.75rem 0;
    }
</style>
