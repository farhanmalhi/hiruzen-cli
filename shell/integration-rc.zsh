# hiruzen: loaded by zsh as .zshrc from hiruzen's private ZDOTDIR.
# Runs the user's own .zshrc, then marks where each prompt ends so hiruzen
# can tell which text on screen is the command being typed.
autoload -U add-zsh-hook

HIRUZEN_ZDOTDIR=$ZDOTDIR

# /etc/zshrc on macOS points HISTFILE into ZDOTDIR, which is hiruzen's
# temporary directory at this point.
if [[ $HISTFILE == "$ZDOTDIR/.zsh_history" ]]; then
	HISTFILE=$HIRUZEN_USER_ZDOTDIR/.zsh_history
fi

ZDOTDIR=$HIRUZEN_USER_ZDOTDIR
if [[ -f $ZDOTDIR/.zshrc ]]; then
	. $ZDOTDIR/.zshrc
fi
# A login shell still has .zlogin to read from hiruzen's directory.
if [[ -o login ]]; then
	HIRUZEN_USER_ZDOTDIR=$ZDOTDIR
	ZDOTDIR=$HIRUZEN_ZDOTDIR
fi

__hiruzen_prompt_start() {
	builtin printf '\e]7391;PS\a' > /dev/tty
}

__hiruzen_prompt_end() {
	builtin printf '\e]7391;PE\a' > /dev/tty
}

__hiruzen_escape() {
	builtin emulate -L zsh
	# Byte by byte, so multi-byte characters pass through untouched.
	builtin local LC_ALL=C str="$1" i byte out=''
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
	builtin print -r -- "$out"
}

__hiruzen_precmd() {
	builtin printf '\e]7391;CWD;%s\a' "$(__hiruzen_escape "${PWD}")" > /dev/tty
}

# The markers are sent when the line editor starts, after the prompt has
# been drawn, so they work with any prompt theme.
if (( ${+widgets[zle-line-init]} )); then
	zle -A zle-line-init __hiruzen_orig_zle_line_init
	__hiruzen_zle_line_init() {
		__hiruzen_prompt_start
		zle __hiruzen_orig_zle_line_init
		__hiruzen_prompt_end
	}
else
	__hiruzen_zle_line_init() {
		__hiruzen_prompt_start
		__hiruzen_prompt_end
	}
fi
zle -N zle-line-init __hiruzen_zle_line_init

add-zsh-hook precmd __hiruzen_precmd
