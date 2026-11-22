<script lang="ts">
    // Browse the project's folders and edit text files. Nothing is written until Save; switching
    // away from unsaved edits asks first. The file's own text is shown as it is on disk.
    import { api } from "../api";
    import type { AppError } from "../bindings/AppError";
    import type { FileEntry } from "../bindings/FileEntry";
    import { findAll, lineOf, pickMatch } from "../fileSearch";
    import { highlight } from "../highlight";
    import { langForPath, type Token } from "../history";
    import ErrorBox from "./ErrorBox.svelte";

    let {
        projectId,
        changed,
        path = $bindable(""),
    }: {
        projectId: string;
        /** Bumped when the project's files change on disk. */
        changed: number;
        /** The open file; set from outside to open one. */
        path: string;
    } = $props();

    let dirs = $state<Record<string, FileEntry[]>>({});
    let truncated = $state<Record<string, boolean>>({});
    let expanded = $state<Record<string, boolean>>({});
    let error = $state<AppError | null>(null);
    /** The open file's contents on disk, and the text in the editor. */
    let loaded = $state("");
    let draft = $state("");
    let why = $state<string | null>(null);
    let saving = $state(false);
    let notice = $state("");
    /** A file the user clicked while edits were unsaved; waits for their answer. */
    let asking = $state<string | null>(null);
    let shown = $state("");
    let request = 0;

    // Syntax colors: the text the tokens were made for is kept, so stale colors are never shown
    // over different text.
    let tokens = $state<{ text: string; lines: Token[][] } | null>(null);
    let editorEl = $state<HTMLTextAreaElement>();
    let underEl = $state<HTMLPreElement>();
    // A line break between the colored lines (the pre must hold exactly the text, no layout).
    const NL = "\n";
    $effect(() => {
        const [text, lang] = [draft, langForPath(shown)];
        if (!lang || why !== null) {
            tokens = null;
            return;
        }
        const t = setTimeout(async () => {
            const lines = await highlight(text, lang);
            tokens = lines ? { text, lines } : null;
        }, 120);
        return () => clearTimeout(t);
    });
    const colored = $derived(
        tokens && tokens.text === draft ? tokens.lines : null,
    );
    let gutterEl = $state<HTMLPreElement>();
    function syncScroll() {
        if (!editorEl || !underEl) return;
        underEl.scrollTop = editorEl.scrollTop;
        underEl.scrollLeft = editorEl.scrollLeft;
        if (gutterEl) gutterEl.scrollTop = editorEl.scrollTop;
    }
    // Line numbers, one per line of the text (a final newline does not add a numbered line).
    const lineCount = $derived(Math.max(1, draft.split("\n").length));
    const numbers = $derived(
        Array.from({ length: lineCount }, (_, i) => i + 1).join("\n"),
    );

    // Find in the open file.
    let finding = $state(false);
    let query = $state("");
    let matchCase = $state(false);
    let findEl = $state<HTMLInputElement>();
    const matches = $derived(finding ? findAll(draft, query, matchCase) : []);
    let current = $state<number | null>(null);
    function go(forward: boolean) {
        const ta = editorEl;
        if (!ta || matches.length === 0) return;
        const from = forward ? ta.selectionEnd : ta.selectionStart;
        const at = pickMatch(matches, from, forward);
        if (at === null) return;
        current = matches.indexOf(at);
        ta.setSelectionRange(at, at + query.length);
        // Scroll the match into view, centred when it was off screen.
        const line = lineOf(draft, at);
        const lh = parseFloat(getComputedStyle(ta).lineHeight) || 18;
        const top = line * lh;
        if (top < ta.scrollTop || top > ta.scrollTop + ta.clientHeight - lh * 2)
            ta.scrollTop = Math.max(0, top - ta.clientHeight / 2);
        syncScroll();
    }
    function openFind() {
        finding = true;
        const sel = editorEl
            ? draft.slice(editorEl.selectionStart, editorEl.selectionEnd)
            : "";
        if (sel && !sel.includes("\n")) query = sel;
        queueMicrotask(() => {
            findEl?.focus();
            findEl?.select();
        });
    }
    function closeFind() {
        finding = false;
        current = null;
        editorEl?.focus();
    }

    // Make a new file or folder, named relative to the project.
    let creating = $state<null | "file" | "folder">(null);
    let newName = $state("");
    async function create() {
        const name = newName.trim().replace(/^\/+/, "");
        if (!name || !creating) return;
        const dir = creating === "folder";
        try {
            await api.fileCreate(projectId, name, dir);
            creating = null;
            newName = "";
            error = null;
            await loadDir("");
            const parts = name.split("/");
            const folders = dir ? parts.length : parts.length - 1;
            for (let i = 1; i <= folders; i++) {
                const d = parts.slice(0, i).join("/");
                await loadDir(d);
                expanded[d] = true;
            }
            if (!dir) await openFile(name);
        } catch (e) {
            error = e as AppError;
        }
    }

    const dirty = $derived(!!shown && why === null && draft !== loaded);

    async function loadDir(dir: string) {
        try {
            const l = await api.filesList(projectId, dir);
            dirs[dir] = l.entries;
            truncated[dir] = l.truncated;
        } catch (e) {
            error = e as AppError;
        }
    }

    async function openFile(p: string) {
        const mine = ++request;
        try {
            const f = await api.fileRead(projectId, p);
            if (mine !== request) return;
            shown = p;
            loaded = f.text ?? "";
            draft = loaded;
            why = f.reason;
            error = null;
            notice = "";
            // Show the file in the tree: open each folder on its way.
            const parts = p.split("/");
            for (let i = 1; i < parts.length; i++) {
                const d = parts.slice(0, i).join("/");
                if (!dirs[d]) await loadDir(d);
                expanded[d] = true;
            }
        } catch (e) {
            if (mine === request) error = e as AppError;
        }
    }

    function choose(p: string) {
        if (p === shown) return;
        if (dirty) asking = p;
        else void openFile(p);
    }

    async function toggle(e: FileEntry) {
        if (!e.dir) return choose(e.path);
        if (!dirs[e.path]) await loadDir(e.path);
        expanded[e.path] = !expanded[e.path];
    }

    async function save() {
        if (!dirty || saving) return;
        saving = true;
        try {
            await api.fileWrite(projectId, shown, draft);
            loaded = draft;
            notice = "Saved.";
            error = null;
        } catch (e) {
            error = e as AppError;
        } finally {
            saving = false;
        }
    }

    /** The visible tree, in order, with each row's depth. */
    const rows = $derived.by(() => {
        const out: { e: FileEntry; depth: number }[] = [];
        const walk = (dir: string, depth: number) => {
            for (const e of dirs[dir] ?? []) {
                out.push({ e, depth });
                if (e.dir && expanded[e.path]) walk(e.path, depth + 1);
            }
        };
        walk("", 0);
        return out;
    });

    // Open the file asked for from another tab (and again when it asks for another one).
    $effect(() => {
        const p = path;
        if (p && p !== shown) {
            if (dirty) asking = p;
            else void openFile(p);
        }
    });

    // Reload the tree when files change on disk; the open file follows unless it has edits.
    $effect(() => {
        void changed;
        void projectId;
        void loadDir("");
        for (const d of Object.keys(expanded)) if (expanded[d]) void loadDir(d);
        if (shown && !dirty && !asking) void openFile(shown);
    });

    function onkey(e: KeyboardEvent) {
        if (!(e.ctrlKey || e.metaKey)) return;
        const k = e.key.toLowerCase();
        if (k === "s") {
            e.preventDefault();
            void save();
        } else if (k === "f") {
            e.preventDefault();
            openFind();
        }
    }

    const reasons: Record<string, string> = {
        binary: "This file is not text, so it can't be edited here.",
        large: "This file is too large to edit here (over 2 MiB).",
        encoding: "This file is not UTF-8 text, so it can't be edited here.",
    };
