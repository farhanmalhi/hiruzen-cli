# hiruzen: sourced by fish at startup. Wraps the prompt in markers so
# hiruzen can tell which text on screen is the command being typed.
functions --copy fish_prompt __hiruzen_user_prompt

function __hiruzen_escape
	echo $argv \
	| string replace -a '\\' '\\\\' \
	| string replace -a ';' '\\x3b' \
	| string replace -a \e '\\x1b' \
	| string replace -a \a '\\x07' \
	| string split \n | string join '\x0a'
end

# Tell hiruzen the functions this shell defines (fish aliases are functions).
if not set -q HIRUZEN_REMOTE
	printf '\e]7391;NAMES;%s\a' (functions -n | string replace -a ',' ' ' | string join ' ')
end

function __hiruzen_exit_status --on-event fish_postexec
	printf '\e]7391;EXIT;%s\a' $status
end

function __hiruzen_update_cwd --on-event fish_prompt
	set -q HIRUZEN_REMOTE; and set -l __hiruzen_where R
	printf '\e]7391;%sCWD;%s\a' "$__hiruzen_where" (__hiruzen_escape "$PWD")
end

function fish_prompt
	set --local __hiruzen_lines (__hiruzen_user_prompt)
	printf '\e]7391;PS\a'
	# Passed as data so a literal % in the prompt is not a printf conversion.
	printf '%s' (string join \n $__hiruzen_lines)
	printf '\e]7391;PE\a'
end

# Commands named in the terminal.directCommands setting run straight on the
# real terminal, outside hiruzen, so tools that watch the terminal's
# foreground process see them. A function of your own with the same name is
# left alone.
if set -q HIRUZEN_DIRECT_SOCKET; and not set -q HIRUZEN_REMOTE
	for __hiruzen_name in (string split -n ' ' -- "$HIRUZEN_DIRECT")
		functions -q -- $__hiruzen_name; and continue
		function $__hiruzen_name --inherit-variable __hiruzen_name
			set -l exe hiruzen
			set -q HIRUZEN_EXE; and set exe $HIRUZEN_EXE
			command $exe direct -- $__hiruzen_name $argv
		end
	end
	set -e __hiruzen_name
end
