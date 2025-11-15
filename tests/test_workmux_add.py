import os
import shlex
from pathlib import Path

import pytest

from .conftest import (
    TmuxEnvironment,
    create_commit,
    get_window_name,
    get_worktree_path,
    poll_until,
    run_workmux_add,
    run_workmux_command,
    write_workmux_config,
    write_global_workmux_config,
)


def install_fake_agent(env: TmuxEnvironment, name: str, script_body: str) -> Path:
    """Creates a fake agent command in PATH so tmux panes can invoke it."""
    bin_dir = env.tmp_path / "agents-bin"
    bin_dir.mkdir(exist_ok=True)
    script_path = bin_dir / name
    script_path.write_text(script_body)
    script_path.chmod(0o755)

    new_path = f"{bin_dir}:{env.env.get('PATH', '')}"
    env.env["PATH"] = new_path
    env.tmux(["set-environment", "-g", "PATH", new_path])
    return script_path


def test_add_creates_worktree(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` creates a git worktree."""
    env = isolated_tmux_server
    branch_name = "feature-worktree"

    write_workmux_config(repo_path)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify worktree in git's state
    worktree_list_result = env.run_command(["git", "worktree", "list"])
    assert branch_name in worktree_list_result.stdout

    # Verify worktree directory exists
    expected_worktree_dir = get_worktree_path(repo_path, branch_name)
    assert expected_worktree_dir.is_dir()


def test_add_creates_tmux_window(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` creates a tmux window with the correct name."""
    env = isolated_tmux_server
    branch_name = "feature-window"
    window_name = get_window_name(branch_name)

    write_workmux_config(repo_path)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify tmux window was created
    list_windows_result = env.tmux(["list-windows", "-F", "#{window_name}"])
    existing_windows = list_windows_result.stdout.strip().split("\n")
    assert window_name in existing_windows


def test_add_inline_prompt_injects_into_claude(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Inline prompts should be written to PROMPT.md and passed to claude via command substitution."""
    env = isolated_tmux_server
    branch_name = "feature-inline-prompt"
    prompt_text = "Implement inline prompt"
    output_filename = "claude_prompt.txt"

    fake_claude_path = install_fake_agent(
        env,
        "claude",
        f"""#!/bin/sh
# Debug: log all arguments
echo "ARGS: $@" > debug_args.txt
echo "ARG1: $1" >> debug_args.txt
echo "ARG2: $2" >> debug_args.txt

set -e
# The implementation calls: claude "$(cat PROMPT.md)"
# So we expect the prompt content as the first argument
printf '%s' "$1" > "{output_filename}"
""",
    )

    # Use absolute path to ensure we use the fake claude
    write_workmux_config(repo_path, panes=[{"command": str(fake_claude_path)}])

    run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"add {branch_name} --prompt '{prompt_text}'",
    )

    worktree_path = get_worktree_path(repo_path, branch_name)
    # Prompt file is now written to /tmp instead of the worktree
    prompt_file = Path(f"/tmp/workmux-prompt-{branch_name}.md")
    assert prompt_file.exists(), f"Prompt file not found at {prompt_file}"
    assert prompt_file.read_text() == prompt_text

    agent_output = worktree_path / output_filename
    debug_output = worktree_path / "debug_args.txt"

    def agent_wrote_prompt():
        return agent_output.exists()

    if not poll_until(agent_wrote_prompt, timeout=2.0):
        # Print debug info if test fails
        print(f"\nWorktree path: {worktree_path}")
        print(f"Files in worktree: {list(worktree_path.iterdir())}")
        if debug_output.exists():
            print(f"\nDebug output:\n{debug_output.read_text()}")
        else:
            print("\nDebug output file not found - script never executed")

        # Check tmux pane output
        import subprocess

        pane_content = env.tmux(["capture-pane", "-t", f"=wm-{branch_name}.0", "-p"])
        print(f"\nTmux pane content:\n{pane_content.stdout}")

        assert False, "Agent output not found"

    assert agent_output.read_text() == prompt_text


