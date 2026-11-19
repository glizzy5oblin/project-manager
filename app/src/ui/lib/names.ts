// Mirrors of the backend's name rules (src/desktop/src/projects/create.rs), for hints while
// typing. The backend checks again.

/** The folder name a clone of `url` gets: its last path segment without `.git`. */
export function nameFromUrl(url: string): string {
    const trimmed = url.trim().replace(/[/\\]+$/, "");
    const last = trimmed.split(/[/\\:]/).pop() ?? "";
    const name = last.endsWith(".git") ? last.slice(0, -4) : last;
    return projectNameProblem(name) === null ? name : "";
}

/** Why `name` cannot be a project (folder) name, or null when it can. */
export function projectNameProblem(name: string): string | null {
    if (name.trim() === "") return "Enter a project name";
    if ([...name].length > 100) return "Use at most 100 characters";
    if (name !== name.trim()) return "Remove the space at the start or end";
    if (/^[.-]/.test(name) || name.endsWith("."))
        return "Cannot start with . or -, or end with .";
    // eslint-disable-next-line no-control-regex
    if (/[<>:"/\\|?*\u0000-\u001f\u007f]/.test(name)) {
        return 'These characters are not allowed: < > : " / \\ | ? *';
    }
    const stem = name.split(".")[0].toUpperCase();
    if (/^(CON|PRN|AUX|NUL|COM\d|LPT\d)$/.test(stem))
        return "This name is reserved on Windows";
    return null;
}
