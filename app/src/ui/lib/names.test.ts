import { describe, expect, it } from "vitest";
import { nameFromUrl, projectNameProblem } from "./names";

describe("names", () => {
    it("derives the folder name from a URL like git does", () => {
        expect(nameFromUrl("https://github.com/a/My-Mod.git")).toBe("My-Mod");
        expect(nameFromUrl("git@github.com:a/b.git/")).toBe("b");
        expect(nameFromUrl("/srv/repos/thing")).toBe("thing");
        expect(nameFromUrl("https://host/.git")).toBe("");
        expect(nameFromUrl("")).toBe("");
    });

    it("matches the backend's project name rules", () => {
        for (const ok of [
            "ExampleMod",
            "My Mod 2",
            "mod_v1.2",
            "Ünïcödé",
            "CONSOLE",
        ]) {
            expect(projectNameProblem(ok), ok).toBeNull();
        }
        for (const bad of [
            "",
            " lead",
            "trail ",
            ".hidden",
            "-x",
            "dot.",
            "a/b",
            "a\\b",
            "a:b",
            "a*b",
            "CON",
            "com1",
            "lpt9.txt",
            "nul.proj",
            "tab\there",
            "a".repeat(101),
        ]) {
            expect(projectNameProblem(bad), bad).not.toBeNull();
        }
    });
});