</script>

<div class="files">
    <nav class="tree" aria-label="Project files">
        <div class="tools">
            <button
                type="button"
                title="Make a new empty file. Type its path, such as notes/todo.md"
                onclick={() => ((creating = "file"), (newName = ""))}
            >
                New file
            </button>
            <button
                type="button"
                title="Make a new folder. Type its path, such as docs/images"
                onclick={() => ((creating = "folder"), (newName = ""))}
            >
                New folder
            </button>
        </div>
        {#if creating}
            <form
                class="new"
                onsubmit={(e) => {
                    e.preventDefault();
                    void create();
                }}
            >
                <!-- svelte-ignore a11y_autofocus -->
                <input
                    bind:value={newName}
                    autofocus
                    placeholder={creating === "file"
                        ? "path/of/file.txt"
                        : "path/of/folder"}
                    aria-label={`Name of the new ${creating}`}
                    title="The path relative to the project. Folders on the way are made too"
                    onkeydown={(e) => e.key === "Escape" && (creating = null)}
                />
                <button
                    type="submit"
                    disabled={!newName.trim()}
                    title="Make it"
                >
                    Make
                </button>
                <button
                    type="button"
                    title="Cancel"
                    onclick={() => (creating = null)}
                >
                    ✕
                </button>
            </form>
        {/if}
        {#each rows as { e, depth } (e.path)}
            <button
                type="button"
                class="row"
                class:sel={e.path === shown}
                style:padding-left={`${0.5 + depth * 0.9}rem`}
                title={e.dir
                    ? `Open or close the folder ${e.path}`
                    : `Edit ${e.path}`}
                onclick={() => toggle(e)}
            >
                <span class="mark" aria-hidden="true">
                    {e.dir ? (expanded[e.path] ? "▾" : "▸") : ""}
                </span>
                {e.name}
            </button>
        {/each}
        {#if truncated[""]}
            <p class="muted pad">The list was cut short: too many entries.</p>
        {/if}
    </nav>
    <div class="editor">
        {#if error}
            <div class="pad"><ErrorBox {error} /></div>
        {/if}
        {#if asking}
            <div class="ask" role="alert">
                <span>You have unsaved edits to <code>{shown}</code>.</span>
                <button
                    type="button"
                    title="Throw away the edits and open the other file"
                    onclick={() => {
                        const p = asking!;
                        asking = null;
                        void openFile(p);
                    }}
                >
                    Discard and open
                </button>
                <button
                    type="button"
                    title="Stay on this file"
                    onclick={() => {
                        asking = null;
                        path = shown;
                    }}
                >
                    Keep editing
                </button>
            </div>
        {/if}
        {#if shown}
            <header>
                <code>{shown}</code>
                {#if dirty}<span class="dirty">unsaved</span>{/if}
                <span class="grow"></span>
                {#if notice && !dirty}<span class="muted" role="status">
                        {notice}
                    </span>{/if}
                <button
                    type="button"
                    disabled={why !== null}
                    title="Find text in this file (Ctrl+F)"
                    onclick={openFind}
                >
                    Find
                </button>
                <button
                    type="button"
                    class="primary"
                    disabled={!dirty || saving}
                    title="Write the text to the file (Ctrl+S)"
                    onclick={save}
                >
                    Save
                </button>
                <button
                    type="button"
                    disabled={!dirty}
                    title="Put the text back as it is on disk"
                    onclick={() => (draft = loaded)}
                >
                    Revert
                </button>
            </header>
            {#if finding && !why}
                <div class="find" role="search">
                    <input
                        bind:this={findEl}
                        bind:value={query}
                        aria-label="Find in this file"
                        placeholder="Find"
                        title="Text to find. Enter goes to the next match, Shift+Enter to the previous, Esc closes"
                        oninput={() => (current = null)}
                        onkeydown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                go(!e.shiftKey);
                            } else if (e.key === "Escape") {
                                e.preventDefault();
                                e.stopPropagation();
                                closeFind();
                            }
                        }}
                    />
                    <span class="muted" role="status">
                        {#if !query}{:else if matches.length === 0}No matches{:else}{current ===
                            null
                                ? matches.length
                                : `${current + 1} of ${matches.length}`}{/if}
                    </span>
                    <button
                        type="button"
                        title="Previous match (Shift+Enter)"
                        disabled={!matches.length}
                        onclick={() => go(false)}
                    >
                        ↑
                    </button>
                    <button
                        type="button"
                        title="Next match (Enter)"
                        disabled={!matches.length}
                        onclick={() => go(true)}
                    >
                        ↓
                    </button>
                    <label title="Only match the same upper and lower case">
                        <input
                            type="checkbox"
                            title="Only match the same upper and lower case"
                            bind:checked={matchCase}
                        /> Match case
                    </label>
                    <button
                        type="button"
                        title="Close the find bar (Esc)"
                        onclick={closeFind}
                    >
                        ✕
                    </button>
                </div>
            {/if}
            {#if why}
                <p class="muted pad">
                    {reasons[why] ?? "This file can't be edited."}
                </p>
            {:else}
                <div
                    class="code"
                    style:--gw={`${String(lineCount).length + 2}ch`}
                >
                    <!-- prettier-ignore -->
                    <pre class="gutter" bind:this={gutterEl} aria-hidden="true">{numbers}</pre>
                    <!-- prettier-ignore -->
                    <pre class="under" bind:this={underEl} aria-hidden="true">{#if colored}{#each colored as line, i (i)}{#each line as t, j (j)}<span style:color={t.color}>{t.content}</span>{/each}{NL}{/each}{:else}{draft}{NL}{/if}</pre>
                    <textarea
                        bind:this={editorEl}
                        bind:value={draft}
                        onkeydown={onkey}
                        onscroll={syncScroll}
                        spellcheck="false"
                        wrap="off"
                        class:lit={colored !== null}
                        aria-label={`Contents of ${shown}`}
                        title="The file's text. Ctrl+S saves, Ctrl+F finds"
                    ></textarea>
                </div>
            {/if}
        {:else}
            <p class="muted pad">Pick a file on the left to edit it.</p>
        {/if}
    </div>
</div>

<style>
    .files {
        display: grid;
        grid-template-columns: minmax(10rem, 18rem) 1fr;
        height: 100%;
        min-height: 0;
    }
    .tree {
        overflow: auto;
        border-right: 1px solid var(--border, #8884);
        padding: 0.25rem 0;
    }
    .row {
        display: block;
        width: 100%;
        text-align: left;
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        padding: 0.2rem 0.5rem;
        white-space: nowrap;
        cursor: pointer;
    }
    .row:hover {
        background: #8882;
    }
    .row.sel {
        background: #4a90e033;
    }
    .mark {
        display: inline-block;
        width: 1em;
    }
    .editor {
        display: flex;
        flex-direction: column;
        min-width: 0;
        min-height: 0;
    }
    header {
        display: flex;
        gap: 0.5rem;
        align-items: center;
        padding: 0.4rem 0.75rem;
        border-bottom: 1px solid var(--border, #8884);
    }
    .grow {
        flex: 1;
    }
    .dirty {
        color: #c80;
    }
    .ask {
        display: flex;
        gap: 0.5rem;
        align-items: center;
        padding: 0.4rem 0.75rem;
        background: #c802;
    }
    .code {
        position: relative;
        flex: 1;
        min-height: 0;
    }
    /* The colored copy sits under a transparent textarea; both use the same box and font. */
    .code {
        font-family: ui-monospace, "Cascadia Mono", "Segoe UI Mono", monospace;
        font-size: 0.85rem;
        line-height: 1.4;
        tab-size: 4;
    }
    .gutter,
    .under,
    textarea {
        position: absolute;
        inset: 0;
        margin: 0;
        padding: 0.5rem 0.75rem;
        border: 0;
        overflow: auto;
        font: inherit;
        line-height: inherit;
        tab-size: inherit;
        white-space: pre;
        word-wrap: normal;
    }
    .gutter {
        right: auto;
        width: var(--gw);
        box-sizing: content-box;
        padding-right: 0.5rem;
        padding-left: 0.5rem;
        overflow: hidden;
        text-align: right;
        color: var(--text-muted);
        border-right: 1px solid var(--border, #8884);
        user-select: none;
        pointer-events: none;
    }
    .under,
    textarea {
        padding-left: calc(var(--gw) + 1rem + 1px + 0.75rem);
    }
    .under {
        pointer-events: none;
        overflow: hidden;
        color: var(--text);
    }
    textarea {
        resize: none;
        background: transparent;
        color: inherit;
    }
    textarea.lit {
        color: transparent;
        caret-color: var(--text);
    }
    .tools,
    .new,
    .find {
        display: flex;
        gap: 0.35rem;
        align-items: center;
        padding: 0.35rem 0.5rem;
    }
    .new input,
    .find input:not([type="checkbox"]) {
        flex: 1;
        min-width: 0;
    }
    textarea:focus {
        outline: none;
    }
    .pad {
        padding: 0.75rem;
    }
</style>
