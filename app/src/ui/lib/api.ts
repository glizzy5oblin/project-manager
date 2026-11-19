// Typed wrappers for the backend commands (src/desktop/src/commands.rs). Every command rejects
// with an AppError; `toAppError` turns anything else into one.

import { invoke } from "@tauri-apps/api/core";
import type { Account } from "./bindings/Account";
import type { AppError } from "./bindings/AppError";
import type { AuthStatus } from "./bindings/AuthStatus";
import type { Blame } from "./bindings/Blame";
import type { CommitDetail } from "./bindings/CommitDetail";
import type { FileDiff } from "./bindings/FileDiff";
import type { GitInstall } from "./bindings/GitInstall";
import type { GraphInfo } from "./bindings/GraphInfo";
import type { GraphOptions } from "./bindings/GraphOptions";
import type { GraphRow } from "./bindings/GraphRow";
import type { LogQuery } from "./bindings/LogQuery";
import type { ProjectEntry } from "./bindings/ProjectEntry";
import type { ProjectSummary } from "./bindings/ProjectSummary";
import type { RepoState } from "./bindings/RepoState";
import type { SearchResult } from "./bindings/SearchResult";
import type { Session } from "./bindings/Session";
import type { Settings } from "./bindings/Settings";
import type { SignIn } from "./bindings/SignIn";
import type { TaskInfo } from "./bindings/TaskInfo";
import type { ThemeList } from "./bindings/ThemeList";
import type { WorkflowMode } from "./bindings/WorkflowMode";

export function isAppError(e: unknown): e is AppError {
    return (
        typeof e === "object" &&
        e !== null &&
        typeof (e as AppError).kind === "string" &&
        typeof (e as AppError).message === "string"
    );
}

export function toAppError(e: unknown): AppError {
    if (isAppError(e)) {
        return { ...e, detail: e.detail ?? null, issues: e.issues ?? [] };
    }
    const message = e instanceof Error ? e.message : String(e);
    return { kind: "internal", message, detail: null, issues: [] };
}

async function call<T>(
    cmd: string,
    args?: Record<string, unknown>,
): Promise<T> {
    try {
        return await invoke<T>(cmd, args);
    } catch (e) {
        throw toAppError(e);
    }
}

export const api = {
    gitCheck: () => call<GitInstall>("git_check"),
    tasksRunning: () => call<TaskInfo[]>("tasks_running"),
    taskCancel: (id: number) => call<boolean>("task_cancel", { id }),

    projectsList: () => call<ProjectSummary[]>("projects_list"),
    projectSummary: (id: string) =>
        call<ProjectSummary>("project_summary", { id }),
    projectState: (id: string) => call<RepoState>("project_state", { id }),
    projectAdd: (path: string, create: WorkflowMode | null = null) =>
        call<ProjectEntry>("project_add", { path, create }),
    projectCreate: (parent: string, name: string, mode: WorkflowMode) =>
        call<ProjectEntry>("project_create", { parent, name, mode }),
    /** Starts a clone task; the result (a CloneResult) arrives with the task's finished event. */
    projectClone: (url: string, parent: string, name: string | null) =>
        call<number>("project_clone", { url, parent, name }),
    projectRemove: (id: string) => call<void>("project_remove", { id }),
    projectReorder: (id: string, index: number) =>
        call<void>("project_reorder", { id, index }),
    projectsWatch: (ids: string[]) => call<void>("projects_watch", { ids }),

    sessionLoad: () => call<Session>("session_load"),
    sessionSave: (session: Session) => call<void>("session_save", { session }),
    settingsLoad: () => call<Settings>("settings_load"),
    settingsSave: (settings: Settings) =>
        call<void>("settings_save", { settings }),
    themesList: () => call<ThemeList>("themes_list"),
    themesFolder: () => call<string>("themes_folder"),

    /** `check` asks GitHub whether the sign-in still works; `gh` also looks for the GitHub CLI. */
    githubStatus: (check: boolean, gh: boolean) =>
        call<AuthStatus>("github_status", { check, gh }),
    /** Starts the device flow; the Account arrives with the task's finished event. */
    githubSignIn: () => call<SignIn>("github_sign_in"),
    githubUseGh: () => call<Account>("github_use_gh"),
    githubSignOut: () => call<void>("github_sign_out"),

    /** Builds (or reuses) a project's commit graph. Row requests name its `generation`. */
    graphLoad: (id: string, options: GraphOptions) =>
        call<GraphInfo>("graph_load", { id, options }),
    /** Up to 2000 rows; rejects with `stale` once the graph has been rebuilt. */
    graphRows: (id: string, generation: number, start: number, count: number) =>
        call<GraphRow[]>("graph_rows", { id, generation, start, count }),
    /** Row of a full or abbreviated (4+ digit) commit id, or null. */
    graphFind: (id: string, generation: number, commit: string) =>
        call<number | null>("graph_find", { id, generation, commit }),
    /** Starting a search cancels the project's previous one (it rejects with `cancelled`). */
    logSearch: (id: string, generation: number, query: LogQuery) =>
        call<SearchResult>("log_search", { id, generation, query }),
    logSearchCancel: (id: string) => call<void>("log_search_cancel", { id }),
    commitDetail: (id: string, commit: string, parent: number | null) =>
        call<CommitDetail>("commit_detail", { id, commit, parent }),
    fileDiff: (
        id: string,
        commit: string,
        parent: number | null,
        path: string,
        oldPath: string | null,
        force: boolean,
        context: number | null = null,
    ) =>
        call<FileDiff>("file_diff", {
            id,
            commit,
            parent,
            path,
            oldPath,
            context,
            force,
        }),
    fileBlame: (id: string, commit: string, path: string) =>
        call<Blame>("file_blame", { id, commit, path }),
};
