# Write actions

Every change Project Manager makes to a repository goes through one pipeline in
`app/src/desktop/src/actions/`. The UI calls two commands: `action_preview`, then `action_run`.

## The pipeline

1. **Trust.** A repository owned by another user is refused unless the user confirmed it
   (`allow_untrusted`), the same check the read side uses.
2. **Validation.** Every argument is checked before git runs (see [Input rules](#input-rules)).
   The action is then planned: the repository is read, and the plan holds a title, the impact
   in plain words, and the exact git commands.
3. **Confirmation.** A dangerous action runs only with the `token` from its preview. The token
   is a hash of the action and the repository's state: HEAD, every branch, tag and
   remote-tracking ref, any merge in progress, and `git status`. If anything changed after the
   preview, the run fails with `stale` and the UI must preview again. So the user always
   confirms the impact that will actually happen.
4. **Backup.** A dangerous action takes a [backup](backups.md) first. If the backup fails, the
   action does not run. The failure is logged, and the error says that nothing was changed.
5. **Run and log.** The commands run in order. Each one is recorded in the operation log. When a
   command fails after a backup was taken, the error names the backup so the user can undo.

## Actions

| Action              | Dangerous | git                                                                                      |
| ------------------- | --------- | ---------------------------------------------------------------------------------------- |
| `stage`             |           | `add -A -- <paths>`                                                                      |
| `unstage`           |           | `reset -q -- <paths>`                                                                    |
| `stageHunks`        |           | `apply --cached --whitespace=nowarn -` (patch on stdin)                                  |
| `unstageHunks`      |           | the same, with the reverse of the chosen staged hunks                                    |
| `discard`           | yes       | `checkout -q -- <tracked>`, `clean -f -q -- <untracked>`                                 |
| `commit`            |           | `commit -q --cleanup=whitespace -F -` (message on stdin)                                 |
| `commit` amend      | yes       | the same with `--amend`; warns if the commit is already pushed                           |
| `createBranch`      |           | `branch <name> <start>` or `checkout -q -b <name> <start>`                               |
| `renameBranch`      |           | `branch -m <from> <to>`                                                                  |
| `deleteBranch`      | yes       | `branch -D -q <name>`; counts commits found on no other ref                              |
| `checkout`          |           | `switch -q --no-guess <branch>`                                                          |
| `merge`             |           | `merge --no-edit -q <branch>`; stops on conflicts                                        |
| `abortMerge`        | yes       | `merge --abort`                                                                          |
| `cherryPick`        |           | `cherry-pick --no-rerere-autoupdate <commit>`; stops on conflicts                        |
| `revert`            |           | `revert --no-edit --no-rerere-autoupdate [-m 1] <commit>`                                |
| `rebase`            | yes       | `rebase --no-autostash --no-rerere-autoupdate -q <onto>`                                 |
| `rebaseInteractive` | yes       | `rebase -i --no-autostash --no-rerere-autoupdate --empty=drop -q <base>`; see below      |
| `abortCherryPick`   | yes       | `cherry-pick --abort`                                                                    |
| `abortRevert`       | yes       | `revert --abort`                                                                         |
| `abortRebase`       | yes       | `rebase --abort`                                                                         |
| `continue`          |           | `cherry-pick/revert/rebase --continue`; may stop again                                   |
| `reset` (3 modes)   | yes       | `reset -q --soft/--mixed/--hard <commit> --`                                             |
| `stashPush`         |           | `stash push -q [--include-untracked] [--staged] [-m <msg>]`                              |
| `stashApply`        |           | `stash apply -q [--index] stash@{n}`; stops on conflicts                                 |
| `stashPop`          |           | `stash pop -q [--index] stash@{n}`; kept after a conflict                                |
| `stashDrop`         | yes       | `stash drop -q stash@{n}`                                                                |
| `stashClear`        | yes       | `stash clear`                                                                            |
| `fetch`             |           | `fetch --progress --prune <remote>` or `--all`                                           |
| `pull`              |           | `pull --progress --no-edit --no-rebase --ff <remote> refs/heads/<b>`; stops on conflicts |
| `pull` rebase       | yes       | `pull --progress --no-edit --rebase --no-autostash <remote> refs/heads/<b>`              |
| `push`              |           | `push --progress [--set-upstream] <remote> refs/heads/<b>:refs/heads/<rb>`               |
| `push` force        | yes       | the same with `--force-with-lease=refs/heads/<rb>:<sha seen at preview>`                 |
| `pushTag`           |           | `push --progress <remote> refs/tags/<t>:refs/tags/<t>`                                   |

`Action::danger` is the single list of dangerous actions. It has no wildcard arm, so a new
action does not compile until it is classed as dangerous or safe. Its value is the backup's action
name (`reset-hard`, `amend`, and so on).

`stageHunks` and `unstageHunks` take one path and the ids of the hunks to move. `work_diff`
gives each hunk an id: a hash of its header and lines, computed with 3 lines of context. The
action computes the diff again the same way. If any id is no longer there (the file changed after
the diff was shown), it fails with `stale` and stages nothing. The patch is built from the
file's bytes, not the text shown, so line endings, a missing final newline and text that is not
UTF-8 are kept. It is applied to the index only, and the file on disk is never touched. The op
log records the command line only, never the patch. These files are refused, with a hint to
stage the whole file instead:

- new, deleted or type-changed files;
- symlinks, binary files and files too large for a line diff;
- conflicted files;
- files with a `filter` (such as Git LFS), `ident` or `working-tree-encoding` attribute;
- files with CRLF on disk over LF in the index, unless `core.autocrlf` or a `text`/`eol`
  attribute makes git convert them.

### Cherry-pick, revert and rebase

These live in `actions/replay.rs`. While a merge, cherry-pick, revert, rebase, `git am` or bisect is
in progress, every action that starts something new (merge, amend, stash, cherry-pick, revert,
rebase) is refused with the name of what is in progress (`require_idle`). Cherry-pick refuses a
commit already on the current branch, a merge commit and a root commit. Revert refuses a commit
that is not on the current branch and a root commit, and reverts a merge against its first parent.
Rebase needs a branch checked out and no tracked changes; it refuses when the branch already
contains the target. Its impact counts the commits replayed, says they get new ids, that merges in
between are left out, and warns when those commits are already on a remote (a force-push would be
needed). `continue` refuses while a file is still in conflict, and is not used for merges, which
finish by committing. An abort takes a backup first; restoring the backup of an aborted rebase
brings back the files and branches, but not the rebase itself.

During a rebase git calls the branch being rebased onto "ours" and your commit "theirs".
`conflict_info` turns this round (`swapped: true`, and `during` names the operation), so "you"
is always your own work, and `resolve` with `keep: ours` keeps your version. The conflict editor
uses `yourRegions` and `textChoice` (`conflicts.ts`) to do the same for the marker text.

A merge that stops on conflicts is not an error: the outcome lists the conflicted files and the
repository stays mid-merge. Discarding work keeps staged changes. Discard refuses conflicted
paths and paths that only have staged changes, so it never touches the index.

### Editing commits (interactive rebase)

`rebaseInteractive { base, todo }` rewrites the current branch's commits after `base` (all of
them, from the first, when `base` is `null`). `todo` lists each of those commits once, oldest
first, in the new order, as `pick`, `squash`, `fixup` or `drop`; a `pick` may carry a new
`message`. It lives in `actions/interactive.rs`, and the editor gets the commits from the
`rebase_range { id, from }` command, which starts at `from` and checks it can be edited.

It is refused, before anything runs, when: HEAD is not on a branch; something is in progress;
tracked files have changes (untracked files are fine); `base` is not on the branch or is its tip;
there is a merge among the commits (or `from` is one); there are more than 500 commits; a commit
is missing from the list, listed twice or not one of the commits; a squash or fixup has no kept
commit before it; every commit is dropped (a reset does that); a message is empty, contains a NUL
or is given on a commit that is not kept; or nothing would change.

git's own editor is never opened, so the run is the same on every platform:

- The todo list is written as a blob before the rebase starts, and the rebase runs with
  `GIT_SEQUENCE_EDITOR="git cat-file blob <id> >"`, which puts it in place of git's list. The
  blob's id is computed at preview time, so the commands shown are the commands that run.
- A new message, or the message of a squashed group, goes into a copy of the commit object with
  the same tree, parent, author and committer and the new message (a signature is left out,
  since it would no longer match), written the same way. The list picks the copy. Squashes are
  sent to git as `fixup` lines after the group's first commit, so git's message editor is never
  needed and the group's message is the copy's: the commits' own messages, joined by blank
  lines, unless a new message was given for the group.
- `rebase.missingCommitsCheck=ignore`, `rebase.updateRefs=false` and `rebase.rebaseMerges=false`
  are set with `-c` so a user's settings cannot change the result (older git ignores keys it does
  not know; the oldest git supported is 2.30).