def test_add_prompt_file_injects_into_gemini(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Prompt file flag should populate PROMPT.md and pass it to gemini via command substitution."""
    env = isolated_tmux_server
    branch_name = "feature-file-prompt"
    prompt_source = repo_path / "prompt_source.txt"
    prompt_source.write_text("File-based instructions")
    output_filename = "gemini_prompt.txt"

    fake_gemini_path = install_fake_agent(
        env,
        "gemini",
        f"""#!/bin/sh
set -e
# The implementation calls: gemini -i "$(cat PROMPT.md)"
# So we expect -i flag first, then the prompt content as the second argument
if [ "$1" != "-i" ]; then
    echo "Expected -i flag first" >&2
    exit 1
fi
printf '%s' "$2" > "{output_filename}"
""",
    )

    # Use absolute path to ensure we use the fake gemini
    write_workmux_config(repo_path, panes=[{"command": str(fake_gemini_path)}])

    run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"add {branch_name} --prompt-file {shlex.quote(str(prompt_source))}",
    )

    worktree_path = get_worktree_path(repo_path, branch_name)
    # Prompt file is now written to /tmp instead of the worktree
    prompt_file = Path(f"/tmp/workmux-prompt-{branch_name}.md")
    assert prompt_file.exists(), f"Prompt file not found at {prompt_file}"
    assert prompt_file.read_text() == prompt_source.read_text()

    agent_output = worktree_path / output_filename

    def agent_wrote_prompt():
        return agent_output.exists()

    assert poll_until(agent_wrote_prompt, timeout=2.0), (
        "Prompt content was not passed to gemini"
    )
    assert agent_output.read_text() == prompt_source.read_text()


def test_add_executes_post_create_hooks(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` executes post_create hooks in the worktree directory."""
    env = isolated_tmux_server
    branch_name = "feature-hooks"
    hook_file = "hook_was_executed.txt"

    write_workmux_config(repo_path, post_create=[f"touch {hook_file}"])

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify hook file was created in the worktree directory
    expected_worktree_dir = get_worktree_path(repo_path, branch_name)
    assert (expected_worktree_dir / hook_file).exists()


def test_add_without_prompt_skips_prompt_file(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Worktrees created without prompt flags should not create PROMPT.md."""
    env = isolated_tmux_server
    branch_name = "feature-no-prompt"

    write_workmux_config(repo_path, panes=[])

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_path = get_worktree_path(repo_path, branch_name)
    # Verify no PROMPT.md in worktree
    assert not (worktree_path / "PROMPT.md").exists()
    # Verify no prompt file in /tmp either
    prompt_file = Path(f"/tmp/workmux-prompt-{branch_name}.md")
    assert not prompt_file.exists()


def test_add_executes_pane_commands(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` executes commands in configured panes."""
    env = isolated_tmux_server
    branch_name = "feature-panes"
    window_name = get_window_name(branch_name)
    expected_output = "test pane command output"

    write_workmux_config(
        repo_path, panes=[{"command": f"echo '{expected_output}'; sleep 0.5"}]
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify pane command output appears in the pane
    def check_pane_output():
        capture_result = env.tmux(["capture-pane", "-p", "-t", window_name])
        return expected_output in capture_result.stdout

    assert poll_until(check_pane_output, timeout=2.0), (
        f"Expected output '{expected_output}' not found in pane"
    )


def test_add_copies_directories(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that directory copy rules replicate nested contents into the worktree."""
    env = isolated_tmux_server
    branch_name = "feature-copy-dir"
    shared_dir = repo_path / "shared-config"
    nested_dir = shared_dir / "nested"

    nested_dir.mkdir(parents=True)
    (shared_dir / "root.txt").write_text("root-level")
    (nested_dir / "child.txt").write_text("nested-level")

    write_workmux_config(repo_path, files={"copy": ["shared-config"]})

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_path = get_worktree_path(repo_path, branch_name)
    copied_dir = worktree_path / "shared-config"

    assert copied_dir.is_dir()
    assert (copied_dir / "root.txt").read_text() == "root-level"
    assert (copied_dir / "nested" / "child.txt").read_text() == "nested-level"


def test_add_sources_shell_rc_files(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that shell rc files (.zshrc) are sourced and aliases work in pane commands."""
    env = isolated_tmux_server
    branch_name = "feature-aliases"
    window_name = get_window_name(branch_name)
    alias_output = "custom_alias_worked_correctly"

    # The environment now provides an isolated HOME directory.
    # Write the .zshrc file there.
    zshrc_content = f"""
# Test alias
alias testcmd='echo "{alias_output}"'
"""
    (env.home_path / ".zshrc").write_text(zshrc_content)

    write_workmux_config(repo_path, panes=[{"command": "testcmd; sleep 0.5"}])

    # The HOME env var is already set for the tmux server.
    # We still need to ensure the correct SHELL is used if it's non-standard.
    shell_path = os.environ.get("SHELL", "/bin/zsh")
    pre_cmds = [
        ["set-option", "-g", "default-shell", shell_path],
    ]

    # Run workmux add. No pre-run `setenv` for HOME is needed anymore.
    run_workmux_add(
        env, workmux_exe_path, repo_path, branch_name, pre_run_tmux_cmds=pre_cmds
    )

    # Verify the alias output appears in the pane
    def check_alias_output():
        capture_result = env.tmux(["capture-pane", "-p", "-t", window_name])
        return alias_output in capture_result.stdout

    assert poll_until(check_alias_output, timeout=2.0), (
        f"Alias output '{alias_output}' not found in pane - shell rc file not sourced"
    )


def test_project_config_overrides_global_config(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Project-level settings should override conflicting global settings."""
    env = isolated_tmux_server
    branch_name = "feature-project-overrides"
    global_prefix = "global-"
    project_prefix = "project-"

    write_global_workmux_config(env, window_prefix=global_prefix)
    write_workmux_config(repo_path, window_prefix=project_prefix)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    list_windows_result = env.tmux(["list-windows", "-F", "#{window_name}"])
    existing_windows = list_windows_result.stdout.strip().split("\n")
    assert f"{project_prefix}{branch_name}" in existing_windows
    assert f"{global_prefix}{branch_name}" not in existing_windows


def test_global_config_used_when_project_config_absent(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Global config should be respected even if the repository lacks .workmux.yaml."""
    env = isolated_tmux_server
    branch_name = "feature-global-only"
    hook_file = "global_only_hook.txt"

    write_global_workmux_config(env, post_create=[f"touch {hook_file}"])

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    expected_worktree_dir = get_worktree_path(repo_path, branch_name)
    assert (expected_worktree_dir / hook_file).exists()


def test_global_placeholder_merges_post_create_commands(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """The '<global>' placeholder should expand to global post_create commands."""
    env = isolated_tmux_server
    branch_name = "feature-global-hooks"
    global_hook = "created_from_global.txt"
    before_hook = "project_before.txt"
    after_hook = "project_after.txt"

    write_global_workmux_config(env, post_create=[f"touch {global_hook}"])
    write_workmux_config(
        repo_path,
        post_create=[f"touch {before_hook}", "<global>", f"touch {after_hook}"],
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_dir = get_worktree_path(repo_path, branch_name)
    assert (worktree_dir / before_hook).exists()
    assert (worktree_dir / global_hook).exists()
    assert (worktree_dir / after_hook).exists()


def test_global_placeholder_merges_file_operations(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """The '<global>' placeholder should merge copy and symlink file operations."""
    env = isolated_tmux_server
    branch_name = "feature-global-files"

    # Create files/directories that will be copied or symlinked.
    global_copy = repo_path / "global.env"
    project_copy = repo_path / "project.env"
    global_copy.write_text("GLOBAL")
    project_copy.write_text("PROJECT")

    global_dir = repo_path / "global_cache"
    project_dir = repo_path / "project_cache"
    global_dir.mkdir()
    (global_dir / "shared.txt").write_text("global data")
    project_dir.mkdir()
    (project_dir / "local.txt").write_text("project data")

    env.run_command(
        ["git", "add", "global.env", "project.env", "global_cache", "project_cache"],
        cwd=repo_path,
    )
    env.run_command(
        ["git", "commit", "-m", "Add files for global placeholder tests"], cwd=repo_path
    )

    write_global_workmux_config(
        env,
        files={"copy": ["global.env"], "symlink": ["global_cache"]},
    )
    write_workmux_config(
        repo_path,
        files={
            "copy": ["<global>", "project.env"],
            "symlink": ["<global>", "project_cache"],
        },
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_dir = get_worktree_path(repo_path, branch_name)
    copied_global = worktree_dir / "global.env"
    copied_project = worktree_dir / "project.env"
    assert copied_global.exists()
    assert copied_global.read_text() == "GLOBAL"
    assert not copied_global.is_symlink()
    assert copied_project.exists()
    assert copied_project.read_text() == "PROJECT"
    assert not copied_project.is_symlink()

    symlinked_global = worktree_dir / "global_cache"
    symlinked_project = worktree_dir / "project_cache"
    assert symlinked_global.is_symlink()
    assert (symlinked_global / "shared.txt").read_text() == "global data"
    assert symlinked_project.is_symlink()
    assert (symlinked_project / "local.txt").read_text() == "project data"


def test_global_placeholder_only_merges_specific_file_lists(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """`<global>` can merge copy patterns while symlink patterns fully override."""
    env = isolated_tmux_server
    branch_name = "feature-partial-file-merge"

    ignored_files = [
        "global_copy.txt",
        "project_copy.txt",
        "global_symlink_dir/",
        "project_symlink_dir/",
    ]
    with (repo_path / ".gitignore").open("a") as gitignore:
        for entry in ignored_files:
            gitignore.write(f"{entry}\n")

    (repo_path / "global_copy.txt").write_text("global copy")
    (repo_path / "project_copy.txt").write_text("project copy")
    global_symlink_dir = repo_path / "global_symlink_dir"
    global_symlink_dir.mkdir()
    (global_symlink_dir / "global.txt").write_text("global data")
    project_symlink_dir = repo_path / "project_symlink_dir"
    project_symlink_dir.mkdir()
    (project_symlink_dir / "project.txt").write_text("project data")

    write_global_workmux_config(
        env,
        files={"copy": ["global_copy.txt"], "symlink": ["global_symlink_dir"]},
    )
    write_workmux_config(
        repo_path,
        files={
            "copy": ["<global>", "project_copy.txt"],
            "symlink": ["project_symlink_dir"],
        },
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_dir = get_worktree_path(repo_path, branch_name)
    assert (worktree_dir / "global_copy.txt").exists()
    assert (worktree_dir / "project_copy.txt").exists()

    assert (worktree_dir / "project_symlink_dir").is_symlink()
    assert not (worktree_dir / "global_symlink_dir").exists()


def test_project_empty_file_lists_override_global_lists(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Explicit empty lists suppress the corresponding global file operations."""
    env = isolated_tmux_server
    branch_name = "feature-empty-file-override"

    ignored_files = [
        "global_only.env",
        "global_shared_dir/",
    ]
    with (repo_path / ".gitignore").open("a") as gitignore:
        for entry in ignored_files:
            gitignore.write(f"{entry}\n")

    (repo_path / "global_only.env").write_text("SECRET=1")
    global_shared_dir = repo_path / "global_shared_dir"
    global_shared_dir.mkdir()
    (global_shared_dir / "package.json").write_text('{"name":"demo"}')

    write_global_workmux_config(
        env,
        files={"copy": ["global_only.env"], "symlink": ["global_shared_dir"]},
    )
    write_workmux_config(
        repo_path,
        files={"copy": [], "symlink": []},
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_dir = get_worktree_path(repo_path, branch_name)
    assert not (worktree_dir / "global_only.env").exists()
    assert not (worktree_dir / "global_shared_dir").exists()


def test_project_panes_replace_global_panes(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Project panes should completely replace global panes (no merging)."""
    env = isolated_tmux_server
    branch_name = "feature-pane-override"
    window_name = get_window_name(branch_name)
    global_output = "GLOBAL_PANE_OUTPUT"
    project_output = "PROJECT_PANE_OUTPUT"

    write_global_workmux_config(
        env, panes=[{"command": f"echo '{global_output}'; sleep 0.5"}]
    )
    write_workmux_config(
        repo_path, panes=[{"command": f"echo '{project_output}'; sleep 0.5"}]
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    def check_project_output():
        capture_result = env.tmux(["capture-pane", "-p", "-t", window_name])
        return project_output in capture_result.stdout

    assert poll_until(check_project_output, timeout=2.0), (
        f"Expected project pane output '{project_output}' not found"
    )

    capture_result = env.tmux(["capture-pane", "-p", "-t", window_name])
    assert global_output not in capture_result.stdout


def test_add_from_specific_branch(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add --base` creates a worktree from a specific branch."""
    env = isolated_tmux_server
    new_branch = "feature-from-base"

    write_workmux_config(repo_path)

    # Create a commit on the current branch
    create_commit(env, repo_path, "Add base file")

    # Get current branch name
    result = env.run_command(["git", "branch", "--show-current"], cwd=repo_path)
    base_branch = result.stdout.strip()

    # Run workmux add with --base flag
    run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"add {new_branch} --base {base_branch}",
    )

    # Verify worktree was created
    expected_worktree_dir = get_worktree_path(repo_path, new_branch)
    assert expected_worktree_dir.is_dir()

    # Verify the new branch contains the file from base branch
    # The create_commit helper creates a file with a specific naming pattern
    expected_file = expected_worktree_dir / "file_for_Add_base_file.txt"
    assert expected_file.exists()

    # Verify tmux window was created
    window_name = get_window_name(new_branch)
    list_windows_result = env.tmux(["list-windows", "-F", "#{window_name}"])
    existing_windows = list_windows_result.stdout.strip().split("\n")
    assert window_name in existing_windows


def test_add_from_current_branch_flag(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """`workmux add --from-current` should base new branches on the active branch."""
    env = isolated_tmux_server
    base_branch = "feature-stack-base"
    stacked_branch = "feature-stack-child"
    commit_message = "Stack base change"

    write_workmux_config(repo_path)

    # Start a new branch and add a commit that the stacked branch should inherit.
    env.run_command(["git", "checkout", "-b", base_branch], cwd=repo_path)
    create_commit(env, repo_path, commit_message)

    run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"add {stacked_branch} --from-current",
    )

    stacked_worktree = get_worktree_path(repo_path, stacked_branch)
    expected_file = (
        stacked_worktree
        / f"file_for_{commit_message.replace(' ', '_').replace(':', '')}.txt"
    )
    assert expected_file.exists()

    window_name = get_window_name(stacked_branch)
    list_windows_result = env.tmux(["list-windows", "-F", "#{window_name}"])
    existing_windows = list_windows_result.stdout.strip().split("\n")
    assert window_name in existing_windows


def test_add_reuses_existing_branch(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` reuses an existing branch instead of creating a new one."""
    env = isolated_tmux_server
    branch_name = "feature-existing-branch"
    commit_message = "Existing branch changes"

    write_workmux_config(repo_path)

    # Remember the default branch so we can switch back after preparing the feature branch
    current_branch_result = env.run_command(
        ["git", "branch", "--show-current"], cwd=repo_path
    )
    default_branch = current_branch_result.stdout.strip()

    # Create and populate an existing branch
    env.run_command(["git", "checkout", "-b", branch_name], cwd=repo_path)
    create_commit(env, repo_path, commit_message)
    branch_head = env.run_command(
        ["git", "rev-parse", "HEAD"], cwd=repo_path
    ).stdout.strip()

    # Switch back to the default branch so workmux add runs from a typical state
    env.run_command(["git", "checkout", default_branch], cwd=repo_path)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    worktree_path = get_worktree_path(repo_path, branch_name)
    expected_file = (
        worktree_path
        / f"file_for_{commit_message.replace(' ', '_').replace(':', '')}.txt"
    )
    assert expected_file.exists()
    assert expected_file.read_text() == f"content for {commit_message}"

    # The branch should still point to the commit we created earlier
    branch_tip = env.run_command(
        ["git", "rev-parse", branch_name], cwd=repo_path
    ).stdout.strip()
    assert branch_tip == branch_head


def test_add_from_remote_branch(
    isolated_tmux_server: TmuxEnvironment,
    workmux_exe_path: Path,
    repo_path: Path,
    remote_repo_path: Path,
):
    """When the branch exists only on the remote, workmux add should fetch and track it."""
    env = isolated_tmux_server
    remote_branch_path = "feature/remote-pr"
    remote_ref = f"origin/{remote_branch_path}"
    commit_message = "Remote PR work"

    write_workmux_config(repo_path)

    # Wire up the repo to a bare remote and push the default branch.
    env.run_command(
        ["git", "remote", "add", "origin", str(remote_repo_path)], cwd=repo_path
    )
    env.run_command(["git", "push", "-u", "origin", "main"], cwd=repo_path)

    # Create a branch with commits and push it to the remote.
    env.run_command(["git", "checkout", "-b", remote_branch_path], cwd=repo_path)
    create_commit(env, repo_path, commit_message)
    remote_tip = env.run_command(
        ["git", "rev-parse", remote_branch_path], cwd=repo_path
    ).stdout.strip()
    env.run_command(["git", "push", "-u", "origin", remote_branch_path], cwd=repo_path)

    # Remove the local branch and remote-tracking ref so the branch only exists on the remote.
    env.run_command(["git", "checkout", "main"], cwd=repo_path)
    env.run_command(["git", "branch", "-D", remote_branch_path], cwd=repo_path)
    env.run_command(
        ["git", "update-ref", "-d", f"refs/remotes/{remote_ref}"],
        cwd=repo_path,
    )

    run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"add {remote_ref}",
    )

    worktree_path = get_worktree_path(repo_path, remote_branch_path)
    expected_file = (
        worktree_path
        / f"file_for_{commit_message.replace(' ', '_').replace(':', '')}.txt"
    )
    assert expected_file.exists()
    assert expected_file.read_text() == f"content for {commit_message}"

    # Local branch should point to the remote commit and track origin/<branch_name>.
    branch_tip = env.run_command(
        ["git", "rev-parse", remote_branch_path], cwd=repo_path
    ).stdout.strip()
    assert branch_tip == remote_tip

    upstream_tip = env.run_command(
        ["git", "rev-parse", f"{remote_branch_path}@{{upstream}}"], cwd=repo_path
    ).stdout.strip()
    assert upstream_tip == remote_tip

    origin_tip = env.run_command(
        ["git", "rev-parse", remote_ref], cwd=repo_path
    ).stdout.strip()
    assert origin_tip == remote_tip


def test_add_fails_when_worktree_exists(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` fails with a clear message if the worktree already exists."""
    env = isolated_tmux_server
    branch_name = "feature-existing-worktree"
    existing_worktree_path = repo_path.parent / "existing_worktree_dir"

    write_workmux_config(repo_path)

    # Create the branch and then return to the default branch
    env.run_command(["git", "checkout", "-b", branch_name], cwd=repo_path)
    env.run_command(["git", "checkout", "main"], cwd=repo_path)

    # Manually create a git worktree for the branch to simulate the pre-existing state
    env.run_command(
        ["git", "worktree", "add", str(existing_worktree_path), branch_name],
        cwd=repo_path,
    )

    with pytest.raises(AssertionError) as excinfo:
        run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    stderr = str(excinfo.value)
    assert f"A worktree for branch '{branch_name}' already exists." in stderr
    assert "Use 'workmux open" in stderr


def test_add_copies_single_file(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` copies a single file to the worktree."""
    env = isolated_tmux_server
    branch_name = "feature-copy-file"

    # Create a file in the repo root to copy
    env_file = repo_path / ".env"
    env_file.write_text("SECRET_KEY=test123")

    # Commit the file to avoid uncommitted changes
    env.run_command(["git", "add", ".env"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add .env file"], cwd=repo_path)

    # Configure workmux to copy the .env file
    write_workmux_config(repo_path, files={"copy": [".env"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify the file was copied (not symlinked)
    worktree_path = get_worktree_path(repo_path, branch_name)
    copied_file = worktree_path / ".env"
    assert copied_file.exists()
    assert copied_file.read_text() == "SECRET_KEY=test123"
    # Verify it's a real file, not a symlink
    assert not copied_file.is_symlink()


def test_add_copies_multiple_files_with_glob(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` copies multiple files using glob patterns."""
    env = isolated_tmux_server
    branch_name = "feature-copy-glob"

    # Create multiple .local files in the repo root
    (repo_path / ".env.local").write_text("LOCAL_VAR=value1")
    (repo_path / ".secrets.local").write_text("API_KEY=secret")

    # Commit the files
    env.run_command(["git", "add", "*.local"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add local files"], cwd=repo_path)

    # Configure workmux to copy all .local files
    write_workmux_config(repo_path, files={"copy": ["*.local"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify both files were copied
    worktree_path = get_worktree_path(repo_path, branch_name)
    assert (worktree_path / ".env.local").exists()
    assert (worktree_path / ".env.local").read_text() == "LOCAL_VAR=value1"
    assert (worktree_path / ".secrets.local").exists()
    assert (worktree_path / ".secrets.local").read_text() == "API_KEY=secret"


def test_add_copies_file_with_parent_directories(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` creates parent directories when copying nested files."""
    env = isolated_tmux_server
    branch_name = "feature-copy-nested"

    # Create a nested file structure
    config_dir = repo_path / "config"
    config_dir.mkdir()
    (config_dir / "app.conf").write_text("setting=value")

    # Commit the files
    env.run_command(["git", "add", "config/"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add config files"], cwd=repo_path)

    # Configure workmux to copy the nested file
    write_workmux_config(repo_path, files={"copy": ["config/app.conf"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify the file and parent directory were created
    worktree_path = get_worktree_path(repo_path, branch_name)
    nested_file = worktree_path / "config" / "app.conf"
    assert nested_file.exists()
    assert nested_file.read_text() == "setting=value"
    assert (worktree_path / "config").is_dir()


def test_add_symlinks_single_file(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` creates a symlink for a single file."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-file"

    # Create a file in the repo root to symlink
    shared_file = repo_path / "shared.txt"
    shared_file.write_text("shared content")

    # Commit the file
    env.run_command(["git", "add", "shared.txt"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add shared file"], cwd=repo_path)

    # Configure workmux to symlink the file
    write_workmux_config(repo_path, files={"symlink": ["shared.txt"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify the symlink was created
    worktree_path = get_worktree_path(repo_path, branch_name)
    symlinked_file = worktree_path / "shared.txt"
    assert symlinked_file.exists()
    assert symlinked_file.is_symlink()
    # Verify the content is accessible through the symlink
    assert symlinked_file.read_text() == "shared content"


def test_add_symlinks_directory(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` creates a symlink for a directory."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-dir"

    # Create a directory in the repo root to symlink
    node_modules = repo_path / "node_modules"
    node_modules.mkdir()
    (node_modules / "package.json").write_text('{"name": "test"}')

    # Commit the directory
    env.run_command(["git", "add", "node_modules/"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add node_modules"], cwd=repo_path)

    # Configure workmux to symlink the directory
    write_workmux_config(repo_path, files={"symlink": ["node_modules"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify the symlink was created
    worktree_path = get_worktree_path(repo_path, branch_name)
    symlinked_dir = worktree_path / "node_modules"
    assert symlinked_dir.exists()
    assert symlinked_dir.is_symlink()
    # Verify the directory contents are accessible
    assert (symlinked_dir / "package.json").exists()


def test_add_symlinks_multiple_items_with_glob(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` creates symlinks for multiple items using glob patterns."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-glob"

    # Create multiple cache directories
    (repo_path / ".cache").mkdir()
    (repo_path / ".cache" / "data.txt").write_text("cache data")
    (repo_path / ".pnpm-store").mkdir()
    (repo_path / ".pnpm-store" / "index.txt").write_text("pnpm index")

    # Commit the directories
    env.run_command(["git", "add", ".cache/", ".pnpm-store/"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add cache dirs"], cwd=repo_path)

    # Configure workmux to symlink using glob patterns
    write_workmux_config(repo_path, files={"symlink": [".*-store", ".cache"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify both symlinks were created
    worktree_path = get_worktree_path(repo_path, branch_name)
    assert (worktree_path / ".cache").is_symlink()
    assert (worktree_path / ".cache" / "data.txt").exists()
    assert (worktree_path / ".pnpm-store").is_symlink()
    assert (worktree_path / ".pnpm-store" / "index.txt").exists()


def test_add_symlinks_are_relative(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that created symlinks use relative paths, not absolute paths."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-relative"

    # Create a file to symlink
    test_file = repo_path / "test.txt"
    test_file.write_text("test content")

    # Commit the file
    env.run_command(["git", "add", "test.txt"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add test file"], cwd=repo_path)

    # Configure workmux to symlink the file
    write_workmux_config(repo_path, files={"symlink": ["test.txt"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify the symlink uses the correct relative path
    worktree_path = get_worktree_path(repo_path, branch_name)
    symlinked_file = worktree_path / "test.txt"
    assert symlinked_file.is_symlink()

    # Verify the symlink points to the correct relative path
    source_file = repo_path / "test.txt"
    expected_target = os.path.relpath(source_file, symlinked_file.parent)
    link_target = os.readlink(symlinked_file)
    assert link_target == expected_target, (
        f"Symlink target incorrect. Expected: {expected_target}, Got: {link_target}"
    )


def test_add_symlink_replaces_existing_file(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that symlinking replaces an existing file at the destination."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-replace"

    # Create a file to symlink
    source_file = repo_path / "source.txt"
    source_file.write_text("source content")

    # Commit the file
    env.run_command(["git", "add", "source.txt"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add source file"], cwd=repo_path)

    # Configure workmux to symlink the file
    write_workmux_config(repo_path, files={"symlink": ["source.txt"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Now manually create a regular file at the symlink location in the worktree
    worktree_path = get_worktree_path(repo_path, branch_name)
    dest_file = worktree_path / "source.txt"

    # Remove the existing symlink and create a regular file
    dest_file.unlink()
    dest_file.write_text("replaced content")
    assert not dest_file.is_symlink()

    # Run workmux add again on a different branch to trigger symlink creation again
    # This simulates the --force-files behavior
    branch_name_2 = "feature-symlink-replace-2"
    run_workmux_add(env, workmux_exe_path, repo_path, branch_name_2)

    # Verify the file was replaced with a symlink
    worktree_path_2 = get_worktree_path(repo_path, branch_name_2)
    dest_file_2 = worktree_path_2 / "source.txt"
    assert dest_file_2.is_symlink()
    assert dest_file_2.read_text() == "source content"


def test_add_symlink_with_nested_structure(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that symlinking works with nested directory structures."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-nested"

    # Create a nested directory structure
    nested_dir = repo_path / "lib" / "cache"
    nested_dir.mkdir(parents=True)
    (nested_dir / "data.db").write_text("database content")

    # Commit the structure
    env.run_command(["git", "add", "lib/"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add nested structure"], cwd=repo_path)

    # Configure workmux to symlink the nested directory
    write_workmux_config(repo_path, files={"symlink": ["lib/cache"]}, env=env)

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify the nested symlink was created
    worktree_path = get_worktree_path(repo_path, branch_name)
    symlinked_dir = worktree_path / "lib" / "cache"
    assert symlinked_dir.exists()
    assert symlinked_dir.is_symlink()
    assert (symlinked_dir / "data.db").read_text() == "database content"
    # Verify parent directory exists and is NOT a symlink
    assert (worktree_path / "lib").is_dir()
    assert not (worktree_path / "lib").is_symlink()


def test_add_combines_copy_and_symlink(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that copy and symlink operations can be used together."""
    env = isolated_tmux_server
    branch_name = "feature-combined-ops"

    # Create files for both copy and symlink
    (repo_path / ".env").write_text("SECRET=abc123")
    node_modules = repo_path / "node_modules"
    node_modules.mkdir()
    (node_modules / "package.json").write_text('{"name": "test"}')

    # Commit both
    env.run_command(["git", "add", ".env", "node_modules/"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add files"], cwd=repo_path)

    # Configure workmux to both copy and symlink
    write_workmux_config(
        repo_path, files={"copy": [".env"], "symlink": ["node_modules"]}, env=env
    )

    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify copy operation
    worktree_path = get_worktree_path(repo_path, branch_name)
    copied_file = worktree_path / ".env"
    assert copied_file.exists()
    assert not copied_file.is_symlink()
    assert copied_file.read_text() == "SECRET=abc123"

    # Verify symlink operation
    symlinked_dir = worktree_path / "node_modules"
    assert symlinked_dir.exists()
    assert symlinked_dir.is_symlink()
    assert (symlinked_dir / "package.json").exists()


def test_add_file_operations_with_empty_config(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that workmux add works when files config is empty or missing."""
    env = isolated_tmux_server
    branch_name = "feature-no-files"

    # Configure workmux with no file operations
    write_workmux_config(repo_path, env=env)

    # Should succeed without errors
    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify worktree was created
    worktree_path = get_worktree_path(repo_path, branch_name)
    assert worktree_path.is_dir()


def test_add_file_operations_with_nonexistent_pattern(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that workmux handles glob patterns that match no files gracefully."""
    env = isolated_tmux_server
    branch_name = "feature-no-match"

    # Configure workmux with patterns that don't match anything
    write_workmux_config(
        repo_path,
        files={"copy": ["nonexistent-*.txt"], "symlink": ["missing-dir"]},
        env=env,
    )

    # Should succeed without errors (no matches is not an error)
    run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # Verify worktree was created
    worktree_path = get_worktree_path(repo_path, branch_name)
    assert worktree_path.is_dir()


def test_add_copy_with_path_traversal_fails(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` fails if a copy path attempts to traverse outside the repo."""
    env = isolated_tmux_server
    branch_name = "feature-copy-traversal"

    # Create a sensitive file outside the repository
    (repo_path.parent / "sensitive_file").write_text("secret")

    write_workmux_config(repo_path, files={"copy": ["../sensitive_file"]}, env=env)

    with pytest.raises(AssertionError) as excinfo:
        run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # The error should indicate path traversal or invalid path
    stderr = str(excinfo.value)
    assert (
        "Path traversal" in stderr
        or "outside" in stderr
        or "No such file" in stderr
        or "pattern matched nothing" in stderr
    )


def test_add_symlink_with_path_traversal_fails(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies that `workmux add` fails if a symlink path attempts to traverse outside the repo."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-traversal"

    # Create a directory outside the repository that will be matched by the glob
    (repo_path.parent / "some_dir").mkdir()
    (repo_path.parent / "some_dir" / "file.txt").write_text("outside repo")

    write_workmux_config(repo_path, files={"symlink": ["../some_dir"]}, env=env)

    with pytest.raises(AssertionError) as excinfo:
        run_workmux_add(env, workmux_exe_path, repo_path, branch_name)

    # The error should indicate path traversal
    stderr = str(excinfo.value)
    assert "Path traversal" in stderr or "outside" in stderr


def test_add_symlink_overwrites_conflicting_file_from_git(
    isolated_tmux_server: TmuxEnvironment, workmux_exe_path: Path, repo_path: Path
):
    """Verifies a symlink operation overwrites a conflicting file checked out by git."""
    env = isolated_tmux_server
    branch_name = "feature-symlink-overwrite"

    # In main repo root, create the directory to be symlinked
    (repo_path / "node_modules").mkdir()
    (repo_path / "node_modules" / "dep.js").write_text("content")
    env.run_command(["git", "add", "node_modules/"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add real node_modules"], cwd=repo_path)

    # On a different branch, create a conflicting FILE with the same name
    env.run_command(["git", "checkout", "-b", "conflict-branch"], cwd=repo_path)
    env.run_command(["git", "rm", "-r", "node_modules"], cwd=repo_path)
    (repo_path / "node_modules").write_text("this is a placeholder file")
    env.run_command(["git", "add", "node_modules"], cwd=repo_path)
    env.run_command(["git", "commit", "-m", "Add conflicting file"], cwd=repo_path)

    # On main, configure workmux to symlink the directory
    env.run_command(["git", "checkout", "main"], cwd=repo_path)
    write_workmux_config(repo_path, files={"symlink": ["node_modules"]}, env=env)

    # Create a worktree from the branch with the conflicting file
    run_workmux_command(
        env,
        workmux_exe_path,
        repo_path,
        f"add {branch_name} --base conflict-branch",
    )

    # Verify the symlink exists and replaced the original file
    worktree_path = get_worktree_path(repo_path, branch_name)
    symlinked_target = worktree_path / "node_modules"
    assert symlinked_target.is_symlink()
    assert (symlinked_target / "dep.js").exists()
