// The right-click menu for a file in the Changes lists: the same actions as the row's buttons,
// plus copying the path. Pure; ChangesView runs the chosen command.

import type { ChangeSide } from "./changes";
import type { MenuItem } from "./contextMenu";

export type ChangeCommand =
    | "stage"
    | "unstage"
    | "discard"
    | "resolved"
    | "open-file"
    | "ignore-file"
    | "ignore-ext"
    | "show-folder"
    | "copy-path"
    | "copy-name";

/** The .gitignore lines the menu offers for a path: that one file, and all files of its type. */
export function ignorePatterns(path: string): {
    file: string;
    ext: string | null;
} {
    const name = path.slice(path.lastIndexOf("/") + 1);
    const dot = name.lastIndexOf(".");
    return {
        file: "/" + path,
        ext: dot > 0 && dot < name.length - 1 ? "*" + name.slice(dot) : null,
    };
}

export function changeMenu(c: {
    path: string;
    side: ChangeSide;
    busy: boolean;
    /** Not tracked by git yet: only such a file can be ignored. */
    untracked?: boolean;
}): MenuItem[] {
    const items: MenuItem[] = [];
    const add = (
        id: ChangeCommand,
        label: string,
        hint: string,
        extra: Partial<MenuItem> = {},
    ) =>
        items.push({
            id,
            label,
            keys: "",
            disabled: c.busy && !id.startsWith("copy"),
            hint,
            ...extra,
        });
    if (c.side === "staged")
        add(
            "unstage",
            "Unstage",
            "Take the file out of the next commit. Your changes stay in the file",
        );
    else if (c.side === "conflicted")
        add(
            "resolved",
            "Mark resolved",
            "Stage the file as it is now, once you have fixed the conflict in it",
        );
    else {
        add("stage", "Stage", "Put the file in the next commit");
        add(
            "discard",
            "Discard changes",
            "Throw away your changes to this file. A backup is taken first",
            { danger: true },
        );
    }
    items.push({
        id: "",
        label: "",
        keys: "",
        disabled: false,
        separator: true,
    });
    if (c.untracked) {
        const p = ignorePatterns(c.path);
        add(
            "ignore-file",
            "Add to .gitignore",
            `Stop git from tracking this file: adds ${p.file} to .gitignore`,
        );
        if (p.ext)
            add(
                "ignore-ext",
                `Ignore all ${p.ext} files`,
                `Adds ${p.ext} to .gitignore, so every file of this type is ignored`,
            );
    }
    if (c.side !== "conflicted")
        add(
            "open-file",
            "Open in Files",
            "Open the file in the Files tab to edit it",
        );
    add(
        "show-folder",
        "Show in folder",
        "Open the file's folder in your file manager",
    );
    add("copy-path", "Copy path", "Copy the file's path to the clipboard");
    add("copy-name", "Copy file name", "Copy just the file's name");
    return items;
}

export const fileName = (p: string) => p.slice(p.lastIndexOf("/") + 1);
