import { describe, expect, it } from "vitest";
import type { Theme } from "./bindings/Theme";
import { isColor, pickTheme, themeVars } from "./theme";

const theme = (colors: Record<string, string>): Theme => ({
    id: "t",
    name: "T",
    scheme: "dark",
    builtin: false,
    colors,
});

describe("theme", () => {
    it("accepts color literals only", () => {
        for (const ok of [
            "#abc",
            "#a1b2c3ff",
            "rgb(1 2 3 / 50%)",
            "hsl(210deg, 5%, 4%)",
            "transparent",
        ]) {
            expect(isColor(ok), ok).toBe(true);
        }
        for (const bad of [
            "red",
            "var(--x)",
            "url(x)",
            "#fff;}body{",
            "rgb(1,2,3) !important",
            "",
        ]) {
            expect(isColor(bad), bad).toBe(false);
        }
    });

    it("turns colors into CSS variables, dropping anything unsafe", () => {
        const vars = themeVars(
            theme({
                bg: "#000",
                "graph-1": "#111",
                "x;y": "#222",
                accent: "url(evil)",
            }),
        );
        expect(vars).toEqual([
            ["--bg", "#000"],
            ["--graph-1", "#111"],
        ]);
    });

    it("falls back to the first theme for an unknown id", () => {
        const list = {
            themes: [theme({}), { ...theme({}), id: "x" }],
            problems: [],
        };
        expect(pickTheme(list, "x").id).toBe("x");
        expect(pickTheme(list, "missing").id).toBe("t");
    });
});
