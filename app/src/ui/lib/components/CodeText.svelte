<script lang="ts">
    // One line of code: syntax colors and changed-word emphasis, as plain text spans.
    import { mergeSegments, type Token } from "../history";

    let {
        text,
        tokens = null,
        emphasis = [],
    }: {
        text: string;
        tokens?: Token[] | null;
        emphasis?: readonly (readonly [number, number])[];
    } = $props();

    const segments = $derived(mergeSegments(text, tokens, emphasis));
</script>

{#each segments as s, i (i)}<span class:em={s.emph} style:color={s.color}>
        {s.text}
    </span>{/each}

<style>
    .em {
        border-radius: 2px;
        background: var(--code-emphasis);
    }
</style>
