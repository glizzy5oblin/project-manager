// The app is for people new to git, so every button, field and menu has a hover description in
// plain words. This reads the components' markup and fails on a control that has none.
import { describe, expect, it } from "vitest";

const components = import.meta.glob<string>("./components/*.svelte", {
    query: "?raw",
    import: "default",
    eager: true,
});

/** The opening tags of the controls in a component's markup that carry no `title`. */
function untitled(source: string): string[] {
    const end = source.indexOf("<style");
    const body = end < 0 ? source : source.slice(0, end);
    const markup = body.slice(body.indexOf("</script>"));
    const found: string[] = [];
    for (const m of markup.matchAll(
        /<(button|input|select|textarea|summary)\b/g,
    )) {
        let depth = 0;
        let j = m.index + m[0].length;
        for (; j < markup.length; j++) {
            const c = markup[j];
            if (c === "{") depth++;
            else if (c === "}") depth--;
            else if (c === ">" && depth === 0) break;
        }
        const tag = markup.slice(m.index, j);
        if (!/\btitle=/.test(tag)) {
            const next = markup
                .slice(j + 1, j + 60)
                .replace(/\s+/g, " ")
                .trim();
            found.push(`${tag.replace(/\s+/g, " ").slice(0, 60)} … ${next}`);
        }
    }
    return found;
}

describe("hover descriptions", () => {
    for (const [file, source] of Object.entries(components)) {
        it(`${file}: every control has a title`, () => {
            expect(untitled(source)).toEqual([]);
        });
    }
});
