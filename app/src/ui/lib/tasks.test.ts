import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const { TaskTracker } = await import("./tasks.svelte");

const task = (id: number) => ({ id, kind: "clone", title: `Task ${id}` });

describe("TaskTracker", () => {
    it("tracks progress and removes finished tasks", async () => {
        const t = new TaskTracker();
        t.handle({ type: "started", task: task(1) });
        t.handle({
            type: "progress",
            id: 1,
            phase: "Receiving objects",
            percent: 40,
            message: "",
        });
        t.handle({
            type: "progress",
            id: 1,
            phase: "",
            percent: null,
            message: "hello",
        });
        expect(t.running).toMatchObject([
            {
                id: 1,
                phase: "Receiving objects",
                percent: 40,
                message: "hello",
            },
        ]);
        const done = t.wait(1);
        t.handle({
            type: "finished",
            id: 1,
            outcome: { status: "ok", value: 7 },
        });
        expect(await done).toEqual({ status: "ok", value: 7 });
        expect(t.running).toEqual([]);
    });

    it("keeps an outcome that arrives before anyone waits", async () => {
        const t = new TaskTracker();
        t.handle({ type: "finished", id: 9, outcome: { status: "cancelled" } });
        expect(await t.wait(9)).toEqual({ status: "cancelled" });
    });
});
