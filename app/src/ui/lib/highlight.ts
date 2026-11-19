// Syntax highlighting with Shiki, loaded on first use so it costs nothing until a diff or blame
// opens. Colors are CSS variables (`--shiki-*`, mapped to the app theme in app.css), so a theme
// change recolors code without highlighting it again. Any failure falls back to plain text.

import type { HighlighterCore } from "shiki/core";
import type { Token } from "./history";

type LangModule = Promise<{ default: unknown }>;

const LOADERS: Record<string, () => LangModule> = {
    lua: () => import("shiki/langs/lua.mjs"),
    luau: () => import("shiki/langs/luau.mjs"),
    json: () => import("shiki/langs/json.mjs"),
    jsonc: () => import("shiki/langs/jsonc.mjs"),
    javascript: () => import("shiki/langs/javascript.mjs"),
    typescript: () => import("shiki/langs/typescript.mjs"),
    csharp: () => import("shiki/langs/csharp.mjs"),
    java: () => import("shiki/langs/java.mjs"),
    kotlin: () => import("shiki/langs/kotlin.mjs"),
    groovy: () => import("shiki/langs/groovy.mjs"),
    xml: () => import("shiki/langs/xml.mjs"),
    yaml: () => import("shiki/langs/yaml.mjs"),
    toml: () => import("shiki/langs/toml.mjs"),
    ini: () => import("shiki/langs/ini.mjs"),
    properties: () => import("shiki/langs/properties.mjs"),
    markdown: () => import("shiki/langs/markdown.mjs"),
    python: () => import("shiki/langs/python.mjs"),
    c: () => import("shiki/langs/c.mjs"),
    cpp: () => import("shiki/langs/cpp.mjs"),
    glsl: () => import("shiki/langs/glsl.mjs"),
    hlsl: () => import("shiki/langs/hlsl.mjs"),
    shellscript: () => import("shiki/langs/shellscript.mjs"),
    powershell: () => import("shiki/langs/powershell.mjs"),
    batch: () => import("shiki/langs/batch.mjs"),
    html: () => import("shiki/langs/html.mjs"),
    css: () => import("shiki/langs/css.mjs"),
    rust: () => import("shiki/langs/rust.mjs"),
    go: () => import("shiki/langs/go.mjs"),
};

/** Larger inputs are shown as plain text; tokenising them would stall the view. */
export const MAX_HIGHLIGHT_CHARS = 512 * 1024;

let highlighter: Promise<HighlighterCore> | null = null;

type ThemeRule = { scope?: string | string[] };

/**
 * Drops theme scopes with a part named like an `Object.prototype` member ("constructor"). Tauri's
 * `freezePrototype` makes Shiki's theme trie throw when it stores such a part as an object key,
 * which would turn off highlighting everywhere.
 */
export function withoutPrototypeScopes<T extends ThemeRule>(rules: T[]): T[] {
    const reserved = new Set(Object.getOwnPropertyNames(Object.prototype));
    const ok = (scope: string) =>
        !scope.split(/[\s.]+/).some((part) => reserved.has(part));
    return rules.flatMap((rule) => {
        if (rule.scope === undefined) return [rule];
        const scopes = (
            Array.isArray(rule.scope) ? rule.scope : [rule.scope]
        ).filter(ok);
        return scopes.length > 0 ? [{ ...rule, scope: scopes }] : [];
    });
}

function load(): Promise<HighlighterCore> {
    highlighter ??= (async () => {
        const [
            { createHighlighterCore, createCssVariablesTheme },
            { createJavaScriptRegexEngine },
        ] = await Promise.all([
            import("shiki/core"),
            import("shiki/engine/javascript"),
        ]);
        const theme = createCssVariablesTheme({
            name: "project-manager",
            variablePrefix: "--shiki-",
            fontStyle: false,
        });
        theme.tokenColors = withoutPrototypeScopes(theme.tokenColors ?? []);
        return createHighlighterCore({
            themes: [theme],
            langs: [],
            engine: createJavaScriptRegexEngine({ forgiving: true }),
        });
    })();
    return highlighter;
}

/** Tokens per line of `code`, or null when the language is unknown or highlighting fails. */
export async function highlight(
    code: string,
    lang: string | null,
): Promise<Token[][] | null> {
    if (!lang || !(lang in LOADERS) || code.length > MAX_HIGHLIGHT_CHARS)
        return null;
    try {
        const h = await load();
        if (!h.getLoadedLanguages().includes(lang)) {
            await h.loadLanguage(
                (await LOADERS[lang]()) as Parameters<
                    HighlighterCore["loadLanguage"]
                >[0],
            );
        }
        return h
            .codeToTokens(code, { lang, theme: "project-manager" })
            .tokens.map((line) =>
                line.map((t) => ({ content: t.content, color: t.color })),
            );
    } catch {
        return null;
    }
}
