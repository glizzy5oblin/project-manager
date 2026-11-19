import { describe, expect, it } from "vitest";
import { pathTail } from "./format";

describe("pathTail", () => {
    it("keeps short paths whole", () => {
        expect(pathTail("/srv/mods")).toBe("/srv/mods");
        expect(pathTail("C:\\Mods\\Cool")).toBe("C:\\Mods\\Cool");
    });
    it("keeps the last folders of long paths", () => {
        expect(pathTail("/srv/data/mods/Cool/private")).toBe(
            "…/mods/Cool/private",
        );
        expect(pathTail("D:\\work\\mods\\Cool\\private")).toBe(
            "…\\mods\\Cool\\private",
        );
    });
});
