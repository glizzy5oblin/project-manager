# Changelog

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
