<script lang="ts">
    // A folder path chosen with the system folder picker. Typing is not offered on purpose: the
    // backend only receives paths the user picked.
    import { open } from "@tauri-apps/plugin-dialog";

    let {
        label,
        value = $bindable(""),
        title,
    }: { label: string; value: string; title: string } = $props();

    async function pick() {
        const chosen = await open({ directory: true, multiple: false, title });
        if (typeof chosen === "string") value = chosen;
    }
</script>

<div class="field">
    <span class="label">{label}</span>
    <div class="row">
        <span class="path" class:empty={!value} title={value}>
            <!-- The box is right-to-left so a long path is cut at its start; the marks keep the
                 path's own characters (a leading "/") in left-to-right order. -->
            {value ? `\u200e${value}\u200e` : "No folder chosen"}
        </span>
        <button type="button" onclick={pick}>Choose…</button>
    </div>
</div>

<style>
    .field {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: 0.3rem;
    }
    .label {
        color: var(--text-muted);
        font-size: 0.9em;
    }
    .row {
        display: flex;
        gap: 0.5rem;
        align-items: center;
    }
    .path {
        flex: 1;
        min-width: 0;
        padding: 0.4rem 0.5rem;
        overflow: hidden;
        border: 1px solid var(--border);
        border-radius: 4px;
        text-overflow: ellipsis;
        white-space: nowrap;
        direction: rtl;
        text-align: left;
    }
    .empty {
        color: var(--text-muted);
        direction: ltr;
    }
</style>
