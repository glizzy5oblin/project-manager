// Own file: freezing Object.prototype lasts for the whole test worker.
import { expect, it } from "vitest";
import { highlight } from "./highlight";

it("highlights with a frozen Object.prototype, like Tauri's freezePrototype", async () => {
    Object.freeze(Object.prototype);
    const toks = await highlight("new Lamp();", "csharp");
    expect(toks).not.toBeNull();
});
