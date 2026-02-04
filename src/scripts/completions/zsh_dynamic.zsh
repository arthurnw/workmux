# Dynamic worktree handle completion (directory names)
# Used for open/remove/merge/path/close - these accept handles or branch names
_workmux_handles() {
    local handles
    handles=("${(@f)$(workmux _complete-handles 2>/dev/null)}")
    compadd -a handles
}

# Dynamic git branch completion for add command
_workmux_git_branches() {
    local branches
    branches=("${(@f)$(workmux _complete-git-branches 2>/dev/null)}")
    compadd -a branches
}

# Override completion for commands that need dynamic completion
_workmux_dynamic() {
    # Ensure standard zsh array indexing (1-based) regardless of user settings
    emulate -L zsh
    setopt extended_glob  # Required for _files glob qualifiers like *(-/)
    setopt no_nomatch     # Allow failed globs to resolve to empty list

    # Get the subcommand (second word)
    local cmd="${words[2]}"

    # List of flags that take arguments (values), by command.
    # We must defer to _workmux for these so it can offer files/custom hints.
    # Boolean flags are excluded so we can offer positional completions after them.
    local -a arg_flags
    case "$cmd" in
        add)
            arg_flags=(
                -p --prompt
                -P --prompt-file
                --name
                -a --agent
                -n --count
                --foreach
                --branch-template
                --pr
                # Note: --base is excluded because it needs dynamic completion
            )
            ;;
        open)
            arg_flags=(
                -p --prompt
                -P --prompt-file
                # Note: -n/--new is a boolean flag, not included here
            )
            ;;
        merge)
            arg_flags=(
                # Note: --into is excluded because it needs dynamic completion
            )
            ;;
        *)
            arg_flags=()
            ;;
    esac

    # Check if we are currently completing a flag (starts with -)
    # OR if the previous word is a flag that requires an argument.
    if [[ "${words[CURRENT]}" == -* ]] || [[ -n "${arg_flags[(r)${words[CURRENT-1]}]}" ]]; then
        _workmux "$@"
        return
    fi

    # Count how many positional arguments have already been provided
    # Start from position 3 (after command and subcommand)
    local positional_count=0
    local i=3
    local skip_next=false
    while [[ $i -lt $CURRENT ]]; do
        local word="${words[$i]}"

        if [[ "$skip_next" == "true" ]]; then
            skip_next=false
        elif [[ "$word" == -* ]]; then
            # This is a flag - check if it takes a value
            if [[ -n "${arg_flags[(r)$word]}" ]]; then
                skip_next=true
            fi
        else
            # This is a positional argument
            ((positional_count++))
        fi
        ((i++))
    done

    # Only handle commands that need dynamic completion
    case "$cmd" in
        open|remove|rm|path|merge|close)
            # These commands take exactly one positional argument (the handle/branch name)
            # Only offer completions if we haven't provided it yet
            if [[ $positional_count -eq 0 ]]; then
                _workmux_handles
            else
                # Already have the positional arg - offer no completions
                return 0
            fi
            ;;
        add)
            # Add command takes one positional argument (the branch name)
            # Only offer completions if we haven't provided it yet
            if [[ $positional_count -eq 0 ]]; then
                _workmux_git_branches
            else
                # Already have the positional arg - offer no completions
                return 0
            fi
            ;;
        *)
            # For all other commands, strictly use generated completions
            _workmux "$@"
            ;;
    esac
}

compdef _workmux_dynamic workmux
