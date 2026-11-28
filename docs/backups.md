# Backups

Project Manager takes a backup before every dangerous action (reset, rebase, force-push, branch delete,
history rewrite and so on). The user can undo the action by restoring that backup. The code is
in `app/src/desktop/src/backup/`.

## What a backup holds

A backup with id `<id>` is a set of refs under `refs/pm/backups/<id>/`:

| Ref               | Contents                                                                                                                    |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `head`            | The commit HEAD pointed at. Absent on an unborn branch.                                                                     |
| `heads/<branch>`  | Every local branch tip.                                                                                                     |
| `tags/<tag>`      | Every tag. Annotated tags keep their tag object.                                                                            |
| `index`           | A commit whose tree is the staged index, conflicted entries left out.                                                       |
| `pending/<n>`     | The commits a merge, cherry-pick or revert in progress was applying.                                                        |
| `blobs/<n>`       | The file versions of each conflicted entry (base, ours, theirs).                                                            |
| `stash/<n>`       | Each stash commit, newest first, so a dropped stash is not lost.                                                            |
| `worktree`        | A commit whose tree is the whole working tree, untracked files included.                                                    |
| `remotes/<r>/<b>` | Every remote-tracking branch tip, so a force push never loses what it replaced. Not restored: these refs follow the remote. |

Ignored files are not backed up. Nothing an action does touches them either, except
`git clean -x`, which Project Manager does not run.

The working-tree snapshot is built with a temporary index file (`GIT_INDEX_FILE`), so the
user's real index and `refs/stash` are never changed. Because the snapshot lives under a ref,
`git gc` keeps every object it needs, whatever happens to branches and reflogs.

The id is `<UTC time>-<action>`, for example `20261119-142501-reset-hard`. A suffix (`-2`, `-3`)
is added when two backups are taken in the same second.

## Backup index

`<git dir>/pm/backups.json` lists the backups for the Undo UI: id, action, time, HEAD branch
and commit, branch and tag tips, untracked files, the conflicted index entries (as
`git ls-files -u` prints them), the operation in progress (`MERGE_HEAD`, `CHERRY_PICK_HEAD`,
`REVERT_HEAD`, `MERGE_MSG`, `MERGE_MODE`), whether the repo uses Git LFS, and the bundle path if one
was written. It lives in the git dir so it is never
committed and moves with the repository. If a backup fails part-way, its refs are deleted and no
entry is written.

## Bundles (optional)

When a bundle folder is configured, the backup refs are also written to
`<folder>/<id>.bundle` and checked with `git bundle verify`. A bundle survives
deleting the repository, or losing the refs. `Backups::import_bundle` fetches the refs back from
the bundle, after which the backup can be restored as normal.

## Restore

Restoring takes a new backup first (action `restore`), so a restore can itself be undone. Then:

1. A rebase, `git am`, or multi-commit cherry-pick or revert in progress is stopped, and the
   outcome's warnings say so. A pending merge or single cherry-pick is cleared by the reset in
   step 4. A bisect is left running; it only records commits.
2. Branches and tags are reset to their backed-up tips. Branches and tags created after the
   backup are left alone.
3. HEAD is pointed back at the backed-up branch, or detached at the backed-up commit.
4. The working tree is reset and cleaned (ignored files are kept), then the snapshot is checked
   out over it. Files that were deleted at backup time are removed again.
5. The index is set back to the backed-up index commit, and the conflicted entries are put back
   with their stages (`git update-index --index-info`).
6. A merge, cherry-pick or revert that was in progress is put back (`MERGE_HEAD`, `MERGE_MSG`,
   `MERGE_MODE`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`), so it can be finished or aborted as before.
   The index is refreshed.
7. The stash list is put back as it was, with stashes made since kept on top. Git can only
   push a stash onto the top of the list, so when the list differs it is emptied and stored
   again, oldest first, with their original messages. Every stash commit stays reachable from
   the backups meanwhile.

So undoing "Abort the merge" returns the repository to mid-merge: the same conflicts, the
resolutions already made, and `git commit` concluding the merge. A rebase or sequencer in progress
is not brought back (step 1); its own state lives in folders that change at every step.

Backups taken by older versions with a conflicted index have no `index` ref. Restoring one puts
back the conflicted file contents (with their markers) and warns that the conflicts are no longer
marked in the index.

Git hooks never run during backup or restore: `core.hooksPath` points at `<git dir>/pm/no-hooks`,
an empty regular file, so git finds no hooks and tools that install hooks there (git-lfs) cannot
write any. Commit signing is disabled for the snapshot commits.

## Retention

`Backups::prune` deletes backups older than `max_age_days`, then the oldest beyond
`max_count`. The defaults are 50 backups and 30 days. Either limit can be turned off.

## Git LFS

`git add` runs the LFS clean filter while the snapshot is taken, so changed LFS file contents are
stored by git-lfs in `.git/lfs/objects` and the snapshot holds pointers. Restore checks the
pointers out through the smudge filter. A bundle carries only the pointers: restoring an LFS repo
from a bundle on another machine needs the LFS objects from the remote.

## Security

- Restore writes and deletes only paths inside the working tree. A path whose parent is a
  symlink, or that contains `..` or is absolute, is refused (`safe_join`).
- Action names are limited to `[a-z0-9-]`, 1 to 40 characters, not starting with `-`, so they
  are safe in ref names and file names.
- Hooks are disabled for every git command the service runs.