The impact says whether the order changes and which commits get a new message, are folded or
are left out (the backup keeps them), how many get new ids (those before the first change keep theirs), when some are
already on a remote (publishing needs a force push), and which other local branches still have
the old ones. A conflict stops the rebase like any other: settle the files in Changes, then
Continue or Abort. While it is stopped, the conflict panel calls the other side "the commits
before it", since the branch is being rebased onto its own past.

Known git behaviour: when a commit stops on a conflict, `rebase --continue` tidies its message as
`git commit` does, which drops lines starting with `#`. This happens to any commit that stops on
a conflict in a rebase, not only to one given a new message.

### Resolving a conflict

`resolve { path, keep }` settles one conflicted file and marks it resolved. `keep` is `ours` or
`theirs` (`git checkout --ours/--theirs`, then `git add`; refused when that side has no such
file), `delete` (`git rm`), or `edited { content }`, which writes the text first. The file must be
in conflict. A restore point (`resolve`) is taken first, without a confirmation, so Activity can
undo it. `conflict_info` explains a file in conflict: its kind, the branch names, the regions
between markers and the text for the inline editor (text files up to 2 MiB). The Changes view uses
both to settle each disagreement with a button or by editing the text.

`ignore { pattern }` appends one line to `.gitignore` (created if missing; refused when the line is
already there, empty, or starts with `#` or `!`). It is safe and takes no restore point; the file
is written by the pipeline like an edited resolve.

