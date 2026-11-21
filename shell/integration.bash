# hiruzen: passed to bash as --init-file. Runs the user's own startup files,
# then wraps the prompt in markers so hiruzen can tell which text on screen
# is the command being typed.
if [ -z "$HIRUZEN_LOGIN" ]; then
	if [ -r ~/.bashrc ]; then
		. ~/.bashrc
	fi
else
	if [ -r /etc/profile ]; then
		. /etc/profile
	fi
	if [ -r ~/.bash_profile ]; then
		. ~/.bash_profile
	elif [ -r ~/.bash_login ]; then
		. ~/.bash_login
	elif [ -r ~/.profile ]; then
		. ~/.profile
	fi
fi

__hiruzen_escape() {
	builtin local LC_ALL=C str="${1}" i byte out=''
	for (( i = 0; i < ${#str}; ++i )); do
		byte="${str:$i:1}"
		case "$byte" in
			'\') out+='\\' ;;
			';') out+='\x3b' ;;
			$'\n') out+='\x0a' ;;
			$'\e') out+='\x1b' ;;
			$'\a') out+='\x07' ;;
			*) out+="$byte" ;;
		esac
	done
	builtin printf '%s\n' "${out}"
}

__hiruzen_precmd() {
	builtin printf '\e]7391;%sCWD;%s\a' "${HIRUZEN_REMOTE:+R}" "$(__hiruzen_escape "$PWD")"
	# Re-wrap whenever something (a theme, the user) has replaced PS1.
	if [[ "$__hiruzen_wrapped_ps1" == "" || "$__hiruzen_wrapped_ps1" != "$PS1" ]]; then
		__hiruzen_wrapped_ps1='\[\e]7391;PS\a\]'"$PS1"'\[\e]7391;PE\a\]'
		PS1="$__hiruzen_wrapped_ps1"
	fi
}

# Run after the user's PROMPT_COMMAND, which may be a string or (bash 5.1+) an array.
if [[ "$(builtin declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
	PROMPT_COMMAND+=(__hiruzen_precmd)
elif [[ -n "${PROMPT_COMMAND:-}" ]]; then
	__hiruzen_user_prompt_command=$PROMPT_COMMAND
	__hiruzen_prompt_command() {
		builtin local __hiruzen_status=$?
		builtin eval "$(builtin printf '(exit %d)' "$__hiruzen_status"); $__hiruzen_user_prompt_command"
		__hiruzen_precmd
	}
	PROMPT_COMMAND=__hiruzen_prompt_command
else
	PROMPT_COMMAND=__hiruzen_precmd
fi
