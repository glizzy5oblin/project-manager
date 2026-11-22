// The right-click menu that replaces the webview's own (Reload, Inspect, ...): clipboard
// commands for the text under the pointer, or a menu a view offers for the thing clicked (see
// `offerMenu`). The pure parts are here; ContextMenu.svelte draws it.

export type MenuCommand = "cut" | "copy" | "paste" | "selectAll";

export interface MenuItem {
    /** Names the item to `run`; a clipboard command for the built-in items. */
    id: string;
    label: string;
    /** Shown as the shortcut hint. */
    keys: string;
    disabled: boolean;
    /** A line between groups; has no action. */
    separator?: boolean;
    /** Marks an action that discards work. */
    danger?: boolean;
    /** A plain-words line shown on hover. */
    hint?: string;
}

/** A menu a view offers for what was right-clicked, in place of the clipboard menu. */
export interface CustomMenu {
    items: MenuItem[];
    run: (id: string) => void;
}

let offered: { event: Event; menu: CustomMenu } | null = null;

/**
 * Called from a view's `contextmenu` handler, which runs before the window's: the menu shows
 * these items for this event instead of the clipboard ones.
 */
export function offerMenu(event: Event, menu: CustomMenu): void {
    offered = { event, menu };
}

/** The menu offered for this event, if any. Taken once. */
export function takeMenu(event: Event): CustomMenu | null {
    const o = offered;
    offered = null;
    return o && o.event === event ? o.menu : null;
}

/** What was right-clicked: a text field, or plain page text. */
export interface MenuTarget {
    /** A text field or text area the user can type in. */
    editable: boolean;
    /** Some text is selected. */
    hasSelection: boolean;
}

const isMac = () =>
    typeof navigator !== "undefined" && /mac/i.test(navigator.platform);

function keys(letter: string): string {
    return `${isMac() ? "⌘" : "Ctrl+"}${letter}`;
}

/** The menu items for a target. Cut and Paste only appear where text can be typed. */
export function menuItems(t: MenuTarget): MenuItem[] {
    const items: MenuItem[] = [];
    if (t.editable) {
        items.push({
            id: "cut",
            label: "Cut",
            hint: "Remove the selected text and keep it on the clipboard",
            keys: keys("X"),
            disabled: !t.hasSelection,
        });
    }
    items.push({
        id: "copy",
        label: "Copy",
        hint: "Copy the selected text to the clipboard",
        keys: keys("C"),
        disabled: !t.hasSelection,
    });
    if (t.editable) {
        items.push({
            id: "paste",
            label: "Paste",
            hint: "Put the clipboard text here",
            keys: keys("V"),
            disabled: false,
        });
    }
    items.push({
        id: "selectAll",
        label: "Select all",
        hint: "Select all the text here",
        keys: keys("A"),
        disabled: false,
    });
    return items;
}

/** Where to put a menu of `w` by `h` opened at (x, y), kept inside the window with a margin. */
export function menuPosition(
    x: number,
    y: number,
    w: number,
    h: number,
    vw: number,
    vh: number,
    margin = 4,
): { x: number; y: number } {
    const fit = (p: number, size: number, view: number) =>
        Math.max(margin, Math.min(p, view - size - margin));
    return { x: fit(x, w, vw), y: fit(y, h, vh) };
}

/** The index of the next enabled item from `from`, stepping by `step` and wrapping; -1 if none. */
export function nextEnabled(
    items: MenuItem[],
    from: number,
    step: 1 | -1,
): number {
    const n = items.length;
    for (let i = 1; i <= n; i++) {
        const k = (((from + step * i) % n) + n) % n;
        if (!items[k].disabled && !items[k].separator) return k;
    }
    return -1;
}

type TextField = HTMLInputElement | HTMLTextAreaElement;

const NON_TEXT_INPUTS = new Set([
    "button",
    "checkbox",
    "color",
    "file",
    "image",
    "radio",
    "range",
    "reset",
    "submit",
]);

/** The text field an element is, if it is one that has a text selection. */
export function textField(el: EventTarget | null): TextField | null {
    if (el instanceof HTMLTextAreaElement) return el;
    if (el instanceof HTMLInputElement && !NON_TEXT_INPUTS.has(el.type)) {
        return el;
    }
    return null;
}

/** The selected text in a field or, for a field, on the page. */
export function selectedText(field: TextField | null): string {
    if (field) {
        const { selectionStart: a, selectionEnd: b } = field;
        return a === null || b === null ? "" : field.value.slice(a, b);
    }
    return window.getSelection()?.toString() ?? "";
}

function replaceSelection(field: TextField, text: string): void {
    field.setRangeText(
        text,
        field.selectionStart ?? field.value.length,
        field.selectionEnd ?? field.value.length,
        "end",
    );
    // Frameworks listen for `input`; setRangeText does not fire it.
    field.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Run a menu command on the field or page text it was opened on. */
export async function runCommand(
    command: MenuCommand,
    field: TextField | null,
): Promise<void> {
    field?.focus();
    const writable = field !== null && !field.readOnly && !field.disabled;
    switch (command) {
        case "copy": {
            const text = selectedText(field);
            if (text) await navigator.clipboard.writeText(text);
            break;
        }
        case "cut": {
            const text = selectedText(field);
            if (text && field && writable) {
                await navigator.clipboard.writeText(text);
                replaceSelection(field, "");
            }
            break;
        }
        case "paste": {
            if (field && writable) {
                replaceSelection(field, await navigator.clipboard.readText());
            }
            break;
        }
        case "selectAll": {
            if (field) field.select();
            else document.execCommand("selectAll");
            break;
        }
    }
}