### Files view

`files_list`, `file_read` and `file_write` (`src/files.rs`) back the Files tab: a folder tree and a
text editor. Paths are relative to the project, checked like action paths, and resolved through
symlinks so nothing outside the project can be read or written; `.git` is never listed. Files over
2 MiB, with NUL bytes or not UTF-8 are shown with the reason and not opened. Saving is the user's
own edit, so it takes no restore point. "Open in Files" is in the Changes right-click menu and
under the conflict editor.

### Stash

A stash is named by its commit id, never by `stash@{n}`: positions shift when another stash is
made or dropped, so a position chosen in the UI could name a different stash by the time the action
runs. The position is looked up when the commands are planned, and a stash that is no longer in the
list fails with `notFound`. `stash_list` returns the list (newest first) with each stash's branch,
message, time and file counts.

- Push refuses when there is nothing to stash, when a merge is in progress or files have conflicts,
  and on a repository with no commits. `stagedOnly` (`--staged`) needs git 2.35 and cannot be
  combined with `includeUntracked`. The message is one line.
- Apply and pop refuse during a merge or with conflicted files. A conflict is an outcome, not an
  error: the files are listed, and pop keeps the stash, as git does. When git would overwrite local
  changes it stops and changes nothing; that is an ordinary git error.
- Drop and clear are dangerous. The backup keeps every stash commit under
  `refs/pm/backups/<id>/stash/<n>`, and a restore puts the list back (see `backups.md`).
- Stash commands run with `GIT_LITERAL_PATHSPECS=0`. With it on, `stash push --include-untracked`
  stashed the untracked files but left them in the working tree. No stash command takes a path.

