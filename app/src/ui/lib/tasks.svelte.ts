// Background tasks: a live list for the task bar, and `awaitTask` for code that started one.

import { listen } from "@tauri-apps/api/event";
import { api } from "./api";
import type { TaskEvent } from "./bindings/TaskEvent";
import type { TaskInfo } from "./bindings/TaskInfo";
import type { TaskOutcome } from "./bindings/TaskOutcome";

export type RunningTask = TaskInfo & {
    phase: string;
    percent: number | null;
    message: string;
};

/**
 * Tracks running tasks from `task` events. Pure apart from the reactive state, so it can be
 * driven directly in tests.
 */
export class TaskTracker {
    running = $state<RunningTask[]>([]);
    /** Outcomes not yet collected by `wait` (a task can finish before its starter asks). */
    // Bookkeeping only, never rendered, so plain (non-reactive) maps.
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    #done = new Map<number, TaskOutcome>();
    // eslint-disable-next-line svelte/prefer-svelte-reactivity
    #waiting = new Map<number, (o: TaskOutcome) => void>();

    handle(event: TaskEvent) {
        switch (event.type) {
            case "started":
                if (!this.running.some((t) => t.id === event.task.id)) {
                    this.running.push({
                        ...event.task,
                        phase: "",
                        percent: null,
                        message: "",
                    });
                }
                break;
            case "progress": {
                const t = this.running.find((t) => t.id === event.id);
                if (t) {
                    if (event.phase) {
                        t.phase = event.phase;
                        t.percent = event.percent;
                    }
                    if (event.message) t.message = event.message;
                }
                break;
            }
            case "finished": {
                this.running = this.running.filter((t) => t.id !== event.id);
                const resolve = this.#waiting.get(event.id);
                if (resolve) {
                    this.#waiting.delete(event.id);
                    resolve(event.outcome);
                } else {
                    this.#done.set(event.id, event.outcome);
                    // Nobody collected it; keep the buffer small.
                    if (this.#done.size > 100) {
                        this.#done.delete(this.#done.keys().next().value!);
                    }
                }
                break;
            }
        }
    }

    wait(id: number): Promise<TaskOutcome> {
        const done = this.#done.get(id);
        if (done) {
            this.#done.delete(id);
            return Promise.resolve(done);
        }
        return new Promise((resolve) => this.#waiting.set(id, resolve));
    }
}

export const tasks = new TaskTracker();

/** Start listening for task events and pick up tasks already running (after a reload). */
export async function initTasks() {
    await listen<TaskEvent>("task", (e) => tasks.handle(e.payload));
    for (const task of await api.tasksRunning()) {
        tasks.handle({ type: "started", task });
    }
}
