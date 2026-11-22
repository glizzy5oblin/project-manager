// Runs write actions through the backend pipeline (docs/actions.md): preview, confirm when the
// action is dangerous or the repository is untrusted, run as a task, and preview again when the
// repository changed before the run started. One confirmation shows at a time, in ActionDialog.

import { api } from "./api";
import type { Action } from "./bindings/Action";
import type { ActionOutcome } from "./bindings/ActionOutcome";
import type { AppError } from "./bindings/AppError";
import type { Preview } from "./bindings/Preview";
import type { TaskOutcome } from "./bindings/TaskOutcome";
import { tasks } from "./tasks.svelte";

/** A confirmation waiting for the user. */
export type Pending = {
    preview: Preview;
    /** The repository is owned by another user; running needs the user's explicit consent. */
    untrusted: boolean;
    /** Why the dialog is showing again, e.g. the repository changed. */
    note: string | null;
    answer: (ok: boolean) => void;
};

export type RunnerDeps = {
    preview: (
        id: string,
        action: Action,
        allowUntrusted: boolean,
    ) => Promise<Preview>;
    run: (
        id: string,
        action: Action,
        token: string | null,
        allowUntrusted: boolean,
    ) => Promise<number>;
    wait: (task: number) => Promise<TaskOutcome>;
};

/** Times a stale preview is shown again before giving up (the repository keeps changing). */
const MAX_RETRIES = 3;

export class ActionRunner {
    pending = $state<Pending | null>(null);
    /** Actions running, by project, so views can disable their buttons meanwhile. */
    busy = $state<Record<string, number>>({});
    #deps: RunnerDeps;

    constructor(deps: RunnerDeps) {
        this.#deps = deps;
    }

    #ask(preview: Preview, untrusted: boolean, note: string | null) {
        // A second request while one is open would hide the first; refuse it instead.
        if (this.pending) return Promise.resolve(false);
        return new Promise<boolean>((resolve) => {
            this.pending = {
                preview,
                untrusted,
                note,
                answer: (ok) => {
                    this.pending = null;
                    resolve(ok);
                },
            };
        });
    }

    /**
     * Preview, confirm if needed, and run `action`. Resolves with the outcome, or `null` when the
     * user said no or the task was cancelled; rejects with an AppError when the action failed.
     * `confirm` asks even for a safe action.
     */
    async run(
        projectId: string,
        action: Action,
        confirm = false,
    ): Promise<ActionOutcome | null> {
        this.busy[projectId] = (this.busy[projectId] ?? 0) + 1;
        try {
            return await this.#run(projectId, action, confirm);
        } finally {
            const n = (this.busy[projectId] ?? 1) - 1;
            if (n > 0) this.busy[projectId] = n;
            else delete this.busy[projectId];
        }
    }

    async #run(
        projectId: string,
        action: Action,
        confirm: boolean,
    ): Promise<ActionOutcome | null> {
        let untrusted = false;
        let note: string | null = null;
        for (let attempt = 0; ; attempt++) {
            let preview: Preview;
            try {
                preview = await this.#deps.preview(
                    projectId,
                    action,
                    untrusted,
                );
            } catch (e) {
                if ((e as AppError).kind !== "untrusted" || untrusted) throw e;
                // Ask once with the warning, then preview as allowed.
                untrusted = true;
                preview = await this.#deps.preview(projectId, action, true);
            }
            if (preview.dangerous || untrusted || confirm || note) {
                if (!(await this.#ask(preview, untrusted, note))) return null;
            }
            const task = await this.#deps.run(
                projectId,
                action,
                preview.dangerous ? preview.token : null,
                untrusted,
            );
            const outcome = await this.#deps.wait(task);
            if (outcome.status === "ok") return outcome.value as ActionOutcome;
            if (outcome.status === "cancelled") return null;
            if (outcome.error.kind !== "stale" || attempt >= MAX_RETRIES) {
                throw outcome.error;
            }
            note =
                "The repository changed after the preview. Check the impact again.";
        }
    }
}

export const actions = new ActionRunner({
    preview: api.actionPreview,
    run: api.actionRun,
    wait: (id) => tasks.wait(id),
});

/** The status line shown after an action ran: what was done, conflicts, and the backup taken. */
export function outcomeNotice(
    outcome: ActionOutcome,
    done?: string,
): { text: string; warn: boolean } {
    const sentence = (s: string) => (/[.!?]$/.test(s) ? s : s + ".");
    const parts = [sentence(done ?? outcome.title)];
    const n = outcome.conflicts.length;
    if (n) {
        parts.push(
            `${n} conflicted ${n === 1 ? "file" : "files"}: resolve ${n === 1 ? "it" : "them"} in Changes.`,
        );
    }
    if (outcome.backup) parts.push(`Backup ${outcome.backup.id} taken.`);
    return { text: parts.join(" "), warn: n > 0 };
}
