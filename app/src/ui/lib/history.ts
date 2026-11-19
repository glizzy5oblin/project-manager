// Pure helpers for the history views: the row cache, graph geometry, diff layout, and merging
// syntax tokens with word emphasis. Kept free of Svelte and the DOM so they are unit tested.

import type { DiffLine } from "./bindings/DiffLine";
import type { GraphRow } from "./bindings/GraphRow";

/** Rows fetched per request. */
export const CHUNK = 200;

/**
 * Graph rows fetched in chunks on demand, keeping the most recently used `max` chunks.
 * `get` returns undefined for a row not loaded yet and starts loading it; `onload` runs when a
 * chunk arrives so the view can redraw.
 */
export class RowCache {
    private chunks = new Map<number, GraphRow[]>();
    private pending = new Map<number, Promise<void>>();
    private closed = false;

    constructor(
        private load: (start: number, count: number) => Promise<GraphRow[]>,
        private onload: () => void,
        private onerror: (e: unknown) => void,
        private max = 60,
    ) {}

    get(row: number): GraphRow | undefined {
        const c = Math.floor(row / CHUNK);
        const chunk = this.chunks.get(c);
        if (!chunk) {
            void this.fetch(c);
            return undefined;
        }
        // Most recently used last.
        this.chunks.delete(c);
        this.chunks.set(c, chunk);
        return chunk[row - c * CHUNK];
    }

    /** The row, loading it first if needed. */
    async row(row: number): Promise<GraphRow | undefined> {
        const c = Math.floor(row / CHUNK);
        if (!this.chunks.has(c)) await this.fetch(c);
        return this.chunks.get(c)?.[row - c * CHUNK];
    }

    /** Stop delivering results (the graph was replaced). */
    close() {
        this.closed = true;
        this.chunks.clear();
    }

    get size(): number {
        return this.chunks.size;
    }

    private fetch(c: number): Promise<void> {
        const running = this.pending.get(c);
        if (running) return running;
        const p = this.load(c * CHUNK, CHUNK)
            .then((rows) => {
                if (this.closed) return;
                this.chunks.set(c, rows);
                while (this.chunks.size > this.max) {
                    const oldest = this.chunks.keys().next().value as number;
                    this.chunks.delete(oldest);
                }
                this.onload();
            })
            .catch((e: unknown) => {
                if (!this.closed) this.onerror(e);
            })
            .finally(() => this.pending.delete(c));
        this.pending.set(c, p);
        return p;
    }
}

/** Theme variable for a lane color index (the theme defines eight). */
export function graphColorVar(color: number): string {
    return `--graph-${(color % 8) + 1}`;
}

/** Centre of a lane, in CSS pixels from the left of the graph column. */
export function laneX(lane: number, laneWidth: number): number {
    return 6 + lane * laneWidth + laneWidth / 2;
}

/** Width of the graph column: every lane, but never more than `cap` pixels. */
export function graphWidth(
    maxLanes: number,
    laneWidth: number,
    cap = 320,
): number {
    return Math.min(cap, 12 + Math.max(1, maxLanes) * laneWidth);
}

/** Rows `[first, last]` (inclusive) that intersect the viewport, plus `overscan` either side. */
export function visibleRange(
    scrollTop: number,
    height: number,
    rowHeight: number,
    count: number,
    overscan = 0,
): [number, number] {
    if (count <= 0) return [0, -1];
    const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
    const last = Math.min(
        count - 1,
        Math.ceil((scrollTop + height) / rowHeight) + overscan,
    );
    return [first, last];
}

/** The scrollTop that brings row `i` into view, or the current one when it already shows. */
export function scrollToShow(
    i: number,
    scrollTop: number,
    height: number,
    rowHeight: number,
): number {
    const top = i * rowHeight;
    if (top < scrollTop) return top;
    if (top + rowHeight > scrollTop + height) return top + rowHeight - height;
    return scrollTop;
}

/** First index in a sorted array whose value is at least `v`. */
export function lowerBound(sorted: readonly number[], v: number): number {
    let lo = 0;
    let hi = sorted.length;
    while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (sorted[mid] < v) lo = mid + 1;
        else hi = mid;
    }
    return lo;
}

export type SplitRow = { left: DiffLine | null; right: DiffLine | null };

/**
 * Side-by-side layout of a hunk's lines: context lines on both sides, and each run of deleted
 * lines paired line by line with the added lines that follow it.
 */
export function splitRows(lines: readonly DiffLine[]): SplitRow[] {
    const out: SplitRow[] = [];
    let i = 0;
    while (i < lines.length) {
        if (lines[i].kind === "context") {
            out.push({ left: lines[i], right: lines[i] });
            i++;
            continue;
        }
        const del: DiffLine[] = [];
        const add: DiffLine[] = [];
        while (i < lines.length && lines[i].kind === "deleted")
            del.push(lines[i++]);
        while (i < lines.length && lines[i].kind === "added")
            add.push(lines[i++]);
        for (let k = 0; k < Math.max(del.length, add.length); k++) {
            out.push({ left: del[k] ?? null, right: add[k] ?? null });
        }
    }
    return out;
}

