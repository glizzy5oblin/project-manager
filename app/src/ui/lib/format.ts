import type { Operation } from "./bindings/Operation";

const OPERATIONS: Record<Operation, string> = {
    merge: "Merge",
    rebase: "Rebase",
    cherryPick: "Cherry-pick",
    revert: "Revert",
    bisect: "Bisect",
    applyMailbox: "Patch apply (am)",
};

export function operationLabel(op: Operation): string {
    return OPERATIONS[op] ?? op;
}

/** The last `keep` folders of a path, led by `…` when shortened; for tight spots, with the full path as a tooltip. */
export function pathTail(path: string, keep = 3): string {
    const sep = path.includes("\\") && !path.includes("/") ? "\\" : "/";
    const parts = path.split(sep).filter((s) => s !== "");
    return parts.length <= keep
        ? path
        : `…${sep}${parts.slice(-keep).join(sep)}`;
}
