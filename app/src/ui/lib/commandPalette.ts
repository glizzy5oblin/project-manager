// A pure, testable command filter for the command palette.

export interface Command {
    id: string;
    label: string;
    hint: string;
}

/** Case-insensitive substring match; prefix matches rank before mid-string matches. */
export function filterCommands(
    commands: readonly Command[],
    query: string,
): Command[] {
    const q = query.trim().toLowerCase();
    if (!q) return [...commands];
    const hits: { rank: number; command: Command }[] = [];
    for (const command of commands) {
        const hay = command.label.toLowerCase();
        if (hay.startsWith(q)) hits.push({ rank: 0, command });
        else if (hay.includes(q)) hits.push({ rank: 1, command });
    }
    hits.sort((a, b) => a.rank - b.rank);
    return hits.map((h) => h.command);
}
