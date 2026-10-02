# annox for VS Code

A thin VS Code client for the annox language server. The server does all the annox work: storage, replay, anchoring, and conflicts. The extension shows annotations as comment threads and sends your actions.

Requires VS Code 1.100+ and the `annox` binary (see the [install script](https://github.com/gaardhus/annox#reference-implementation), or `cargo build --release` at the repository root).

## Setup

Install the `.vsix` attached to each [release](https://github.com/gaardhus/annox/releases) with **Extensions: Install from VSIX…**, or build it from this directory:

```sh
npm ci
npm run package        # writes annox-<version>.vsix
code --install-extension annox-*.vsix
```

The extension runs `annox lsp`. It finds `annox` through the `annox.path` setting, then on your `PATH`, then in `~/.local/bin`, where the install script puts it.

The extension starts when a workspace contains `.annox/annox.json`, and it attaches to the files inside it. To start annotating a project, run **annox: Initialize Workspace**. It creates the workspace at the git root of the current file, or the workspace folder if there's no git repository, after asking you to confirm.

Changes others make to `.annox/`, such as a `git pull`, show up without reloading.

## Threads

Annotations are comment threads, under their own **annox** comment provider, so they sit beside threads from other extensions such as GitHub Pull Requests. They also appear in the **Comments** panel.

- **To comment,** select text and click the **+** in the gutter, or run **annox: Comment**. Write the comment in the thread that opens, then press **Comment**, or **Save Draft** for a local-only draft.
- **To reply,** type in a thread's reply box.
- **To edit a comment,** use the pencil on it.
- **The buttons in a thread's header** act on the thread. A suggestion has *Accept*, *Reject*, and *Edit*, or *Re-target* once it's stale. A comment has *Resolve*, a closed thread *Reopen*, and a draft *Publish*. An orphaned comment has *Re-attach*, a conflicted annotation *Resolve Conflicts*, and every thread *History*.

Resolved threads and closed suggestions stay in the panel, marked resolved. Once their text is gone, as with an accepted suggestion whose text was replaced, they leave the editor and the Comments panel, and are listed only in the annox view.

The **annox** view in the panel lists the annotations of the open files: open ones in document order, then closed ones, newest first. Click one to go to it, or, if its text is gone, to read its thread and history.

In the editor, annotated text is tinted. Comments are blue, suggestions purple, and drafts green. A stale suggestion has a wavy yellow underline, and a conflicted annotation a red one. An annotation on a point between two characters shows as ◆. The first line of each comment, or each suggested replacement, is shown at the end of its line. Annotations whose text could not be found are counted in a notice at the top of the file. Click it to pick one.

## Commands

Every command is in the command palette under **annox:**, and the common ones are in the editor's context menu under **annox**. Commands act on the thread whose button you click, or else on the annotation under the cursor, asking if there are several.

| Command                                                            | Does                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Initialize Workspace                                               | Create an annox workspace for this project and restart the server.                                                                                                                                                                                       |
| Comment / Draft Comment                                            | Open a thread on the selection, or the cursor, to write a comment or a local-only draft in. Drafts are stored in the git-ignored `.annox/local/` and marked *draft*.                                                                                      |
| Suggest Replacement                                                | Suggest a replacement for the selection, or an insertion at the cursor. The prompt is pre-filled with the current text. For a selection spanning lines, the replacement opens in an editor. Save it (Ctrl+S) to send it.                                 |
| Toggle Suggestion Mode                                             | Turn suggestion mode on or off for the file (see below).                                                                                                                                                                                                  |
| Reply / Show Thread                                                | Open the thread under the cursor.                                                                                                                                                                                                                         |
| Edit                                                               | Edit the suggested text of a suggestion in an editor beside this one, where saving sends it, or the text of a comment in its thread.                                                                                                                     |
| Accept Suggestion / Reject Suggestion                              | Accept or reject the suggestion. Accepting edits the file, and you can undo as usual. With a selection, act on every suggestion touching it.                                                                                                              |
| Accept All Suggestions in File                                     | Accept every suggestion as one edit that a single undo reverts. If some suggestions moved because the text around them changed, you're asked once whether to accept them too. Stale or overlapping suggestions are skipped and reported.                 |
| Reject All Suggestions in File / Resolve All Threads in File       | Reject every suggestion, or resolve every comment thread, after one confirmation. They can be reopened one by one.                                                                                                                                       |
| Resolve Thread / Reopen                                            | Resolve or reopen a thread. With a selection, Resolve acts on every comment thread touching it.                                                                                                                                                          |
| Publish Draft / Publish All Drafts in File                         | Publish local drafts.                                                                                                                                                                                                                                     |
| Re-attach Comment to Selection                                     | Attach a comment whose text could not be found to the selection.                                                                                                                                                                                          |
| Re-target Suggestion at Selection                                  | Point a stale suggestion at the selection, and review its replacement.                                                                                                                                                                                   |
| Resolve Conflicts                                                  | Resolve a conflicting field by picking one of the competing values or writing a merged version. Run it again for any other conflicting fields. If an accepted suggestion loses, you're offered to revert its edit.                                       |
| Show History                                                       | Show every event of the annotation.                                                                                                                                                                                                                       |
| List Annotations / Show Annotations That Could Not Be Located      | Pick an annotation of the file and go to it.                                                                                                                                                                                                              |
| Commit Annotations                                                 | Commit the workspace's annotation files to git, and nothing else. It shows how many there are and asks you to confirm or edit the message, which summarizes them. It doesn't push.                                                                        |
| Restart Server                                                     | Restart `annox lsp`.                                                                                                                                                                                                                                      |

No keys are bound by default except Ctrl+Z in suggestion mode. To bind your own, open **Preferences: Open Keyboard Shortcuts** and search for `annox`. For example, in `keybindings.json`:

```json
{ "key": "ctrl+alt+m", "command": "annox.comment", "when": "editorTextFocus && annox.inWorkspace" },
{ "key": "ctrl+alt+s", "command": "annox.suggest", "when": "editorHasSelection && annox.inWorkspace" },
{ "key": "ctrl+alt+a", "command": "annox.acceptSuggestion", "when": "editorTextFocus && annox.inWorkspace" }
```

## Suggestion mode

**annox: Toggle Suggestion Mode**, or the pencil in the editor title bar, turns the file into suggestion mode, like "Suggesting" in online editors. You edit as usual, and once you stop typing for a moment, or save, your edits become suggestions and the file goes back to its original text. The file never contains suggested text, so saving is always safe.

- Suggestions are drawn inline: deleted text struck through, inserted text after it in green.
- Changes are widened to whole words: changing `pd` to `pl` suggests `pd` → `pl`, not `d` → `l`.
- Typing next to a suggestion you made in this session extends it. Delete a word and type its replacement right there, and you get one suggestion.
- To change text you've already suggested, use **Edit** on it. The inline green text can't hold the cursor.
- Ctrl+Z (Cmd+Z) undoes your last suggestion, instead of undoing edits to the file.
- Accepting a suggestion still edits the file.
- Suggestions are shared right away. Suggestion mode has no private-draft variant.

While suggestion mode is on, the status bar shows **Suggesting**, and the context key `annox.suggesting` is true.

When the workspace syncs through a hub, collaborators' cursors appear with their names, and your own cursor is shared unless `annox.presence` is off.

## Settings

| Setting                   | Default | Meaning                                                                                                                            |
| ------------------------- | ------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `annox.path`              | `""`    | The `annox` binary. If empty, it's looked up on `PATH` and in `~/.local/bin`.                                                      |
| `annox.author.id`         | `""`    | Your author id, e.g. `mailto:you@example.org`. If empty, the server uses `~/.config/annox/config.json`, then your git identity. |
| `annox.author.name`       | `""`    | Your display name.                                                                                                                 |
| `annox.presence`          | `true`  | Share your cursor with collaborators on a sync hub.                                                                                |
| `annox.labels`            | `true`  | Show comment text and suggested replacements at the end of lines.                                                                 |
| `annox.inlineSuggestions` | `false` | Always draw suggestions inline, not only in suggestion mode.                                                                       |
| `annox.suggestionDelay`   | `750`   | In suggestion mode, the pause in milliseconds after which your edits become suggestions.                                          |

The colors can be changed in `workbench.colorCustomizations`: `annox.commentBackground`, `annox.suggestionBackground`, `annox.draftBackground`, `annox.pointForeground`, `annox.deletionForeground`, `annox.insertionForeground`, `annox.labelForeground`, `annox.presenceForeground` and `annox.presenceBackground`.

## Tests

```sh
npm ci
npm run check          # type check
npm run test:unit      # suggestion mode's diffing
cargo build -p annox-lsp
ANNOX_BIN=$PWD/../../target/debug/annox npm run test:e2e
```

The end-to-end test downloads VS Code into `.vscode-test/` and runs it against a real `annox lsp` in a temporary workspace. On a machine without a display, run it under `xvfb-run -a`.
