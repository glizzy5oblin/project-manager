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

    it("passes the diagnostics save path to the backend", async () => {
        vi.mocked(invoke).mockResolvedValueOnce(undefined);
        await api.diagnosticsSave("/tmp/report.txt");
        expect(invoke).toHaveBeenCalledWith("diagnostics_save", {
            path: "/tmp/report.txt",
        });
    });

    it("passes the cleanup arguments to the backend", async () => {
        vi.mocked(invoke).mockResolvedValueOnce(["a-1"]);
        await api.backupsCleanup("p1");
        expect(invoke).toHaveBeenCalledWith("backups_cleanup", {
            id: "p1",
            repoSide: "private",
        });
        vi.mocked(invoke).mockResolvedValueOnce([]);
        await api.backupsCleanupPreview("p1");
        expect(invoke).toHaveBeenCalledWith("backups_cleanup_preview", {
            id: "p1",
            repoSide: "private",
        });
    });

    it("passes app-data export/restore arguments to the backend", async () => {
        vi.mocked(invoke).mockResolvedValueOnce(undefined);
        await api.appDataExport("/tmp/data.zip");
        expect(invoke).toHaveBeenCalledWith("app_data_export", {
            path: "/tmp/data.zip",
        });
        vi.mocked(invoke).mockResolvedValueOnce([
            "projects.json",
            "settings.json",
        ]);
        await api.appDataRestore("/tmp/data.zip");
        expect(invoke).toHaveBeenCalledWith("app_data_restore", {
            path: "/tmp/data.zip",
        });
    });

    it("passes staging arguments to the backend", async () => {
        vi.mocked(invoke).mockResolvedValueOnce({
            files: [],
            blockers: 0,
            truncated: false,
        });
        expect(await api.stagingPreview("p1")).toMatchObject({ files: [] });
        expect(invoke).toHaveBeenCalledWith("staging_preview", { id: "p1" });

        vi.mocked(invoke).mockResolvedValueOnce(7);
        expect(await api.stagingSync("p1", ["a.txt"], "msg", true)).toBe(7);
        expect(invoke).toHaveBeenCalledWith("staging_sync", {
            id: "p1",
            allow: ["a.txt"],
            message: "msg",
            push: true,
        });

        vi.mocked(invoke).mockResolvedValueOnce({ ok: true, issues: [] });
        expect(await api.stagingVerify("p1")).toMatchObject({ ok: true });
        expect(invoke).toHaveBeenCalledWith("staging_verify", { id: "p1" });

        vi.mocked(invoke).mockResolvedValueOnce([]);
        expect(await api.stagingRecords("p1")).toEqual([]);
        expect(invoke).toHaveBeenCalledWith("staging_records", { id: "p1" });

        vi.mocked(invoke).mockResolvedValueOnce(undefined);
        await api.stagingSetOverride("p1", "a.txt", "exclude");
        expect(invoke).toHaveBeenCalledWith("staging_set_override", {
            id: "p1",
            path: "a.txt",
            kind: "exclude",
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            current: "1.2.3",
            target: "1.2.4",
            files: [],
        });
        expect(await api.releasePreview("p1", "patch", null)).toMatchObject({
            target: "1.2.4",
        });
        expect(invoke).toHaveBeenCalledWith("release_preview", {
            id: "p1",
            bump: "patch",
            exact: null,
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            current: "1.2.3",
            target: "1.3.0",
            files: [],
        });
        expect(await api.releaseApply("p1", "minor", null)).toMatchObject({
            target: "1.3.0",
        });
        expect(invoke).toHaveBeenCalledWith("release_apply", {
            id: "p1",
            bump: "minor",
            exact: null,
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            tag: "v1.2.4",
            branch: "main",
            expectedBranch: "main",
            clean: true,
            tagExists: false,
        });
        expect(await api.releaseTagPreview("p1")).toMatchObject({
            tag: "v1.2.4",
            clean: true,
        });
        expect(invoke).toHaveBeenCalledWith("release_tag_preview", {
            id: "p1",
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            tag: "v1.2.4",
            branch: "main",
            pushed: true,
        });
        expect(await api.releaseTagCreate("p1", true)).toMatchObject({
            pushed: true,
        });
        expect(invoke).toHaveBeenCalledWith("release_tag_create", {
            id: "p1",
            push: true,
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            fileName: "Example-1.2.3.zip",
            path: "/data/releases/Example-1.2.3.zip",
            entries: [],
            sha256: "abc123",
            bytes: 100,
            violations: [],
        });
        expect(await api.releaseZip("p1")).toMatchObject({
            fileName: "Example-1.2.3.zip",
        });
        expect(invoke).toHaveBeenCalledWith("release_zip", {
            id: "p1",
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            fromTag: "v1.2.3",
            toVersion: "1.2.3",
            groups: [],
            markdown: "## v1.2.3\n",
        });
        expect(await api.releaseChangelog("p1")).toMatchObject({
            fromTag: "v1.2.3",
        });
        expect(invoke).toHaveBeenCalledWith("release_changelog", {
            id: "p1",
        });

        vi.mocked(invoke).mockResolvedValueOnce({
            path: "a.txt",
            oldPath: null,
            status: "modified",
            binary: false,
            tooLarge: false,
            oldSize: 10,
            newSize: 12,
            oldMode: "100644",
            newMode: "100644",
            hunks: [],
        });
        expect(await api.stagingDiff("p1", "a.txt", true)).toMatchObject({
            status: "modified",
        });
        expect(invoke).toHaveBeenCalledWith("staging_diff", {
            id: "p1",
            path: "a.txt",
            force: true,
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
