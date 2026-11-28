// The command palette's filter, pinned to its ranking rules.
import { describe, expect, it } from "vitest";
import { filterCommands, type Command } from "./commandPalette";

const COMMANDS: Command[] = [
    { id: "hub", label: "Go to hub", hint: "Close every tab" },
    { id: "view:graph", label: "View: Graph", hint: "The branching graph" },
    { id: "view:log", label: "View: Log", hint: "Every commit as a list" },
    {
        id: "preferences",
        label: "Open preferences",
        hint: "The preferences dialog",
    },
];

describe("filterCommands", () => {
    it("returns every command for an empty query", () => {
        expect(filterCommands(COMMANDS, "")).toHaveLength(4);
        expect(filterCommands(COMMANDS, "   ")).toHaveLength(4);
    });

    it("matches case-insensitively by substring", () => {
        expect(filterCommands(COMMANDS, "VIEW:").map((c) => c.id)).toEqual([
            "view:graph",
            "view:log",
        ]);
    });

    it("ranks prefix matches ahead of mid-string matches", () => {
        const mixed: Command[] = [
            {
                id: "substring",
                label: "Go to the preferences screen",
                hint: "",
            },
            { id: "prefix", label: "Preferences panel", hint: "" },
        ];
        expect(filterCommands(mixed, "pref").map((c) => c.id)).toEqual([
            "prefix",
            "substring",
        ]);
    });

    it("returns nothing when nothing matches", () => {
        expect(filterCommands(COMMANDS, "zzz")).toEqual([]);
    });
});
