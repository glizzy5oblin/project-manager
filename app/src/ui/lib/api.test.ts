import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const { invoke } = await import("@tauri-apps/api/core");
const { api, toAppError } = await import("./api");

describe("api", () => {
    it("passes typed arguments with the backend's names", async () => {
        vi.mocked(invoke).mockResolvedValueOnce(5);
        expect(await api.projectClone("https://x/y.git", "/p", null)).toBe(5);
        expect(invoke).toHaveBeenCalledWith("project_clone", {
            url: "https://x/y.git",
            parent: "/p",
            name: null,
        });
    });

    it("rejects with an AppError whatever the backend throws", async () => {
        vi.mocked(invoke).mockRejectedValueOnce({
            kind: "notFound",
            message: "gone",
        });
        await expect(api.projectSummary("p1")).rejects.toEqual({
            kind: "notFound",
            message: "gone",
            detail: null,
            issues: [],
        });
        vi.mocked(invoke).mockRejectedValueOnce("plain string");
        await expect(api.projectsList()).rejects.toMatchObject({
            kind: "internal",
            message: "plain string",
        });
        expect(toAppError(new Error("boom")).message).toBe("boom");
    });
});
