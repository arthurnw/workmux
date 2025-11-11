<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="meta/logo-dark.svg">
    <img src="meta/logo.svg" alt="workmux icon" width="300">
  </picture>
</p>

<p align="center">
  <strong>Git worktrees + tmux windows</strong>
</p>

---

Giga opinionated zero-friction workflow tool for managing git worktrees and
tmux windows as isolated development environments.

Perfect for running multiple AI agents in parallel without conflict.

## Philosophy

- **One worktree, one tmux window**: Each git worktree gets its own dedicated,
  pre-configured tmux window
- **Frictionless**: Multi-step workflows reduced to simple commands
- **Configuration as code**: Define your tmux layout and setup steps in
  `.workmux.yaml`

## Features

- Create git worktrees with matching tmux windows in a single command (`add`)
- Automatically set up your preferred pane layout (editor, shell, watchers,
  etc.)
- Run post-creation hooks (install dependencies, setup database, etc.)
- Copy or symlink configuration files (`.env`, `node_modules`) into new
  worktrees
- Merge branches and clean up everything (worktree, tmux window, branches) in
  one command (`merge`)
- List all worktrees with their tmux and merge status
- Bootstrap projects with an initial configuration file (`init`)
- Dynamic shell completions for branch names

## Installation

```bash
cargo install workmux
```

## Quick start

1. **Initialize configuration (optional)**:

```bash
workmux init
```

This creates a `.workmux.yaml` file to customize your workflow (pane layouts,
setup commands, file operations, etc.). workmux works out of the box with
sensible defaults, so this step is optional.

2. **Create a new worktree and tmux window**:

```bash
workmux add new-feature
```

This will:

- Create a git worktree at
  `<project_root>/../<project_name>__worktrees/new-feature`
- Create a tmux window named `new-feature`
- Automatically switch your tmux client to the new window

3. **When done, merge and clean up**:

```bash
# Run in the worktree window
workmux merge
```

Merges your branch into main and cleans up everything (tmux window, worktree,
and local branch).

## Configuration

workmux uses a two-level configuration system:

- **Global** (`~/.config/workmux/config.yaml`): Personal defaults for all
  projects
- **Project** (`.workmux.yaml`): Project-specific overrides

Project settings override global settings. For `post_create` and file operation
lists (`files.copy`, `files.symlink`), you can use `"<global>"` to include
global values alongside project-specific ones. Other settings like `panes` are
replaced entirely when defined in the project config.

### Global configuration example

`~/.config/workmux/config.yaml`:

```yaml
window_prefix: wm-
panes:
  - command: nvim .
    focus: true
  # Just a default shell (command omitted)
  - split: horizontal
post_create:
  - mise install
files:
  symlink:
    - node_modules
```

### Project configuration example

`.workmux.yaml`:

```yaml
post_create:
  - '<global>'
  - pnpm install

files:
  symlink:
    - '<global>' # Include global symlinks (node_modules)
    - .pnpm-store # Add project-specific symlink

panes:
  - command: claude
    focus: true
  - command: pnpm run dev
    split: horizontal
```

### Configuration options

- `main_branch`: Branch to merge into (optional, auto-detected from remote or
  checks for `main`/`master`)
- `worktree_dir`: Custom directory for worktrees (absolute or relative to repo
  root)
- `window_prefix`: Prefix for tmux window names (default: `wm-`)
- `panes`: Array of pane configurations
  - `command`: Optional command to run when the pane is created. If omitted,
    the pane starts with your default shell.
  - `focus`: Whether this pane should receive focus (default: false)
  - `split`: How to split from previous pane (`horizontal` or `vertical`)
- `post_create`: Commands to run after worktree creation (in the new worktree
  directory)
- `files`: File operations to perform on worktree creation
  - `copy`: List of glob patterns for files to copy
  - `symlink`: List of glob patterns for files/directories to symlink

**Default behavior:**

- Worktrees are created in `<project>__worktrees` as a sibling directory to your
  project by default
- If no `panes` configuration is defined, workmux provides sensible defaults:
  - For projects with a `CLAUDE.md` file: Opens `claude` in the first pane
  - For all other projects: Opens your default shell
  - Both configurations include a second pane split horizontally
- If no `post_create` configuration is defined, workmux automatically detects
  your package manager and runs the appropriate install command:
  - For pnpm projects (detects `pnpm-lock.yaml`): Runs `pnpm install`
  - To disable: Set `post_create: []` in your config
  - To override: Define custom `post_create` commands

### Shell alias (recommended)

For faster typing, alias `workmux` to `wm`:

```bash
alias wm='workmux'
```

## Commands

