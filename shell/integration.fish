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

function __hiruzen_update_cwd --on-event fish_prompt
	printf '\e]7391;CWD;%s\a' (__hiruzen_escape "$PWD")
end

function fish_prompt
	set --local __hiruzen_lines (__hiruzen_user_prompt)
	printf '\e]7391;PS\a'
	# Passed as data so a literal % in the prompt is not a printf conversion.
	printf '%s' (string join \n $__hiruzen_lines)
	printf '\e]7391;PE\a'
end
