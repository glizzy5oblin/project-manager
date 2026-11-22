// Find in the Files editor: where the matches are and which one comes next. Pure.

/** Start offsets of every match of `query` in `text`, in order. No overlaps. */
export function findAll(
    text: string,
    query: string,
    matchCase: boolean,
): number[] {
    if (!query) return [];
    const hay = matchCase ? text : text.toLowerCase();
    const needle = matchCase ? query : query.toLowerCase();
    const out: number[] = [];
    for (
        let i = hay.indexOf(needle);
        i !== -1;
        i = hay.indexOf(needle, i + needle.length)
    )
        out.push(i);
    return out;
}

/** The match to go to from `from` (a caret offset): the first at or after it, wrapping round.
 *  Backwards, the last one that starts before it. Null when there are no matches. */
export function pickMatch(
    matches: number[],
    from: number,
    forward: boolean,
): number | null {
    if (matches.length === 0) return null;
    if (forward) return matches.find((m) => m >= from) ?? matches[0];
    return (
        [...matches].reverse().find((m) => m < from) ??
        matches[matches.length - 1]
    );
}

/** The 0-based line an offset is on. */
export const lineOf = (text: string, offset: number) => {
    let n = 0;
    for (
        let i = text.indexOf("\n");
        i !== -1 && i < offset;
        i = text.indexOf("\n", i + 1)
    )
        n++;
    return n;
};
