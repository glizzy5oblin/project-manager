# `.proj` file format

Each Project Manager project has one `<name>.proj` file at its project root — the folder that holds the file, which is where Project Manager looks for it. It is TOML. Project Manager edits it in place and keeps your comments and layout; only the values it changes are rewritten.

In two-repo mode the project root is a plain folder beside two repositories, `private/` (the working repo) and `public/` (the release repo), both described by the `.proj`; in single-repo mode the project root IS the repository. The `.proj` is also read from one level down, for backward compatibility with the old layout (the proj inside the `private/` folder, with `path = "."` and `path = "../public"`).

Project Manager looks for exactly one `*.proj` file in the folder. None, or more than one, is an error.

## Example (two-repo mode)

```toml
schema_version = 1

[project]
name = "ExampleProject"
version = "0.0.1"

[workflow]
mode = "two-repo"

[workflow.private]
path = "./private"
branch = "main"
remote = "origin"

[workflow.public]
path = "./public"
branch = "main"
remote = "origin"

[staging]
include = ["src/**", "README.md", "LICENSE", "!src/dev/**"]
exclude = ["secrets/**", "**/*.dump"]
overrides = { include = [], exclude = [] }

[release]
version_files = ["manifest.json"]
asset_name = "{name}-{version}.zip"

[[release.targets]]
kind = "github"
```

## Keys

Unknown keys are errors, so a typo such as `exlude` is reported instead of ignored.

### Top level

| Key              | Required | Meaning                                                            |
| ---------------- | -------- | ------------------------------------------------------------------ |
| `schema_version` | yes      | Format version. Project Manager writes `1`. Newer versions are refused. |

### `[project]`

| Key       | Required | Meaning                                     |
| --------- | -------- | ------------------------------------------- |
| `name`    | yes      | Display name. Not empty, no `/` or `\`.     |
| `version` | yes      | Semantic version (`1.2.3`, `1.2.3-beta.1`). |

### `[workflow]`

`mode` is `"two-repo"` (a private repo and a separate public repo) or `"single-repo"` (one repo with a private branch and a public branch).

`[workflow.private]` and `[workflow.public]` each take:

| Key      | Required  | Meaning                                                                   |
| -------- | --------- | ------------------------------------------------------------------------- |
| `path`   | see below | Repository folder, relative to the `.proj` file. Private defaults to `.`. |
| `branch` | yes       | Branch name, checked with git's branch-name rules.                        |
| `remote` | no        | Remote name. Defaults to `origin`.                                        |

- Two-repo mode: `workflow.public.path` is required and must differ from the private path.
- Single-repo mode: omit `workflow.public.path` (or set it to the private path), and the public branch must differ from the private branch.

### `[staging]`

Which private files are copied to the public side.

| Key                 | Meaning                                                              |
| ------------------- | -------------------------------------------------------------------- |
| `include`           | Globs of files to copy. An entry starting with `!` excludes instead. |
| `exclude`           | Globs never copied.                                                  |
| `overrides.include` | Manual per-file choices that force a file in.                        |
| `overrides.exclude` | Manual per-file choices that force a file out.                       |

Precedence, later wins: built-in defaults (for example `.md` files other than README.md, CHANGELOG.md and LICENSE stay private), then `include`/`exclude`, then `overrides`. All paths are relative to the private repository and must not use `..` or be absolute. `!` is only allowed in `include`.

`[staging.map]` publishes a private file under another name, for a public file that must differ from its private namesake (a public Makefile without the private targets, say):

```toml
[staging.map]
"publish/Makefile" = "Makefile"
"publish/gitignore" = ".gitignore"
```

The public `Makefile` is then copied from `publish/Makefile` alone: the private `Makefile` is never staged, `publish/Makefile` is not staged under its own name, and the release zip's allowlist check accepts the public `Makefile`. The include and exclude globs do not apply to mapped files. Each public name may be the target of one entry only.

### `[release]`

| Key               | Default                                                                       | Meaning                                                                                          |
| ----------------- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `version_files`   | `[]`                                                                          | Files in the private repository whose version string is bumped with the project (relative to it). |
| `asset_name`      | `"{name}-{version}.zip"`                                                      | Release zip name. Placeholders: `{name}`, `{version}`.                                           |
| `changelog_groups` | conventional prefixes (`["feat", "fix", "perf", "docs", "refactor", "test", "build", "ci", "chore"]`), in display order | Conventional-commit prefixes the release notes group by. Commits matching none of them land in an "Other changes" group. Lowercase, no duplicates — validated. |
| `targets`         | `[]`                                                                          | `[[release.targets]]` entries with a `kind`.                                                      |
| `history`         | `[]`                                                                          | `[[release.history]]` entries with a `tag`, `version` and `date`. See below.                     |

`kind = "github"` is the only target for now. Each kind may appear once.

Release notes are generated from the commits on the private branch since the last tag (merge commits excluded), grouped by `changelog_groups`, and rendered as markdown in the release view. The editor edits the generated markdown in place; the preview shows exactly what would be published.

### `[[release.history]]`

| Key      | Meaning                                                                |
| -------- | ---------------------------------------------------------------------- |
| `tag`    | The annotated tag, e.g. `v1.2.3`. Identifies the entry — idempotent by tag. |
| `version` | The released version, `1.2.3`.                                          |
| `date`   | The tag's date (`YYYY-MM-DD`), as recorded.                            |

The history table records a release the moment its annotated tag is created (before any push). A tag that is already recorded is left alone, and the history is otherwise hand-editable — the UI lists it verbatim.

## Errors

- Syntax and type errors give the line and column.
- Validation reports every problem at once, each with its key path, e.g. `workflow.public.path: required in two-repo mode`.
- When Project Manager saves an edited file it validates first and writes nothing if the result would not load. Writes are atomic (temporary file, then rename).

## Versions and migration

When the format changes, `schema_version` goes up and Project Manager reads older files by upgrading them in memory, and writes the upgraded form the next time it saves the file. A file with a newer `schema_version` than the running app is refused rather than downgraded.