Stashing is safe, so it asks for nothing, but it still takes a backup first (`Action::auto_backup`)
so that Activity can undo it. Activity lists the restore points (`backups_list`), shows what a
restore would change (`backup_preview`, `Backups::preview`) and runs it (`backup_restore`, a task
that takes a new `restore` backup first; see `backups.md`).

### Remotes

Fetch, pull and push run as tasks with `--progress`, so the task bar shows how far they are and
Cancel stops them. Credentials never pass through the app: git uses the user's own credential
helper or SSH agent, with `GIT_TERMINAL_PROMPT=0`, and an authentication failure comes back with a
hint. Only the `file`, `git`, `http`, `https` and `ssh` protocols are allowed.

- A remote name must be one of the configured remotes. Push goes to the branch's upstream, else to
  the only remote; with several remotes and no upstream it asks the user to choose. A branch pushed
  for the first time gets `--set-upstream`.
- Pull needs an upstream and no operation in progress. The preview counts, as of the last fetch,
  the commits to come in and the branch's own. Pull with rebase refuses tracked changes, like
  `rebase`, and is dangerous because it rewrites the branch's own commits.
- Push refuses, before git runs, when there is nothing to push and when the remote has commits the
  branch lacks ("pull first, or force push").
- Force push is dangerous. It refuses when nothing on the remote would be replaced (use Push) and
  when the branch is not on the remote yet. The dialog says which commits are replaced, that anyone
  who already has them must reset to the new tip, and warns when the branch is the remote's main
  branch (its `HEAD`, else `main` or `master`). The lease names the exact commit the preview saw,
  so a push someone made after the last fetch is never overwritten: git refuses with "stale info"
  and the error says to fetch and look again. The backup keeps every remote-tracking tip under
  `refs/pm/backups/<id>/remotes/`, so the replaced commits stay reachable.
- A rejection from the remote (stale lease, non-fast-forward, an existing tag, a protected branch)
  comes back in plain words, with git's own report as the detail.

In the app, the sync bar at the top of Changes shows the branch, its upstream and how far apart
they are, with Fetch, Pull, Pull (rebase), Push, Force push and Push tag.

## Input rules

- **Paths** are relative to the repository root, use `/`, and have no empty, `.` or `..` parts,
  no backslash, NUL or drive letter, and no `.git` component (any case, trailing dots or
  spaces ignored). They are passed after `--` with `GIT_LITERAL_PATHSPECS=1`, so `*.txt` or
  `:(glob)` mean exactly that file name.
- **Branch names** must pass `git check-ref-format --branch` rules and may not be `HEAD` or
  start with `-`.
- **Revisions** (reset target, branch start) may not be empty, start with `-`, or hold control
  characters. They are resolved with `rev-parse --verify --end-of-options <rev>^{commit}`, and
  the command gets the full commit id, never the user's text.
- Commit messages go to git on stdin.

## Operation log

The app keeps the latest 500 entries in memory and in `oplog.jsonl` in the app data folder (one
JSON entry per line, so they survive a restart and "Clear log" empties the file too). It copies
them to the app log, and sends each one
to the window as an `op-log` event. An entry holds the repository, the action, the command as it
would be typed, the exit code, the duration, and git's error output (shortened to 4000 bytes).
Command lines and errors are redacted by the git runner, so URL credentials never appear. Commit
messages never appear either, because they are not on the command line. Backups are logged as
their own entries. The `op_log` command returns the entries and `op_log_clear` empties the log.

## Tasks

`action_run` starts a task locked on the repository path, so two write actions never run on the
same repository at once. Cancelling works between steps. After an action that took a backup,
old backups are pruned with the default retention (50 backups, 30 days).

## In the app

