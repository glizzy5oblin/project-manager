# Changelog

## v0.0.3 - 2026-11-28

Releases from inside the app: stage the private repo into the public one, then version, tag, zip and publish a GitHub release, with a checklist that shows what is left. History editing and remote operations arrive too.

### Releasing

- Staging tab: copies the allowlisted private files to the public side (two-repo or single-repo), with a leak scan that blocks keys, tokens, env files and decompiled code, a per-file diff, and the staged state of each commit in the graph.
- `[staging.map]` in the `.proj` publishes a private file under another name, such as `publish/Makefile` as the public `Makefile`.
- Release tab: version bump (`.proj` and configured version files), annotated tag with checks and optional push, release zip with a manifest and SHA-256 checksum, release notes generated from the commits since the last tag (editable), and GitHub release publishing with asset upload.
- Release checklist and history: every step reports whether it is done, and each tagged release is recorded in the `.proj`.

### History and remotes

- Cherry-pick, revert and rebase, with continue and abort.
- Interactive rebase: reorder, squash, fixup, drop and reword.
- Fetch, pull, push, force push with lease, and tag push.

### App

- Preferences dialog, backup settings, and backup retention by count or age with a cleanup preview.
- Theme editor with save as, import and export; the light theme now meets WCAG AA contrast.
- Command palette, keyboard focus, reduced motion, and clearer empty and loading states.
- Error reporting and logging, a diagnostics export, and export/import of the app's data folder.
- A Guide button and palette command that open the documentation.

### Fixes

- Commits, fetches and pushes made from the AppImage no longer fail or warn: git, gh and git hooks now run with the system's libraries instead of the AppImage's bundled ones.
- Release notes no longer go empty after the Tag step: once this version's tag is on HEAD, they run from the previous tag.
- Editing, deleting, uploading the zip to, and replacing the asset on a GitHub release work (they failed with "missing required key releaseId").
- The commit details panel can be dragged narrower; the danger button label in the sync bar is readable.
- Security: updated the `cookie` crate (RUSTSEC-2026-0594).

### Builds

- Linux AppImage built in a Debian 12 container; a manual release build for Windows, Linux and macOS in CI, and a public-side check on every push.

## v0.0.2 - 2026-11-22

Write actions arrive: the app can now change a repository, not only show it. Every risky action takes a restore point first, and Activity can undo it.

- Changes tab: stage, unstage (whole files or single hunks), discard and commit or amend, with a right-click menu on each file (including Add to .gitignore, Show in folder, Open in Files).
- Branches and history: create, rename, delete, check out and merge branches, and reset to a commit, from the commit details or a right-click menu. Stash: set work aside, apply, pop, drop.
- Merge conflicts: plain-words explanations, a button per disagreement (mine, theirs, both), an inline editor, and Save and mark resolved.
- Files tab: browse the project, edit text files with syntax colors, line numbers and find, and make new files and folders.
- Activity tab: the operation log (kept between runs) and restore points with a preview of what a restore changes.
- Open folder: from Overview, the project card on the hub, and any changed file.
- Layout: the Graph column resizes, the commit details can be closed (Esc), widen when buttons would be cut off, and every control has a hover description.
- Security: updated the `url` crate to 2.5.9 (RUSTSEC-2026-0592).

Not yet checked by hand in the running app: the Files tab, merge-conflict editing, Add to .gitignore, Open folder and the newest layout fixes. They are covered by automated tests only.

## v0.0.1 - 2026-11-19

- Initial project setup.
