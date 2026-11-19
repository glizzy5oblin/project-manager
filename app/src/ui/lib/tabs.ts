// Tab list operations. Each returns a new Session and never mutates its input; `active === null`
// means the hub is showing.

import type { ProjectView } from "./bindings/ProjectView";
import type { Session } from "./bindings/Session";

export const VIEWS: { id: ProjectView; label: string }[] = [
    { id: "overview", label: "Overview" },
    { id: "graph", label: "Graph" },
    { id: "log", label: "Log" },
    { id: "staging", label: "Staging" },
    { id: "releases", label: "Releases" },
    { id: "settings", label: "Settings" },
];

/** Show a project's tab, opening it at the end if needed. */
export function openTab(s: Session, projectId: string): Session {
    const tabs = s.tabs.some((t) => t.projectId === projectId)
        ? s.tabs
        : [...s.tabs, { projectId, view: "overview" as ProjectView }];
    return { tabs, active: projectId };
}

/** Close a tab. When it was showing, its right neighbour shows, else its left one, else the hub. */
export function closeTab(s: Session, projectId: string): Session {
    const i = s.tabs.findIndex((t) => t.projectId === projectId);
    if (i < 0) return s;
    const tabs = s.tabs.filter((t) => t.projectId !== projectId);
    let active = s.active;
    if (active === projectId) {
        active = (tabs[i] ?? tabs[i - 1])?.projectId ?? null;
    }
    return { tabs, active };
}

/** Move a tab to position `to` (clamped). */
export function moveTab(s: Session, projectId: string, to: number): Session {
    const from = s.tabs.findIndex((t) => t.projectId === projectId);
    if (from < 0) return s;
    const tabs = [...s.tabs];
    const [tab] = tabs.splice(from, 1);
    tabs.splice(Math.max(0, Math.min(to, tabs.length)), 0, tab);
    return { ...s, tabs };
}

export function setView(
    s: Session,
    projectId: string,
    view: ProjectView,
): Session {
    return {
        ...s,
        tabs: s.tabs.map((t) =>
            t.projectId === projectId ? { ...t, view } : t,
        ),
    };
}

/** The tab `step` places from the active one, wrapping, with the hub as position 0. */
export function cycle(s: Session, step: number): Session {
    const order = [null, ...s.tabs.map((t) => t.projectId)];
    const i = Math.max(0, order.indexOf(s.active));
    const n = order.length;
    return { ...s, active: order[(((i + step) % n) + n) % n] };
}

/** Drop tabs for projects that are gone. */
export function pruneTabs(s: Session, known: Set<string>): Session {
    let out = s;
    for (const t of s.tabs) {
        if (!known.has(t.projectId)) out = closeTab(out, t.projectId);
    }
    return out;
}
