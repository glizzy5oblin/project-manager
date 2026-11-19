import { describe, expect, it } from "vitest";
import { highlight, withoutPrototypeScopes } from "./highlight";

describe("withoutPrototypeScopes", () => {
    it("drops scopes with a part named like an Object.prototype member", () => {
        expect(
            withoutPrototypeScopes([
                { scope: ["meta.instance.constructor", "keyword"] },
                { scope: "entity.toString" },
                { scope: "string" },
                {},
            ]),
        ).toEqual([{ scope: ["keyword"] }, { scope: ["string"] }, {}]);
    });
});

describe("highlight", () => {
    it("colors C# with the CSS-variable theme", async () => {
        const toks = await highlight(
            "public class Lamp {\n    float Fuel = 1f;\n}",
            "csharp",
        );
        expect(toks).toHaveLength(3);
        const colors = toks!.flat().map((t) => t.color);
        expect(colors.some((c) => c?.startsWith("var(--shiki-token-"))).toBe(
            true,
        );
    });

    it("returns null for unknown languages", async () => {
        expect(await highlight("x", "nope")).toBeNull();
        expect(await highlight("x", null)).toBeNull();
    });
});
