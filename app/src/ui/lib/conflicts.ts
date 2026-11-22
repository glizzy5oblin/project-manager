// Plain-words explanations of a conflicted file, from the facts the backend collects
// (`conflict_info`). Nothing here talks about "ours", "theirs", stages or markers without
// saying what they mean.

import type { ConflictInfo } from "./bindings/ConflictInfo";

/** How each side is called: "main" for the branch you are on, the merged branch by its name. */
export function sideNames(info: ConflictInfo): { you: string; them: string } {
    return {
        you: info.oursName ?? "your version",
        them: info.theirsName ?? "the other side",
    };
}

const places = (n: number) => (n === 1 ? "1 place" : `${n} places`);

/** What happened, in one sentence, and why Git could not decide alone. */
export function explain(info: ConflictInfo): { headline: string; why: string } {
    const { you, them } = sideNames(info);
    switch (info.kind) {
        case "bothModified":
            return {
                headline: `You (${you}) and ${them} both changed this file.`,
                why: info.binary
                    ? "It is not a text file, so Git cannot combine the two versions. You have to pick one."
                    : info.regions.length
                      ? `Git combined everything it could. In ${places(info.regions.length)}${info.moreRegions ? " (and more)" : ""} both sides changed the same lines in different ways, and Git cannot tell which to keep.`
                      : "Both sides changed the same part of the file in different ways, and Git cannot tell which to keep.",
            };
        case "bothAdded":
            return {
                headline: `You (${you}) and ${them} each created a file with this name.`,
                why: "The two files have different contents, so Git cannot tell which one the file should be.",
            };
        case "deletedByUs":
            return {
                headline: `You (${you}) deleted this file, but ${them} changed it.`,
                why: "Git cannot tell whether the file should stay, with their changes, or be gone.",
            };
        case "deletedByThem":
            return {
                headline: `${them} deleted this file, but you (${you}) changed it.`,
                why: "Git cannot tell whether the file should stay, with your changes, or be gone.",
            };
        case "bothDeleted":
            return {
                headline: `Both ${you} and ${them} removed this file.`,
                why: "They did it in different ways (for example, each moved it to a different new name), so Git cannot tell where it should end up.",
            };
        case "addedByUs":
            return {
                headline: `You (${you}) added this file.`,
                why: `${them} has something else in its place (for example a renamed file or a folder with this name), so Git cannot tell which to keep.`,
            };
        case "addedByThem":
            return {
                headline: `${them} added this file.`,
                why: `You (${you}) have something else in its place (for example a renamed file or a folder with this name), so Git cannot tell which to keep.`,
            };
        case "resolved":
            return {
                headline: "This file is no longer in conflict.",
                why: "Go on to commit.",
            };
    }
}

/** What to do, as short steps. */
export function steps(info: ConflictInfo): string[] {
    const finish = [
        "When every conflicted file is resolved, commit to finish the merge, or use Abort merge to cancel it and go back to how things were.",
    ];
    switch (info.kind) {
        case "bothModified":
        case "bothAdded":
            return info.binary
                ? ["Press Keep my file or Keep their file.", ...finish]
                : [
                      "For each disagreement, press Keep mine, Keep theirs or Keep both. To mix them, edit the text in the box instead.",
                      "Press Save and mark resolved.",
                      ...finish,
                  ];
        case "deletedByUs":
        case "deletedByThem":
        case "bothDeleted":
        case "addedByUs":
        case "addedByThem":
            return [
                "Press Keep my file or Keep their file to keep one, or Delete the file to remove it.",
                ...finish,
            ];
        case "resolved":
            return finish;
    }
}

// ---- editing the file's text ----

/** One conflict in a file's text, found by its marker lines. `start` and `end` are line indexes. */
export interface TextRegion {
    start: number;
    end: number;
    ours: string[];
    theirs: string[];
}

export type Choice = "ours" | "theirs" | "both" | "theirsFirst";

const strip = (l: string) => (l.endsWith("\r") ? l.slice(0, -1) : l);

/** The conflicts still in `text`, in order. A marker with no closing line is ignored. */
export function findRegions(text: string): TextRegion[] {
    const lines = text.split("\n");
    const out: TextRegion[] = [];
    let open: { start: number; ours: string[]; theirs: string[] } | null = null;
    let part: "ours" | "base" | "theirs" = "ours";
    lines.forEach((raw, i) => {
        const l = strip(raw);
        if (!open) {
            if (l.startsWith("<<<<<<<"))
                open = { start: i, ours: [], theirs: [] };
            part = "ours";
        } else if (l.startsWith("|||||||") && part === "ours") part = "base";
        else if (l === "=======" && part !== "theirs") part = "theirs";
        else if (l.startsWith(">>>>>>>") && part === "theirs") {
            out.push({ ...open, end: i });
            open = null;
        } else if (part === "ours") open.ours.push(raw);
        else if (part === "theirs") open.theirs.push(raw);
    });
    return out;
}

/** `text` with conflict number `index` settled as `choice`. Other conflicts are left alone. */
export function resolveRegion(
    text: string,
    index: number,
    choice: Choice,
): string {
    const r = findRegions(text)[index];
    if (!r) return text;
    const keep =
        choice === "ours"
            ? r.ours
            : choice === "theirs"
              ? r.theirs
              : choice === "both"
                ? [...r.ours, ...r.theirs]
                : [...r.theirs, ...r.ours];
    const lines = text.split("\n");
    lines.splice(r.start, r.end - r.start + 1, ...keep);
    return lines.join("\n");
}

/** A text field gives back "\n" line ends; put the file's own back so a save does not change them. */
export function withLineEnds(edited: string, original: string): string {
    return original.includes("\r\n")
        ? edited.replace(/\r?\n/g, "\r\n")
        : edited;
}
