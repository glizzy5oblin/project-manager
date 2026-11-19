# `.proj` file format

Each Project Manager project has one `<name>.proj` file in its private repository root. It is TOML. Project Manager edits it in place and keeps your comments and layout; only the values it changes are rewritten.

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
path = "."
branch = "main"
remote = "origin"

[workflow.public]
path = "../ExampleProject-public"
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

Precedence, later wins: built-in defaults (for example `.md` files other than README.md, CHANGELOG.md and LICENSE stay private), then `include`/`exclude`, then `overrides`. All paths are relative to the project and must not use `..` or be absolute. `!` is only allowed in `include`.

### `[release]`

| Key             | Default                  | Meaning                                                |
| --------------- | ------------------------ | ------------------------------------------------------ |
| `version_files` | `[]`                     | Files whose version string is bumped with the project. |
| `asset_name`    | `"{name}-{version}.zip"` | Release zip name. Placeholders: `{name}`, `{version}`. |
| `targets`       | `[]`                     | `[[release.targets]]` entries with a `kind`.           |

`kind = "github"` is the only target for now. Each kind may appear once.

## Errors

- Syntax and type errors give the line and column.
- Validation reports every problem at once, each with its key path, e.g. `workflow.public.path: required in two-repo mode`.
- When Project Manager saves an edited file it validates first and writes nothing if the result would not load. Writes are atomic (temporary file, then rename).

## Versions and migration

When the format changes, `schema_version` goes up and Project Manager reads older files by upgrading them in memory, and writes the upgraded form the next time it saves the file. A file with a newer `schema_version` than the running app is refused rather than downgraded.