/** A syntax token: its text and (optionally) a color. */
export type Token = { content: string; color?: string };

/** A run of a line with one color and emphasis. */
export type Segment = { text: string; color: string | null; emph: boolean };

/**
 * Only colors the highlighter's CSS-variables theme produces, or plain hex, reach a style
 * attribute.
 */
export function safeColor(c: string | undefined): string | null {
    if (!c) return null;
    if (/^var\(--shiki-[a-z0-9-]+\)$/.test(c)) return c;
    if (/^#[0-9a-fA-F]{3,8}$/.test(c)) return c;
    return null;
}

/**
 * Split a line into segments at every token and emphasis boundary. `emphasis` holds `[start,
 * end)` UTF-16 offsets (JavaScript string indices). Tokens that do not spell out `text` exactly
 * are ignored, so a highlighter mismatch never changes what is shown.
 */
export function mergeSegments(
    text: string,
    tokens: readonly Token[] | null | undefined,
    emphasis: readonly (readonly [number, number])[],
): Segment[] {
    let toks: readonly Token[] = [{ content: text }];
    if (
        tokens &&
        tokens.length > 0 &&
        tokens.map((t) => t.content).join("") === text
    ) {
        toks = tokens;
    }
    const cuts = new Set<number>([0, text.length]);
    let pos = 0;
    for (const t of toks) {
        pos += t.content.length;
        cuts.add(pos);
    }
    for (const [a, b] of emphasis) {
        cuts.add(Math.max(0, Math.min(text.length, a)));
        cuts.add(Math.max(0, Math.min(text.length, b)));
    }
    const points = [...cuts].sort((a, b) => a - b);
    const out: Segment[] = [];
    let ti = 0;
    let tokStart = 0;
    for (let k = 0; k + 1 < points.length; k++) {
        const [a, b] = [points[k], points[k + 1]];
        while (
            ti < toks.length - 1 &&
            tokStart + toks[ti].content.length <= a
        ) {
            tokStart += toks[ti].content.length;
            ti++;
        }
        const color = safeColor(toks[ti]?.color);
        const emph = emphasis.some(([s, e]) => s <= a && b <= e);
        const prev = out[out.length - 1];
        if (prev && prev.color === color && prev.emph === emph) {
            prev.text += text.slice(a, b);
        } else {
            out.push({ text: text.slice(a, b), color, emph });
        }
    }
    return out;
}

/** Start of a local calendar day (`YYYY-MM-DD`), in seconds since the epoch. */
export function dayStart(day: string): number | null {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
    if (!m) return null;
    return Math.floor(new Date(+m[1], +m[2] - 1, +m[3]).getTime() / 1000);
}

/** Last second of a local calendar day. */
export function dayEnd(day: string): number | null {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
    if (!m) return null;
    return (
        Math.floor(new Date(+m[1], +m[2] - 1, +m[3] + 1).getTime() / 1000) - 1
    );
}

export function shortId(id: string): string {
    return id.slice(0, 8);
}

export function formatTime(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
    });
}

/** The day alone, for tight columns; pair it with formatTime in a tooltip. */
export function formatDay(seconds: number): string {
    return new Date(seconds * 1000).toLocaleDateString(undefined, {
        dateStyle: "medium",
    });
}

/** `+0200`-style offset for a commit's recorded time zone. */
export function formatOffset(seconds: number): string {
    const sign = seconds < 0 ? "-" : "+";
    const m = Math.abs(seconds) / 60;
    return `${sign}${String(Math.floor(m / 60)).padStart(2, "0")}${String(m % 60).padStart(2, "0")}`;
}

const LANGS: Record<string, string> = {
    lua: "lua",
    luau: "luau",
    json: "json",
    jsonc: "jsonc",
    json5: "jsonc",
    mcmeta: "json",
    js: "javascript",
    mjs: "javascript",
    cjs: "javascript",
    ts: "typescript",
    cs: "csharp",
    java: "java",
    kt: "kotlin",
    kts: "kotlin",
    gradle: "groovy",
    groovy: "groovy",
    xml: "xml",
    csproj: "xml",
    props: "xml",
    targets: "xml",
    yml: "yaml",
    yaml: "yaml",
    toml: "toml",
    ini: "ini",
    cfg: "ini",
    properties: "properties",
    lang: "properties",
    md: "markdown",
    py: "python",
    c: "c",
    h: "c",
    cpp: "cpp",
    cc: "cpp",
    hpp: "cpp",
    glsl: "glsl",
    vert: "glsl",
    frag: "glsl",
    vsh: "glsl",
    fsh: "glsl",
    hlsl: "hlsl",
    fx: "hlsl",
    sh: "shellscript",
    bash: "shellscript",
    ps1: "powershell",
    bat: "batch",
    cmd: "batch",
    html: "html",
    css: "css",
    rs: "rust",
    go: "go",
};

/** Highlighter language for a file path, or null for plain text. */
export function langForPath(path: string): string | null {
    const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
    const dot = name.lastIndexOf(".");
    if (dot <= 0) return null;
    return LANGS[name.slice(dot + 1)] ?? null;
}