All write buttons call `actions.run` in `app/src/ui/lib/actions.svelte.ts`. It previews the
action and runs it at once when it is safe. It shows the confirmation dialog
(`ActionDialog.svelte`) first when the action is dangerous or the repository is untrusted. The
dialog lists the impact, says that a backup is taken (its name is then listed in Activity,
under "Undo: restore points", where it can be restored), and shows the exact commands under "Commands". Only a dangerous run carries the preview's token. When the run fails with `stale`, the
runner previews again and asks again, with a note that the repository changed (at most three
times). Only one dialog shows at a time, and a second request while one is open is refused. While
an action runs, the project's write buttons are disabled. After a run, a status line says what
was done and names the backup. If conflicts were left, it is shown as a warning and says how many
files to resolve in Changes.

- **Changes** view (`ChangesView.svelte`): conflicted, staged and unstaged files, with stage,
  unstage and discard for one file or all of them. It also has the commit message editor
  (Ctrl/Cmd+Enter commits, and the summary line warns above 72 characters) and "Amend last
  commit", which fills in the last message. During a cherry-pick, revert or rebase it offers
  Continue (greyed out while files are in conflict) and "Abort cherry-pick/revert/rebase"
  (`replayControls` in `changes.ts`). When a merge is in progress it offers "Abort merge",
  fills the editor with git's merge message (`RepoState.mergeMessage`: `MERGE_MSG` without its
  `#` lines, read only during a merge, only from a regular file of at most 64 KiB). Amend and
  stash are disabled while any of these is in progress.
  Each hunk of a modified text file has a "Stage hunk" button (or "Unstage hunk" on the
  staged side). It shows only when the whole hunk is on screen, and the diff reloads after it.
  The selected file's diff comes from `work_diff`, which compares HEAD with the index (staged) or
  the index with the working tree (unstaged). It uses the same size and binary limits as commit
  diffs, takes CRLF in the working tree as LF when the index has none, and refuses to follow a
  symlink in a folder of the path.
- **Stashes** (bottom of the Changes list, `StashPanel.svelte`): "Stash…" opens a form with a
  message, "Include untracked files" and "Only the staged changes" (hidden below git 2.35; the two
  options exclude each other). The button is disabled, with the reason as its tooltip, when there
  is nothing to stash, a merge is in progress, files have conflicts, or there is no commit yet.
  Selecting a stash shows its files in the right pane (`StashDiff.svelte`): tracked changes
  against the commit it was made on, and untracked files (stored in the stash's third parent) as
  added. The selected stash has Apply, Pop and Drop; "Restore the staged state when applying" adds
  `--index`. "Clear all" shows with two or more stashes. Drop and clear use the confirmation
  dialog and a backup.
- **Commit panel** (Graph and Log): new branch at the selected commit, reset the current branch
  to it (soft, mixed or hard), and check out, merge, rename or delete each branch that points at
  it. You can also merge a remote-tracking branch into the current branch. Merge buttons are
  hidden at the current branch's own commit, where a merge would change nothing. The graph
  rebuilds when HEAD moves to another branch at the same commit (`GraphKey.head_ref`), so the
  HEAD label and these buttons follow a checkout.
- **Edit commits** ("Edit commits from here…" in the commit panel and the commit's right-click
  menu, `RebaseEditor.svelte` with the list logic in `lib/rebaseTodo.ts`): the branch's commits
  from the selected one on, oldest first. Each row has up and down buttons, Keep, Squash, Fixup
  or Drop, and "Message…", which opens the message of a kept commit (for a group, the combined
  message; "Reset message" goes back to it). The list says when commits are already pushed, and
  Apply stays disabled, with the reason, until something changes and the list can run. Apply
  goes through the confirmation dialog, which opens over the editor. A merge commit, a commit off
  the branch or a detached HEAD is refused with the reason.
- **Right-click on a commit** (Graph and Log): the same actions as the commit panel, as a menu. It
  selects the commit and hands the choice to the panel, so confirmation, backup, results and
  errors work as for the panel's buttons. The menu key or Shift+F10 opens it for the selected
  commit.
- **Activity** view (`ActivityView.svelte`): the operation log for the project, or for all
  projects, newest first. It updates live from `op-log` events, and failed commands show git's
  error.
