// Typed wrappers for the backend commands (src/desktop/src/commands.rs). Every command rejects
// with an AppError; `toAppError` turns anything else into one.

import { invoke } from "@tauri-apps/api/core";
import type { Account } from "./bindings/Account";
import type { Action } from "./bindings/Action";
import type { AppError } from "./bindings/AppError";
import type { AuthStatus } from "./bindings/AuthStatus";
import type { Blame } from "./bindings/Blame";
import type { Diagnostics } from "./bindings/Diagnostics";
import type { CommitDetail } from "./bindings/CommitDetail";
import type { BackupRecord } from "./bindings/BackupRecord";
import type { FileList } from "./bindings/FileList";
import type { FileText } from "./bindings/FileText";
import type { ConflictInfo } from "./bindings/ConflictInfo";
import type { FileDiff } from "./bindings/FileDiff";
import type { RestorePreview } from "./bindings/RestorePreview";
import type { WorkStat } from "./bindings/WorkStat";
import type { GitInstall } from "./bindings/GitInstall";
import type { GraphInfo } from "./bindings/GraphInfo";
import type { GraphOptions } from "./bindings/GraphOptions";
import type { GraphRow } from "./bindings/GraphRow";
import type { LogQuery } from "./bindings/LogQuery";
import type { OpEntry } from "./bindings/OpEntry";
import type { RebaseRange } from "./bindings/RebaseRange";
import type { Preview } from "./bindings/Preview";
import type { StagingRecord } from "./bindings/StagingRecord";
import type { StagePreview } from "./bindings/StagePreview";
import type { VerifyReport } from "./bindings/VerifyReport";
import type { Bump } from "./bindings/Bump";
import type { ReleasePreview } from "./bindings/ReleasePreview";
import type { TagPreview } from "./bindings/TagPreview";
import type { TagResult } from "./bindings/TagResult";
import type { ZipOutcome } from "./bindings/ZipOutcome";
import type { Changelog } from "./bindings/Changelog";
import type { GitHubRelease } from "./bindings/GitHubRelease";
import type { ReleaseAsset } from "./bindings/ReleaseAsset";
import type { ReleaseChecklist } from "./bindings/ReleaseChecklist";
import type { ReleaseHistoryEntry } from "./bindings/ReleaseHistoryEntry";
import type { ProjectEntry } from "./bindings/ProjectEntry";
import type { ProjectSummary } from "./bindings/ProjectSummary";
import type { RepoSide } from "./bindings/RepoSide";
import type { RepoState } from "./bindings/RepoState";
import type { SearchResult } from "./bindings/SearchResult";
import type { Session } from "./bindings/Session";
import type { Settings } from "./bindings/Settings";
import type { SignIn } from "./bindings/SignIn";
import type { StashEntry } from "./bindings/StashEntry";
import type { TaskInfo } from "./bindings/TaskInfo";
import type { ThemeList } from "./bindings/ThemeList";
import type { WorkflowMode } from "./bindings/WorkflowMode";
import type { WorkSide } from "./bindings/WorkSide";

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
    projectState: (id: string, repoSide: RepoSide = "private") =>
        call<RepoState>("project_state", { id, repoSide }),
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
    /** Rewrite the user theme with a full merged colors map (only the diff is stored). */
    themeSaveUser: (id: string, colors: Record<string, string>) =>
        call<void>("theme_save_user", { id, colors }),
    /** Save a new user theme; the new theme's id is returned. */
    themeSaveAs: (
        name: string,
        scheme: "dark" | "light",
        colors: Record<string, string>,
    ) => call<string>("theme_save_as", { name, scheme, colors }),
    /** Write the theme to a path as a complete file. */
    themeExport: (id: string, path: string) =>
        call<void>("theme_export", { id, path }),
    /** Validate a theme file and copy it into the themes folder; the new id is returned. */
    themeImport: (path: string) => call<string>("theme_import", { path }),

    /** `check` asks GitHub whether the sign-in still works; `gh` also looks for the GitHub CLI. */
    githubStatus: (check: boolean, gh: boolean) =>
        call<AuthStatus>("github_status", { check, gh }),
    /** Starts the device flow; the Account arrives with the task's finished event. */
    githubSignIn: () => call<SignIn>("github_sign_in"),
    githubUseGh: () => call<Account>("github_use_gh"),
    githubSignOut: () => call<void>("github_sign_out"),

    /** Builds (or reuses) a project's commit graph. Row requests name its `generation`. */
    graphLoad: (
        id: string,
        options: GraphOptions,
        repoSide: RepoSide = "private",
    ) => call<GraphInfo>("graph_load", { id, options, repoSide }),
    /** Up to 2000 rows; rejects with `stale` once the graph has been rebuilt. */
    graphRows: (
        id: string,
        generation: number,
        start: number,
        count: number,
        repoSide: RepoSide = "private",
    ) =>
        call<GraphRow[]>("graph_rows", {
            id,
            generation,
            start,
            count,
            repoSide,
        }),
    /** Row of a full or abbreviated (4+ digit) commit id, or null. */
    graphFind: (
        id: string,
        generation: number,
        commit: string,
        repoSide: RepoSide = "private",
    ) =>
        call<number | null>("graph_find", { id, generation, commit, repoSide }),
    /** Starting a search cancels the project's previous one (it rejects with `cancelled`). */
    logSearch: (
        id: string,
        generation: number,
        query: LogQuery,
        repoSide: RepoSide = "private",
    ) => call<SearchResult>("log_search", { id, generation, query, repoSide }),
    logSearchCancel: (id: string, repoSide: RepoSide = "private") =>
        call<void>("log_search_cancel", { id, repoSide }),
    commitDetail: (
        id: string,
        commit: string,
        parent: number | null,
        repoSide: RepoSide = "private",
    ) => call<CommitDetail>("commit_detail", { id, commit, parent, repoSide }),
    fileDiff: (
        id: string,
        commit: string,
        parent: number | null,
        path: string,
        oldPath: string | null,
        force: boolean,
        context: number | null = null,
        repoSide: RepoSide = "private",
    ) =>
        call<FileDiff>("file_diff", {
            id,
            commit,
            parent,
            path,
            oldPath,
            context,
            force,
            repoSide,
        }),
    fileBlame: (
        id: string,
        commit: string,
        path: string,
        repoSide: RepoSide = "private",
    ) => call<Blame>("file_blame", { id, commit, path, repoSide }),
    /** One file's uncommitted change: staged (HEAD to index) or unstaged (index to disk). */
    workDiff: (
        id: string,
        path: string,
        side: WorkSide,
        force: boolean,
        repoSide: RepoSide = "private",
    ) =>
        call<FileDiff>("work_diff", {
            id,
            path,
            side,
            context: null,
            force,
            repoSide,
        }),
    /** Lines added and removed in each uncommitted file of the Changes list. */
    workStats: (
        id: string,
        files: [string, WorkSide][],
        repoSide: RepoSide = "private",
    ) => call<WorkStat[]>("work_stats", { id, files, repoSide }),
    /** What `action` would do; dangerous actions need the returned `token` to run. */
    actionPreview: (
        id: string,
        action: Action,
        allowUntrusted = false,
        repoSide: RepoSide = "private",
    ) =>
        call<Preview>("action_preview", {
            id,
            action,
            allowUntrusted,
            repoSide,
        }),
    /** Starts the action as a task; its ActionOutcome arrives in the task's finished event. */
    actionRun: (
        id: string,
        action: Action,
        token: string | null,
        allowUntrusted = false,
        repoSide: RepoSide = "private",
    ) =>
        call<number>("action_run", {
            id,
            action,
            token,
            allowUntrusted,
            repoSide,
        }),
    /** The stash list, newest first (at most 200). */
    stashList: (id: string, repoSide: RepoSide = "private") =>
        call<StashEntry[]>("stash_list", { id, repoSide }),
    /** The current branch's commits from `from` on, oldest first, for the commit editor. */
    rebaseRange: (id: string, from: string, repoSide: RepoSide = "private") =>
        call<RebaseRange>("rebase_range", { id, from, repoSide }),
    /** Show the project's folder (or one file in it, relative path) in the file manager. */
    openFolder: (
        id: string,
        path: string | null = null,
        repoSide: RepoSide = "private",
    ) => call<void>("open_folder", { id, path, repoSide }),
    /** One folder of the project ("" is the top), folders first. */
    filesList: (id: string, dir: string, repoSide: RepoSide = "private") =>
        call<FileList>("files_list", { id, dir, repoSide }),
    /** A text file of the project, or why it cannot be edited. */
    fileRead: (id: string, path: string, repoSide: RepoSide = "private") =>
        call<FileText>("file_read", { id, path, repoSide }),
    /** Save a text file of the project. */
    fileWrite: (
        id: string,
        path: string,
        content: string,
        repoSide: RepoSide = "private",
    ) => call<void>("file_write", { id, path, content, repoSide }),
    /** Make a new empty file (or folder) in the project. Never replaces one. */
    fileCreate: (
        id: string,
        path: string,
        dir: boolean,
        repoSide: RepoSide = "private",
    ) => call<void>("file_create", { id, path, dir, repoSide }),
    /** What kind of conflict a file is in and where the two sides disagree. */
    conflictInfo: (id: string, path: string, repoSide: RepoSide = "private") =>
        call<ConflictInfo>("conflict_info", { id, path, repoSide }),
    /** Restore points (backups) of a project, newest first. */
    backupsList: (id: string, repoSide: RepoSide = "private") =>
        call<BackupRecord[]>("backups_list", { id, repoSide }),
    /** What restoring a restore point would change. */
    backupPreview: (
        id: string,
        backup: string,
        repoSide: RepoSide = "private",
    ) => call<RestorePreview>("backup_preview", { id, backup, repoSide }),
    /** Starts a restore as a task; its ActionOutcome arrives in the task's finished event. */
    backupRestore: (
        id: string,
        backup: string,
        repoSide: RepoSide = "private",
    ) => call<number>("backup_restore", { id, backup, repoSide }),
    /** Bring a restore point's refs back from its bundle. */
    backupsImportBundle: (
        id: string,
        backup: string,
        repoSide: RepoSide = "private",
    ) => call<void>("backups_import_bundle", { id, backup, repoSide }),
    /** Delete a restore point (its refs, bundle file and index entry). */
    backupsDelete: (
        id: string,
        backup: string,
        repoSide: RepoSide = "private",
    ) => call<void>("backups_delete", { id, backup, repoSide }),
    /** The restore points the default retention would delete (too many or too old). */
    backupsCleanupPreview: (id: string, repoSide: RepoSide = "private") =>
        call<BackupRecord[]>("backups_cleanup_preview", { id, repoSide }),
    /** Apply the default retention now; returns the deleted ids. */
    backupsCleanup: (id: string, repoSide: RepoSide = "private") =>
        call<string[]>("backups_cleanup", { id, repoSide }),
    /** Preview of what a sync would do: the file list, its status and the leak findings. */
    stagingPreview: (id: string) =>
        call<StagePreview>("staging_preview", { id }),
    /**
     * Starts the sync as a task. `allow` names files that may stage despite a blocking finding;
     * `message` overrides the generated commit message; `push` pushes the public side afterwards.
     * Its ActionOutcome arrives in the task's finished event.
     */
    stagingSync: (
        id: string,
        allow: string[] = [],
        message: string | null = null,
        push = false,
    ) => call<number>("staging_sync", { id, allow, message, push }),
    /** Verify that the public side matches the private side. */
    stagingVerify: (id: string) => call<VerifyReport>("staging_verify", { id }),
    /** The staging records, oldest first. */
    stagingRecords: (id: string) =>
        call<StagingRecord[]>("staging_records", { id }),
    /** Set or clear a per-file override ("include" | "exclude" | "clear"). */
    stagingSetOverride: (id: string, path: string, kind: string) =>
        call<void>("staging_set_override", { id, path, kind }),
    /**
     * The diff of one file between the private and public sides (the public side is `old`).
     * `force` shows a file over the large-file limit.
     */
    stagingDiff: (id: string, path: string, force: boolean) =>
        call<FileDiff>("staging_diff", { id, path, force }),
    /** Preview a release bump: what moves, and to what. Writes nothing. */
    releasePreview: (id: string, bump: Bump, exact: string | null) =>
        call<ReleasePreview>("release_preview", { id, bump, exact }),
    /** Apply a release bump: rewrite `.proj` and the configured version files. */
    releaseApply: (id: string, bump: Bump, exact: string | null) =>
        call<ReleasePreview>("release_apply", { id, bump, exact }),
    /** Preview the release tag: what would be created, and whether the checks pass. */
    releaseTagPreview: (id: string) =>
        call<TagPreview>("release_tag_preview", { id }),
    /** Create the annotated release tag, optionally pushed. */
    releaseTagCreate: (id: string, push: boolean) =>
        call<TagResult>("release_tag_create", { id, push }),
    /** Build the release zip: the staged public files, with a manifest and checksum. */
    releaseZip: (id: string) => call<ZipOutcome>("release_zip", { id }),
    /** Generate the release notes: commits since the last tag, grouped and rendered as markdown. */
    releaseChangelog: (id: string) =>
        call<Changelog>("release_changelog", { id }),
    /** The recorded releases in `.proj` (`[[release.history]]`, oldest first). */
    releaseHistoryList: (id: string) =>
        call<ReleaseHistoryEntry[]>("release_history_list", { id }),
    /** Record the current version's release (idempotent; returns whether one was added). */
    releaseHistoryRecord: (id: string) =>
        call<boolean>("release_history_record", { id }),
    /** The ordered release checklist (read-only). */
    releaseChecklist: (id: string) =>
        call<ReleaseChecklist>("release_checklist", { id }),
    /** The public repo's releases, as the GitHub API returns them. */
    releaseGitHubList: (id: string) =>
        call<GitHubRelease[]>("release_github_list", { id }),
    /** Create a GitHub release. The tag is `v<version>` unless one is given. */
    releaseGitHubCreate: (
        id: string,
        body: string,
        name: string | null,
        tag: string | null,
        draft: boolean,
        prerelease: boolean,
    ) =>
        call<GitHubRelease>("release_github_create", {
            id,
            body,
            name,
            tag,
            draft,
            prerelease,
        }),
    /** Edit a release. Only the given fields are sent. */
    releaseGitHubUpdate: (
        id: string,
        releaseId: bigint | number,
        name: string | null,
        body: string | null,
        draft: boolean | null,
        prerelease: boolean | null,
    ) =>
        call<GitHubRelease>("release_github_update", {
            id,
            releaseId: Number(releaseId),
            name,
            body,
            draft,
            prerelease,
        }),
    /** Delete a release. */
    releaseGitHubDelete: (id: string, releaseId: bigint | number) =>
        call<void>("release_github_delete", {
            id,
            releaseId: Number(releaseId),
        }),
    /** Upload the release zip as an asset on the release. */
    releaseGitHubUpload: (id: string, releaseId: bigint | number) =>
        call<ReleaseAsset>("release_github_upload", {
            id,
            releaseId: Number(releaseId),
        }),
    /** Replace the zip asset on the release (delete the old one, then upload the current zip). */
    releaseGitHubReplaceAsset: (id: string, releaseId: bigint | number) =>
        call<ReleaseAsset>("release_github_replace_asset", {
            id,
            releaseId: Number(releaseId),
        }),
    opLog: () => call<OpEntry[]>("op_log"),
    opLogClear: () => call<void>("op_log_clear"),
    diagnostics: () => call<Diagnostics>("get_diagnostics"),
    /** Write the diagnostics report to a path (one the user picked in a dialog). */
    diagnosticsSave: (path: string) => call<void>("diagnostics_save", { path }),
    /** Export the app-level state (registry, session, settings, operation log) as a zip. */
    appDataExport: (path: string) => call<void>("app_data_export", { path }),
    /** Restore the app-level state from a zip written by appDataExport. Returns the files restored. */
    appDataRestore: (path: string) =>
        call<string[]>("app_data_restore", { path }),
};
