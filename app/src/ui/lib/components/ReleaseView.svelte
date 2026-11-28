<script lang="ts">
    // Releases: the version bump (task 5.1) and the annotated tag (task 5.2). The bump preview
    // is read-only and lists what moves; the apply rewrites `.proj` and the configured version
    // files. The tag preview is also read-only and lists the checks; the Tag button creates the
    // annotated tag, and optionally pushes it. No files are published or uploaded yet — that is
    // tasks 5.3+.
    import { api, toAppError } from "../api";
    import type { Bump } from "../bindings/Bump";
    import type { ReleasePreview } from "../bindings/ReleasePreview";
    import type { TagPreview } from "../bindings/TagPreview";
    import type { TagResult } from "../bindings/TagResult";
    import type { ZipOutcome } from "../bindings/ZipOutcome";
    import type { Changelog } from "../bindings/Changelog";
    import type { GitHubRelease } from "../bindings/GitHubRelease";
    import type { ReleaseChecklist } from "../bindings/ReleaseChecklist";
    import type { ReleaseHistoryEntry } from "../bindings/ReleaseHistoryEntry";
    import type { AppError } from "../bindings/AppError";
    import ErrorBox from "./ErrorBox.svelte";
    import { openUrl } from "@tauri-apps/plugin-opener";

    let {
        projectId,
        changed,
        active,
    }: {
        projectId: string;
        /** Bumped when the project's files change on disk. */
        changed: number;
        active: boolean;
    } = $props();

    // --- The bump (5.1) ---
    let current = $state("");
    let target = $state<string | null>(null);
    let preview = $state<ReleasePreview | null>(null);
    let error = $state<AppError | null>(null);
    let loading = $state(false);
    let applying = $state(false);
    let applied = $state(false);
    let bump = $state<Bump>("patch");
    let exact = $state("");
    let loadedFor = -1;

    const exactMode = $derived(bump === "exact");

    async function load(tick: number) {
        loadedFor = tick;
        loading = true;
        try {
            const out = await api.releasePreview(
                projectId,
                bump,
                exactMode ? exact || null : null,
            );
            preview = out;
            current = out.current;
            target = out.target;
            error = null;
            applied = false;
        } catch (e) {
            error = toAppError(e);
            preview = null;
        } finally {
            loading = false;
        }
    }

    async function apply() {
        if (applying || loading) return;
        applying = true;
        applied = false;
        try {
            const out = await api.releaseApply(
                projectId,
                bump,
                exactMode ? exact || null : null,
            );
            preview = out;
            current = out.current;
            target = out.target;
            applied = true;
            error = null;
            await loadTag();
        } catch (e) {
            error = toAppError(e);
        } finally {
            applying = false;
        }
    }

    // --- The tag (5.2) ---
    let tagPreview = $state<TagPreview | null>(null);
    let tagResult = $state<TagResult | null>(null);
    let tagError = $state<AppError | null>(null);
    let tagging = $state(false);
    let tagLoading = $state(false);
    let tagPush = $state(true);

    async function loadTag() {
        tagLoading = true;
        try {
            tagPreview = await api.releaseTagPreview(projectId);
            tagError = null;
        } catch (e) {
            tagError = toAppError(e);
            tagPreview = null;
        } finally {
            tagLoading = false;
        }
    }

    async function tag() {
        if (tagging || tagLoading || !tagPreview) return;
        tagging = true;
        tagResult = null;
        try {
            tagResult = await api.releaseTagCreate(projectId, tagPush);
            tagError = null;
            await loadTag();
            await refreshHistory();
        } catch (e) {
            tagError = toAppError(e);
        } finally {
            tagging = false;
        }
    }

    // --- The release zip (5.3) ---
    let zip = $state<ZipOutcome | null>(null);
    let zipError = $state<AppError | null>(null);
    let zipping = $state(false);

    async function buildZip() {
        if (zipping) return;
        zipping = true;
        zipError = null;
        try {
            zip = await api.releaseZip(projectId);
            await loadChecklist();
        } catch (e) {
            zipError = toAppError(e);
            zip = null;
        } finally {
            zipping = false;
        }
    }

    function formatBytes(n: bigint | number): string {
        const v = typeof n === "bigint" ? Number(n) : n;
        if (!Number.isFinite(v)) return String(n);
        if (v < 1024) return `${v} B`;
        if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KB`;
        if (v < 1024 * 1024 * 1024)
            return `${(v / (1024 * 1024)).toFixed(1)} MB`;
        return `${(v / (1024 * 1024 * 1024)).toFixed(2)} GB`;
    }

    const tagBlocked = $derived(
        !tagPreview ||
            !tagPreview.clean ||
            tagPreview.tagExists ||
            tagPreview.branch !== tagPreview.expectedBranch,
    );
    const tagBlockedReason = $derived(
        !tagPreview
            ? "no preview"
            : !tagPreview.clean
              ? "commit or stash the changes first"
              : tagPreview.branch !== tagPreview.expectedBranch
                ? `on ${tagPreview.branch}, not ${tagPreview.expectedBranch}`
                : tagPreview.tagExists
                  ? `tag ${tagPreview.tag} already exists`
                  : "",
    );

    // --- The release notes (5.4) ---
    let notes = $state<Changelog | null>(null);
    let notesText = $state("");
    let notesError = $state<AppError | null>(null);
    let notesLoading = $state(false);
    let notesTouched = $state(false);
    let notesLoadedFor = -1;

    async function loadNotes(force = false) {
        if (notesLoading) return;
        notesLoading = true;
        try {
            const out = await api.releaseChangelog(projectId);
            notes = out;
            // Don't clobber the user's edits unless they asked for a regenerate.
            if (force || !notesTouched) notesText = out.markdown;
            notesError = null;
        } catch (e) {
            notesError = toAppError(e);
            notes = null;
        } finally {
            notesLoading = false;
        }
    }

    function markNotesTouched() {
        notesTouched = true;
    }

    async function copyNotes() {
        try {
            await navigator.clipboard.writeText(notesText);
        } catch {
            // Clipboard unavailable — nothing else to do.
        }
    }

    const notesCommitCount = $derived(
        notes ? notes.groups.reduce((n, g) => n + g.commits.length, 0) : 0,
    );

    // --- The GitHub releases (5.5) ---
    let ghReleases = $state<GitHubRelease[] | null>(null);
    let ghError = $state<AppError | null>(null);
    let ghLoading = $state(false);
    let publishing = $state(false);
    let uploadingId = $state<bigint | number | null>(null);
    let savingId = $state<bigint | number | null>(null);
    let prerelease = $state(false);
    let armedDeleteId = $state<bigint | number | null>(null);
    let editingId = $state<bigint | number | null>(null);
    let editName = $state("");
    let loadedGhFor = -1;

    // --- Release history + checklist (5.9) ---
    let history = $state<ReleaseHistoryEntry[]>([]);
    let historyError = $state<AppError | null>(null);
    let historyLoading = $state(false);
    let recording = $state(false);
    let checklist = $state<ReleaseChecklist | null>(null);
    let checklistError = $state<AppError | null>(null);
    let checklistLoading = $state(false);
    let loadedHistoryFor = -1;

    async function loadHistory() {
        if (historyLoading) return;
        historyLoading = true;
        historyError = null;
        try {
            history = await api.releaseHistoryList(projectId);
            historyError = null;
            loadedHistoryFor = changed;
        } catch (e) {
            history = [];
            historyError = toAppError(e);
        } finally {
            historyLoading = false;
        }
    }

    async function loadChecklist() {
        if (checklistLoading) return;
        checklistLoading = true;
        checklistError = null;
        try {
            checklist = await api.releaseChecklist(projectId);
            checklistError = null;
        } catch (e) {
            checklist = null;
            checklistError = toAppError(e);
        } finally {
            checklistLoading = false;
        }
    }

    async function recordHistory() {
        if (recording) return;
        recording = true;
        historyError = null;
        try {
            await api.releaseHistoryRecord(projectId);
            await loadHistory();
            await loadChecklist();
        } catch (e) {
            historyError = toAppError(e);
        } finally {
            recording = false;
        }
    }

    async function refreshHistory() {
        await loadHistory();
        await loadChecklist();
    }

    async function loadReleases() {
        if (ghLoading) return;
        ghLoading = true;
        try {
            ghReleases = await api.releaseGitHubList(projectId);
            ghError = null;
            loadedGhFor = changed;
        } catch (e) {
            ghError = toAppError(e);
            ghReleases = null;
        } finally {
            ghLoading = false;
        }
    }

    // A publish is a gate: the checklist decides whether the release is ready to leave.
    // The `published` and `asset` steps are the things being published, so they never block.
    const PUBLISH_BLOCKING = ["version", "tag", "zip", "checksum", "notes"];
    const publishBlockedReason = $derived.by(() => {
        if (!checklist) {
            return checklistError
                ? `the checklist could not be read: ${checklistError.message}`
                : "the checklist is still loading";
        }
        const missing = checklist.steps
            .filter((st) => PUBLISH_BLOCKING.includes(st.id) && !st.done)
            .map((st) => st.label);
        return missing.length ? `not ready: ${missing.join(", ")}` : "";
    });

    async function publish(draft: boolean) {
        if (publishing) return;
        publishing = true;
        ghError = null;
        try {
            await api.releaseGitHubCreate(
                projectId,
                notesText,
                null,
                null,
                draft,
                prerelease,
            );
            await loadReleases();
            await loadChecklist();
        } catch (e) {
            ghError = toAppError(e);
        } finally {
            publishing = false;
        }
    }

    async function remove(release: GitHubRelease) {
        if (armedDeleteId !== release.id) {
            armedDeleteId = release.id;
            return;
        }
        armedDeleteId = null;
        ghError = null;
        try {
            await api.releaseGitHubDelete(projectId, release.id);
            await loadReleases();
        } catch (e) {
            ghError = toAppError(e);
        }
    }

    async function upload(release: GitHubRelease) {
        if (uploadingId !== null) return;
        uploadingId = release.id;
        ghError = null;
        try {
            await api.releaseGitHubReplaceAsset(projectId, release.id);
            await loadReleases();
            await loadChecklist();
        } catch (e) {
            ghError = toAppError(e);
        } finally {
            uploadingId = null;
        }
    }

    function startEdit(release: GitHubRelease) {
        editingId = release.id;
        editName = release.name ?? release.tag_name;
    }

    function cancelEdit() {
        editingId = null;
    }

    async function saveEdit(release: GitHubRelease) {
        if (savingId !== null) return;
        savingId = release.id;
        ghError = null;
        try {
            await api.releaseGitHubUpdate(
                projectId,
                release.id,
                editName.trim() || null,
                null,
                null,
                null,
            );
            editingId = null;
            await loadReleases();
        } catch (e) {
            ghError = toAppError(e);
        } finally {
            savingId = null;
        }
    }

    async function openRelease(release: GitHubRelease) {
        try {
            await openUrl(release.html_url);
        } catch {
            // The browser couldn't be opened — the release page is still listed in the UI.
        }
    }

    // Refresh when this tab becomes active, or when the project changed on disk.
    $effect(() => {
        if (active && loadedGhFor !== changed) void loadReleases();
    });

    // Load the history + checklist when the tab becomes active or the project changes.
    $effect(() => {
        if (active && loadedHistoryFor !== changed) {
            void loadHistory();
            void loadChecklist();
        }
    });

    // Regenerate when the project changes on disk, unless the user has started editing.
    $effect(() => {
        if (active && notesLoadedFor !== changed && !notesTouched) {
            notesLoadedFor = changed;
            void loadNotes();
        }
    });

    // Load when this tab becomes active, and again when the project changes on disk.
    $effect(() => {
        if (active && loadedFor !== changed) void load(changed);
    });

    // Re-preview whenever the bump mode or the exact version moves — the preview is cheap and
    // read-only, and the file list depends on it.
    $effect(() => {
        if (active) void load(loadedFor);
    });

    // The tag preview is independent of the bump; it reads the tree and the tag list directly.
    $effect(() => {
        if (active) void loadTag();
    });
</script>

<section class="release" aria-label="Release">
    <div class="toolbar">
        <div class="mode" role="group" aria-label="Bump type">
            {#each [["major", "Major"], ["minor", "Minor"], ["patch", "Patch"], ["exact", "Exact"]] as [id, label] (id)}
                <button
                    type="button"
                    class:on={bump === id}
                    title={`Bump the version (${label})`}
                    aria-pressed={bump === id}
                    onclick={() => {
                        bump = id as Bump;
                    }}
                >
                    {label}
                </button>
            {/each}
        </div>
        {#if exactMode}
            <input
                type="text"
                class="exact"
                title="The exact version to set (e.g. 2.0.0)"
                aria-label="Exact version"
                bind:value={exact}
                placeholder="x.y.z"
            />
        {/if}
        <button
            type="button"
            class="apply"
            title="Apply this bump: rewrite .proj and the configured version files"
            disabled={loading || applying || (exactMode && !exact.trim())}
            onclick={apply}
        >
            {applying ? "Applying…" : "Apply"}
        </button>
    </div>

    {#if error}
        <ErrorBox {error} />
    {/if}

    {#if !applied && preview}
        <p class="note muted">
            {current} → <strong>{target}</strong>. Nothing is written until you
            apply.
        </p>
    {/if}
    {#if applied && preview}
        <p class="note applied">
            Released <strong>{current}</strong> → <strong>{target}</strong>.
        </p>
    {/if}

    {#if loading}
        <p class="muted">Reading the release…</p>
    {:else if preview}
        <ul class="files" aria-label="Files a bump would touch">
            {#each preview.files as f (f.path)}
                <li class="file">
                    <span class="path" title={f.path}>{f.path}</span>
                    <span class="meta">
                        {f.before} → {f.after}
                    </span>
                </li>
            {/each}
        </ul>
    {:else if !error}
        <p class="muted">No preview available.</p>
    {/if}

    <hr class="divider" />

    <section class="tag" aria-label="Release tag">
        <h3>
            Tag the release
            {#if tagPreview}
                <span class="muted">
                    {tagPreview.tag} on {tagPreview.branch}
                </span>
            {/if}
        </h3>

        {#if tagError}
            <ErrorBox error={tagError} />
        {/if}

        {#if tagLoading}
            <p class="muted">Reading the tag…</p>
        {:else if tagPreview}
            <p class="note muted">
                {#if tagPreview.clean}
                    Clean tree
                {:else}
                    Uncommitted changes
                {/if}
                ·{#if tagPreview.branch === tagPreview.expectedBranch}
                    on the private branch
                {:else}
                    on {tagPreview.branch}, expected {tagPreview.expectedBranch}
                {/if}
                · {tagPreview.tagExists
                    ? `tag ${tagPreview.tag} already exists`
                    : "tag free"}
            </p>
            <div class="tag-actions">
                <label
                    class="push"
                    title="Push the tag to the private remote after creating it"
                >
                    <input
                        type="checkbox"
                        title="Push the tag to the private remote after creating it"
                        bind:checked={tagPush}
                        disabled={tagging}
                    />
                    Push after tagging
                </label>
                <button
                    type="button"
                    class="apply"
                    title={tagBlocked
                        ? tagBlockedReason
                        : `Create an annotated tag ${tagPreview.tag} on ${tagPreview.branch}`}
                    disabled={tagging || tagBlocked}
                    onclick={tag}
                >
                    {tagging ? "Tagging…" : "Tag"}
                </button>
            </div>
            {#if tagResult}
                <p class="note applied">
                    Tagged <strong>{tagResult.tag}</strong> on {tagResult.branch}{tagResult.pushed
                        ? " and pushed it."
                        : "."}
                </p>
            {/if}
        {:else if !tagError}
            <p class="muted">No tag preview available.</p>
        {/if}
    </section>

    <hr class="divider" />

    <section class="zip" aria-label="Release zip">
        <h3>
            Release zip
            <span class="muted">staged public files</span>
        </h3>

        {#if zipError}
            <ErrorBox error={zipError} />
        {/if}

        <div class="zip-actions">
            <button
                type="button"
                class="apply"
                title="Build a release zip of the staged public files, with a manifest and checksum"
                disabled={zipping}
                onclick={buildZip}
            >
                {zipping ? "Building…" : "Build zip"}
            </button>
        </div>

        {#if zip}
            <p class="note">
                <strong>{zip.fileName}</strong> · {formatBytes(zip.bytes)} · {zip
                    .entries.length} files
            </p>
            <p class="muted path-note" title={zip.path}>{zip.path}</p>
            <p class="muted mono">sha256: {zip.sha256}</p>
            {#if zip.violations.length > 0}
                <p class="note warn">
                    Not staged by the allowlist (public repo drift): {zip.violations.join(
                        ", ",
                    )}
                </p>
            {/if}
            <ul class="files" aria-label="Files in the release zip">
                {#each zip.entries as entry (entry.path)}
                    <li class="file">
                        <span class="path" title={entry.path}>
                            {entry.path}
                        </span>
                        <span class="meta">{formatBytes(entry.size)}</span>
                    </li>
                {/each}
            </ul>
        {/if}
    </section>

    <hr class="divider" />

    <section class="notes" aria-label="Release notes">
        <h3>
            Release notes
            <span class="muted">
                {notes
                    ? `${notesCommitCount} commit${notesCommitCount === 1 ? "" : "s"} since ${notes.fromTag ?? "the start"}`
                    : ""}
            </span>
        </h3>

        {#if notesError}
            <ErrorBox error={notesError} />
        {/if}

        <div class="notes-actions">
            <button
                type="button"
                class="apply"
                title="Regenerate the notes from the commits since the last tag"
                disabled={notesLoading}
                onclick={() => loadNotes(true)}
            >
                {notesLoading ? "Generating…" : "Regenerate"}
            </button>
            <button
                type="button"
                class="ghost"
                title="Copy the notes to the clipboard"
                disabled={!notesText}
                onclick={copyNotes}
            >
                Copy
            </button>
        </div>

        <textarea
            class="notes-editor"
            title="Release notes (markdown) — shown as a preview below"
            aria-label="Release notes (markdown)"
            bind:value={notesText}
            oninput={markNotesTouched}
            rows={10}></textarea>
        <pre class="notes-preview" aria-label="Notes preview">{notesText}</pre>
        {#if notes}
            <p class="muted note">
                Commits since {notes.fromTag ?? "the start of the branch"}.
                {notes.groups.length} group{notes.groups.length === 1
                    ? ""
                    : "s"}.
            </p>
        {/if}
    </section>

    <hr class="divider" />

    <section class="github" aria-label="GitHub releases">
        <h3>
            GitHub releases
            <span class="muted">public repo</span>
        </h3>

        {#if ghError}
            <ErrorBox error={ghError} />
        {/if}

        {#if ghLoading}
            <p class="muted">Reading the releases…</p>
        {:else if ghReleases}
            <div class="notes-actions">
                <label
                    class="push"
                    title="Mark newly created releases as pre-releases"
                >
                    <input
                        type="checkbox"
                        title="Mark newly created releases as pre-releases"
                        bind:checked={prerelease}
                    />
                    Pre-release
                </label>
                <button
                    type="button"
                    class="ghost"
                    title="Create a draft release on GitHub from the notes above"
                    disabled={publishing ||
                        !notesText.trim() ||
                        !!publishBlockedReason}
                    onclick={() => publish(true)}
                >
                    {publishing ? "Publishing…" : "Publish draft"}
                </button>
                <button
                    type="button"
                    class="apply"
                    title="Publish a release on GitHub from the notes above"
                    disabled={publishing ||
                        !notesText.trim() ||
                        !!publishBlockedReason}
                    onclick={() => publish(false)}
                >
                    {publishing ? "Publishing…" : "Publish release"}
                </button>
                {#if publishBlockedReason}
                    <p class="muted publish-gate" role="status">
                        {publishBlockedReason}
                    </p>
                {/if}
                <button
                    type="button"
                    class="ghost"
                    title="Refresh the release list (read-only)"
                    onclick={() => loadReleases()}
                >
                    Refresh
                </button>
            </div>

            {#if ghReleases.length === 0}
                <p class="muted">No releases on the public repo yet.</p>
            {:else}
                <ul class="gh-list" aria-label="Releases on the public repo">
                    {#each ghReleases as r (r.id)}
                        <li class="gh-release">
                            <div class="gh-head">
                                {#if editingId === r.id}
                                    <input
                                        class="gh-edit"
                                        title="Release name (the tag shown above this name)"
                                        aria-label="Release name"
                                        bind:value={editName}
                                    />
                                    <button
                                        type="button"
                                        class="ghost"
                                        title="Save the release name"
                                        disabled={savingId !== null}
                                        onclick={() => saveEdit(r)}
                                    >
                                        {savingId === r.id ? "Saving…" : "Save"}
                                    </button>
                                    <button
                                        type="button"
                                        class="ghost"
                                        title="Cancel the edit"
                                        onclick={cancelEdit}
                                    >
                                        Cancel
                                    </button>
                                {:else}
                                    <button
                                        type="button"
                                        class="gh-tag"
                                        title={r.html_url}
                                        onclick={() => openRelease(r)}
                                    >
                                        {r.name ?? r.tag_name}
                                    </button>
                                    {#if r.draft}<span
                                            class="badge"
                                            title="Draft release"
                                        >
                                            draft
                                        </span>{/if}
                                    {#if r.prerelease}<span
                                            class="badge"
                                            title="Pre-release"
                                        >
                                            pre-release
                                        </span>{/if}
                                    <button
                                        type="button"
                                        class="ghost"
                                        title="Edit the release name"
                                        onclick={() => startEdit(r)}
                                    >
                                        Edit
                                    </button>
                                {/if}
                            </div>
                            <p class="muted gh-meta">
                                {r.tag_name}
                                {#if r.published_at}
                                    · published {r.published_at.slice(0, 10)}
                                {/if}
                                · {r.assets.length} asset{r.assets.length === 1
                                    ? ""
                                    : "s"}
                            </p>
                            {#if r.assets.length > 0}
                                <ul
                                    class="gh-assets"
                                    aria-label="Release assets"
                                >
                                    {#each r.assets as a (a.id)}
                                        <li
                                            class="muted"
                                            title={a.browser_download_url}
                                        >
                                            {a.name} · {formatBytes(a.size)}
                                        </li>
                                    {/each}
                                </ul>
                            {/if}
                            <div class="gh-actions">
                                <button
                                    type="button"
                                    class="ghost"
                                    title="Replace the zip asset on this release with the current release zip"
                                    disabled={uploadingId !== null}
                                    onclick={() => upload(r)}
                                >
                                    {uploadingId === r.id
                                        ? "Uploading…"
                                        : "Upload zip"}
                                </button>
                                <button
                                    type="button"
                                    class="danger"
                                    title={armedDeleteId === r.id
                                        ? "Click again to delete this release"
                                        : "Delete this release"}
                                    onclick={() => remove(r)}
                                >
                                    {armedDeleteId === r.id
                                        ? "Confirm delete"
                                        : "Delete"}
                                </button>
                            </div>
                        </li>
                    {/each}
                </ul>
            {/if}
        {:else}
            <p class="muted">No releases loaded.</p>
        {/if}
    </section>

    <hr class="divider" />

    <section class="checklist" aria-label="Release checklist">
        <h3>Release checklist</h3>

        {#if checklistError}
            <ErrorBox error={checklistError} />
        {:else if checklistLoading && !checklist}
            <p class="muted">Reading the checklist…</p>
        {:else if checklist}
            <ul class="checklist-steps" aria-label="Release checklist steps">
                {#each checklist.steps as step (step.id)}
                    <li
                        class="checklist-step"
                        class:done={step.done}
                        class:pending={!step.done}
                        title={step.detail}
                    >
                        <span class="checklist-icon" aria-hidden="true">
                            {step.done ? "✓" : "○"}
                        </span>
                        <span class="checklist-tag">
                            {step.done ? "Done" : "Pending"}
                        </span>
                        <span class="checklist-label">{step.label}</span>
                        <span class="muted">{step.detail}</span>
                    </li>
                {/each}
            </ul>
            {#if checklist.nextId}
                <p class="muted checklist-next">
                    Next: <strong>{checklist.nextId}</strong>
                </p>
            {:else}
                <p class="checklist-done">All steps done.</p>
            {/if}
        {:else}
            <p class="muted">No checklist loaded.</p>
        {/if}
    </section>

    <hr class="divider" />

    <section class="history" aria-label="Release history">
        <h3>
            Release history <span class="muted">.proj</span>
        </h3>

        {#if historyError}
            <ErrorBox error={historyError} />
        {/if}

        <div class="history-actions">
            <button
                type="button"
                class="ghost"
                title="Record the current version's release in the history table (idempotent)"
                disabled={recording || historyLoading}
                onclick={recordHistory}
            >
                {recording ? "Recording…" : "Record current release"}
            </button>
        </div>

        {#if historyLoading && history.length === 0}
            <p class="muted">Reading the history…</p>
        {:else if history.length === 0}
            <p class="muted">
                No releases recorded yet. Tag a version to record it.
            </p>
        {:else}
            <ul class="history-list" aria-label="Recorded releases">
                {#each history as entry (entry.tag)}
                    <li class="history-entry">
                        <span class="history-tag">{entry.tag}</span>
                        <span class="muted">{entry.version}</span>
                        <span class="muted">{entry.date}</span>
                    </li>
                {/each}
            </ul>
        {/if}
    </section>
</section>

<style>
    .release {
        display: flex;
        flex-direction: column;
        gap: 0.6rem;
        padding: 0.75rem 0.9rem;
        min-width: 0;
    }
    .toolbar {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        flex-wrap: wrap;
    }
    .mode {
        display: flex;
        gap: 0.25rem;
    }
    .mode button {
        border-radius: 4px;
        padding: 0.3rem 0.65rem;
        font-size: 0.8rem;
        background: var(--surface);
        color: var(--text-muted);
        border: 1px solid var(--border);
        cursor: pointer;
    }
    .mode button:hover {
        color: var(--text);
    }
    .mode button.on {
        border-color: var(--text);
        color: var(--text);
        background: var(--canvas);
    }
    .exact {
        width: 6.5rem;
        border-radius: 4px;
        border: 1px solid var(--border);
        background: var(--surface);
        color: var(--text);
        padding: 0.3rem 0.45rem;
        font-size: 0.85em;
        font-variant-numeric: tabular-nums;
    }
    .apply {
        margin-left: auto;
        border-radius: 5px;
        padding: 0.35rem 0.9rem;
        font-weight: 600;
        background: var(--accent);
        color: var(--accent-on, #fff);
        border: 1px solid var(--accent);
        cursor: pointer;
    }
    .apply:disabled {
        opacity: 0.5;
        cursor: default;
    }
    .note {
        margin: 0;
        font-size: 0.9em;
    }
    .note.applied {
        color: var(--text);
    }
    .note.muted {
        color: var(--text-muted);
    }
    .files {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 0.25rem;
    }
    .file {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        padding: 0.35rem 0.5rem;
        background: var(--surface);
        border: 1px solid var(--border);
        border-radius: 4px;
        min-width: 0;
    }
    .path {
        flex: 1 1 auto;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-family: var(--mono, monospace);
        font-size: 0.8em;
    }
    .meta {
        flex: none;
        color: var(--text-muted);
        font-size: 0.8em;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .muted {
        color: var(--text-muted);
    }
    .divider {
        border: 0;
        border-top: 1px solid var(--border);
        margin: 0.25rem 0;
    }
    .tag h3 {
        margin: 0;
        font-size: 0.85em;
        font-weight: 600;
        display: flex;
        align-items: baseline;
        gap: 0.4rem;
    }
    .tag h3 .muted {
        font-weight: 400;
        font-size: 0.85em;
    }
    .tag {
        display: flex;
        flex-direction: column;
        gap: 0.45rem;
        min-width: 0;
    }
    .tag-actions {
        display: flex;
        align-items: center;
        gap: 0.6rem;
        flex-wrap: wrap;
    }
    .tag-actions .apply {
        margin-left: auto;
    }
    .push {
        display: inline-flex;
        align-items: center;
        gap: 0.35rem;
        font-size: 0.85em;
        color: var(--text-muted);
        cursor: pointer;
    }
    .push input {
        accent-color: var(--accent);
    }
    .zip h3 {
        margin: 0;
        font-size: 0.85em;
        font-weight: 600;
        display: flex;
        align-items: baseline;
        gap: 0.4rem;
    }
    .zip h3 .muted {
        font-weight: 400;
        font-size: 0.85em;
    }
    .zip {
        display: flex;
        flex-direction: column;
        gap: 0.45rem;
        min-width: 0;
    }
    .zip-actions {
        display: flex;
        gap: 0.6rem;
        align-items: center;
    }
    .note.warn {
        color: var(--warn, #d98a00);
        font-size: 0.85em;
    }
    .mono {
        font-family: var(--mono, monospace);
        font-size: 0.75em;
        word-break: break-all;
    }
    .path-note {
        font-size: 0.75em;
        word-break: break-all;
        margin: 0;
        font-family: var(--mono, monospace);
    }
    .notes {
        display: flex;
        flex-direction: column;
        gap: 0.45rem;
        min-width: 0;
    }
    .notes h3 {
        margin: 0;
        font-size: 0.85em;
        font-weight: 600;
        display: flex;
        align-items: baseline;
        gap: 0.4rem;
    }
    .notes h3 .muted {
        font-weight: 400;
        font-size: 0.85em;
    }
    .notes-actions {
        display: flex;
        gap: 0.5rem;
        align-items: center;
    }
    .notes-actions .apply {
        margin-left: 0;
    }
    .ghost {
        border-radius: 5px;
        padding: 0.35rem 0.9rem;
        background: var(--surface);
        color: var(--text-muted);
        border: 1px solid var(--border);
        cursor: pointer;
        font-size: 0.85em;
    }
    .ghost:hover:not(:disabled) {
        color: var(--text);
    }
    .ghost:disabled {
        opacity: 0.5;
        cursor: default;
    }
    .notes-editor {
        width: 100%;
        min-width: 0;
        box-sizing: border-box;
        resize: vertical;
        border-radius: 5px;
        border: 1px solid var(--border);
        background: var(--surface);
        color: var(--text);
        padding: 0.5rem 0.6rem;
        font-family: var(--mono, monospace);
        font-size: 0.8em;
        line-height: 1.45;
        tab-size: 4;
    }
    .notes-preview {
        margin: 0;
        min-width: 0;
        max-height: 16rem;
        overflow: auto;
        white-space: pre-wrap;
        word-break: break-word;
        background: var(--canvas);
        border: 1px solid var(--border);
        border-radius: 5px;
        padding: 0.5rem 0.6rem;
        font-family: var(--mono, monospace);
        font-size: 0.78em;
        line-height: 1.45;
        color: var(--text-muted);
    }
    .notes-editor:focus-visible,
    .ghost:focus-visible,
    .apply:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: 1px;
    }

    .gh-list {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 0.6rem;
    }
    .gh-release {
        border: 1px solid var(--border);
        border-radius: 6px;
        padding: 0.5rem 0.6rem;
        display: flex;
        flex-direction: column;
        gap: 0.3rem;
    }
    .gh-head {
        display: flex;
        align-items: center;
        gap: 0.4rem;
        flex-wrap: wrap;
    }
    .gh-tag {
        color: var(--text);
        cursor: pointer;
        text-decoration: none;
        font-weight: 600;
        background: none;
        border: none;
        padding: 0;
        font-family: inherit;
        font-size: inherit;
        text-align: left;
    }
    .gh-tag:hover {
        text-decoration: underline;
    }
    .gh-tag:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: 1px;
    }
    .badge {
        border-radius: 999px;
        padding: 0.1rem 0.55rem;
        font-size: 0.72em;
        background: var(--surface);
        color: var(--text-muted);
        border: 1px solid var(--border);
    }
    .gh-meta {
        margin: 0;
        font-size: 0.8em;
    }
    .gh-assets {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 0.1rem;
        font-size: 0.78em;
    }
    .gh-actions {
        display: flex;
        gap: 0.4rem;
        justify-content: flex-end;
    }
    .gh-actions .ghost,
    .gh-actions .danger {
        margin-left: 0;
        font-size: 0.8em;
        padding: 0.25rem 0.6rem;
    }
    .gh-edit {
        flex: 1;
        min-width: 0;
        box-sizing: border-box;
        border-radius: 5px;
        border: 1px solid var(--border);
        background: var(--surface);
        color: var(--text);
        padding: 0.3rem 0.5rem;
        font-size: 0.9em;
    }
    .gh-edit:focus-visible,
    .gh-tag:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: 1px;
    }
    .checklist-steps {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 0.3rem;
    }
    .checklist-step {
        display: flex;
        align-items: baseline;
        gap: 0.45rem;
        font-size: 0.85em;
    }
    .checklist-icon {
        width: 1em;
        text-align: center;
        flex-shrink: 0;
    }
    .checklist-step.done .checklist-icon {
        color: var(--accent);
    }
    .checklist-step.pending .checklist-icon {
        color: var(--text-muted);
    }
    .checklist-tag {
        flex: none;
        text-transform: uppercase;
        font-size: 0.7em;
        font-weight: 700;
        letter-spacing: 0.04em;
        color: var(--text-muted);
    }
    .checklist-step.done .checklist-tag {
        color: var(--accent);
    }
    .checklist-label {
        font-weight: 600;
        color: var(--text);
    }
    .checklist-next {
        margin: 0.4rem 0 0;
        font-size: 0.8em;
    }
    .checklist-done {
        margin: 0;
        color: var(--accent);
        font-weight: 600;
        font-size: 0.9em;
    }
    .history-actions {
        display: flex;
        gap: 0.4rem;
        margin-bottom: 0.35rem;
    }
    .history-actions .ghost {
        font-size: 0.8em;
    }
    .history-list {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 0.3rem;
    }
    .history-entry {
        display: flex;
        align-items: baseline;
        gap: 0.6rem;
        border: 1px solid var(--border);
        border-radius: 6px;
        padding: 0.35rem 0.55rem;
        font-size: 0.85em;
    }
    .history-tag {
        font-weight: 600;
        color: var(--text);
        font-family: ui-monospace, monospace;
    }
</style>