- [`add`](#workmux-add-branch-name) - Create a new worktree and tmux window
- [`merge`](#workmux-merge-branch-name) - Merge a branch and clean up everything
- [`remove`](#workmux-remove-branch-name) - Remove a worktree without merging
- [`list`](#workmux-list) - List all worktrees with status
- [`init`](#workmux-init) - Generate configuration file
- [`open`](#workmux-open-branch-name) - Open a tmux window for an existing
  worktree
- [`claude prune`](#workmux-claude-prune) - Clean up stale Claude Code entries
- [`completions`](#workmux-completions-shell) - Generate shell completions

### `workmux add <branch-name>`

Creates a new git worktree with a matching tmux window and switches you to it
immediately. If the branch doesn't exist, it will be created automatically.

- `<branch-name>`: Name of the branch to create or switch to.

**What happens:**

1. Creates a git worktree at
   `<project_root>/../<project_name>__worktrees/<branch-name>`
2. Creates a new tmux window named after the branch
3. Runs any configured file operations (copy/symlink)
4. Executes `post_create` commands if defined in config, or automatically runs
   `pnpm install` for pnpm projects (when `pnpm-lock.yaml` is detected)
5. Sets up your configured tmux pane layout
6. Automatically switches your tmux client to the new window

**Examples:**

```bash
# Create a new branch and worktree
workmux add user-auth

# Use an existing branch
workmux add existing-work

# Branch names with slashes work too
workmux add feature/new-api
```

---

### `workmux merge [branch-name]`

Merges a branch into the main branch and automatically cleans up all associated
resources (worktree, tmux window, and local branch).

- `[branch-name]`: Optional name of the branch to merge. If omitted,
  automatically detects the current branch from the worktree you're in.

**Common options:**

- `--ignore-uncommitted`: Commit any staged changes before merging without
  opening an editor
- `--delete-remote`, `-r`: Also delete the remote branch after a successful
  merge

**Merge strategies:**

By default, `workmux merge` performs a standard merge commit. You can customize
the merge behavior with these mutually exclusive flags:

- `--rebase`: Rebase the feature branch onto main before merging (creates a
  linear history via fast-forward merge). If conflicts occur, you'll need to
  resolve them manually in the worktree and run `git rebase --continue`.
- `--squash`: Squash all commits from the feature branch into a single commit on
  main. You'll be prompted to provide a commit message in your editor.

**What happens:**

1. Determines which branch to merge (specified branch or current branch if
   omitted)
2. Checks for uncommitted changes (errors if found, unless
   `--ignore-uncommitted` is used)
3. Commits staged changes if present (unless `--ignore-uncommitted` is used)
4. Merges your branch into main using the selected strategy (default: merge
   commit)
5. Deletes the tmux window (including the one you're currently in if you ran
   this from a worktree)
6. Removes the worktree
7. Deletes the local branch

**Typical workflow:**

When you're done working in a worktree, simply run `workmux merge` from within
that worktree's tmux window. The command will automatically detect which branch
you're on, merge it into main, and close the current window as part of cleanup.

**Examples:**

```bash
# Merge branch from main branch (default: merge commit)
workmux merge user-auth

# Merge the current worktree you're in
# (run this from within the worktree's tmux window)
workmux merge

# Rebase onto main before merging for a linear history
workmux merge user-auth --rebase

# Squash all commits into a single commit
workmux merge user-auth --squash

# Merge and also delete the remote branch
workmux merge user-auth --delete-remote
```

---

### `workmux remove <branch-name>` (alias: `rm`)

Removes a worktree, tmux window, and branch without merging. Useful for
abandoning work or cleaning up experimental branches.

- `<branch-name>`: Name of the branch to remove.

**Common options:**

- `--force`, `-f`: Skip confirmation prompt and ignore uncommitted changes
- `--delete-remote`, `-r`: Also delete the remote branch

**Examples:**

```bash
# Remove with confirmation if unmerged
workmux remove experiment

# Use the alias
workmux rm old-work

# Force remove without prompts
workmux rm -f experiment

# Force remove and delete remote branch
workmux rm -f -r old-work
```

---

### `workmux list` (alias: `ls`)

Lists all git worktrees with their tmux window status and merge status.

**Examples:**

```bash
# List all worktrees
workmux list
```

**Example output:**

```
BRANCH      TMUX    UNMERGED    PATH
------      ----    --------    ----
main        -       -           ~/project
user-auth   ✓       -           ~/project__worktrees/user-auth
bug-fix     ✓       ●           ~/project__worktrees/bug-fix
```

**Key:**

- `✓` in TMUX column = tmux window exists for this worktree
- `●` in UNMERGED column = branch has commits not merged into main
- `-` = not applicable

---

### `workmux init`

Generates `.workmux.yaml` with example configuration and `"<global>"`
placeholder usage.

**Examples:**

```bash
workmux init
```

---

### `workmux open <branch-name>`

Opens a new tmux window for a pre-existing git worktree, setting up the
configured pane layout and environment. This is useful if you accidentally
closed the tmux window for a worktree you are still working on.

- `<branch-name>`: Name of the branch that has an existing worktree.

**Common options:**

- `--run-hooks`: Re-runs the `post_create` commands (e.g., `pnpm install`).
- `--force-files`: Re-applies file copy/symlink operations. Useful for restoring
  a deleted `.env` file.

**What happens:**

1. Verifies that a worktree for `<branch-name>` exists and a tmux window does
   not.
2. Creates a new tmux window named after the branch.
3. (If specified) Runs file operations and `post_create` hooks.
4. Sets up your configured tmux pane layout.
5. Automatically switches your tmux client to the new window.

**Examples:**

```bash
# Open a window for an existing worktree
workmux open user-auth

# Open and re-run dependency installation
workmux open user-auth --run-hooks

# Open and restore configuration files
workmux open user-auth --force-files
```

---

### `workmux claude prune`

Removes stale entries from `~/.claude.json` that point to deleted worktree
directories. When you run Claude Code in worktrees, it stores configuration in
`~/.claude.json`. Over time, as worktrees are merged or removed, this file can
accumulate entries for paths that no longer exist.

**What happens:**

1. Scans `~/.claude.json` for entries pointing to non-existent directories
2. Creates a backup at `~/.claude.json.bak` before making changes
3. Removes all stale entries
4. Reports the number of entries cleaned up

**Safety:**

- Only removes entries for absolute paths that don't exist
- Creates a backup before modifying the file
- Preserves all valid entries and relative paths

**Examples:**

```bash
# Clean up stale Claude Code entries
workmux claude prune
```

**Example output:**

```
  - Removing: /Users/user/project__worktrees/old-feature

✓ Created backup at ~/.claude.json.bak
✓ Removed 3 stale entries from ~/.claude.json
```

---

### `workmux completions <shell>`

Generates shell completion script for the specified shell. Completions provide
tab-completion for commands and dynamic branch name suggestions.

- `<shell>`: Shell type: `bash`, `zsh`, or `fish`.

**Examples:**

```bash
# Generate completions for zsh
workmux completions zsh
```

See the [Shell Completions](#shell-completions) section for installation
instructions.

## Workflow example

Here's a complete workflow:

```bash
# Start a new feature
workmux add user-auth

# Work on your feature...
# (tmux automatically sets up your configured panes and environment)

# When ready, merge and clean up
workmux merge user-auth

# Start another feature
workmux add api-endpoint

# List all active worktrees
workmux list
```

## Why workmux?

`workmux` reduces manual setup to a pair of commands and makes it feasible to
run AI-driven development workflows in parallel.

### Without workmux

```bash
# 1. Manually create the worktree and environment
git worktree add ../worktrees/user-auth -b user-auth
cd ../worktrees/user-auth
cp ../../project/.env.example .env
ln -s ../../project/node_modules .
npm install
# ... and other setup steps

# 2. Manually create and configure the tmux window
tmux new-window -n user-auth
tmux split-window -h 'npm run dev'
tmux send-keys -t 0 'claude' C-m
# ... repeat for every pane in your desired layout

# 3. When done, manually merge and clean everything up
cd ../../project
git switch main && git pull
git merge --no-ff user-auth
tmux kill-window -t user-auth
git worktree remove ../worktrees/user-auth
git branch -d user-auth
```

### With workmux

```bash
# Create the environment
workmux add user-auth

# ... work on the feature ...

# Merge and clean up
workmux merge
```

### The parallel AI workflow (with workmux)

Delegate multiple complex tasks to AI agents and let them work at the same time.
This workflow is cumbersome to manage manually.

```bash
# Task 1: Refactor the user model (for Agent 1)
workmux add refactor/user-model

# Task 2: Build a new API endpoint (for Agent 2, in parallel)
workmux add feature/new-api

# ... Command agents work simultaneously in their isolated environments ...

# Merge each task as it's completed
workmux merge refactor/user-model
workmux merge feature/new-api
```

## Shell completions

To enable tab completions for commands and branch names, add the following to
your shell's configuration file.

For **bash**, add to your `.bashrc`:

```bash
eval "$(workmux completions bash)"
```

For **zsh**, add to your `.zshrc`:

```bash
eval "$(workmux completions zsh)"
```

For **fish**, add to your `config.fish`:

```bash
workmux completions fish | source
```

## Requirements

- Rust (for building)
- Git 2.5+ (for worktree support)
- tmux

## Inspiration and related tools

workmux is inspired by [wtp](https://github.com/satococoa/wtp), an excellent git
worktree management tool. While wtp streamlines worktree creation and setup,
workmux takes this further by tightly coupling worktrees with tmux window
management.

For managing multiple AI agents in parallel, tools like
[claude-squad](https://github.com/smtg-ai/claude-squad) and
[vibe-kanban](https://github.com/BloopAI/vibe-kanban/) offer dedicated
interfaces, like a TUI or kanban board. workmux takes a different approach:
**tmux is the interface**. If you already live in tmux, you don't need a new app
or abstraction layer. With workmux, managing parallel agents is managing tmux
windows.

## See also

- [tmux-bro](https://github.com/raine/tmux-bro)
- [tmux-file-picker](https://github.com/raine/tmux-file-picker)
