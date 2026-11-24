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
	# How the command just run ended, read before anything can change it.
	builtin local __hiruzen_status=$?
	builtin printf '\e]7391;EXIT;%s\a' "$__hiruzen_status" > /dev/tty
	builtin printf '\e]7391;%sCWD;%s\a' "${HIRUZEN_REMOTE:+R}" "$(__hiruzen_escape "${PWD}")" > /dev/tty
}

# Sets REPLY to $1 cut down to at most $2 characters by dropping its middle,
# so both ends of a long name stay readable.
__hiruzen_shorten() {
	builtin emulate -L zsh
	builtin local text=$1
	builtin local -i max=$2 head tail
	if (( max >= 3 && ${#text} > max )); then
		head=$(( max / 2 ))
		tail=$(( max - head - 1 ))
		text="${text[1,head]}…${text[-tail,-1]}"
	fi
	REPLY=$text
}

# Sets REPLY to the branch checked out in the current directory's repository
# and REPLY_KIND to "branch", or to the first seven characters of the commit
# and "commit" when no branch is checked out. Reads .git directly, so drawing
# a prompt starts no process.
__hiruzen_git_head() {
	builtin emulate -L zsh
	builtin local dir=$PWD head line
	REPLY= REPLY_KIND=
	while true; do
		if [[ -f $dir/.git/HEAD ]]; then
			head=$dir/.git/HEAD
			break
		elif [[ -f $dir/.git ]]; then
			# A worktree or submodule: .git is a file naming the real directory.
			IFS= builtin read -r line < $dir/.git 2>/dev/null || builtin return 1
			line=${line#gitdir: }
			[[ $line == /* ]] || line=$dir/$line
			head=$line/HEAD
			break
		fi
		[[ $dir == / || -z $dir ]] && builtin return 1
		dir=${dir:h}
	done
	IFS= builtin read -r line < $head 2>/dev/null || builtin return 1
	if [[ $line == 'ref: refs/heads/.invalid' ]]; then
		# A reftable repository keeps a placeholder in HEAD; ask git.
		if line=$(command git symbolic-ref --short HEAD 2>/dev/null); then
			REPLY=$line REPLY_KIND=branch
		elif line=$(command git rev-parse --short=7 HEAD 2>/dev/null); then
			REPLY=$line REPLY_KIND=commit
		fi
	elif [[ $line == 'ref: '* ]]; then
		line=${line#ref: }
		REPLY=${line#refs/heads/} REPLY_KIND=branch
	elif [[ -n $line ]]; then
		REPLY=${line[1,7]} REPLY_KIND=commit
	fi
	[[ -n $REPLY ]]
}

# The text goes through psvar, which zsh prints as it is, so a directory or
# branch named like a prompt escape or a command substitution stays text.
__hiruzen_prompt_precmd() {
	builtin emulate -L zsh
	builtin local REPLY REPLY_KIND
	__hiruzen_shorten "${(%):-%1~}" ${HIRUZEN_PROMPT_DIRECTORY:-24}
	psvar[1]=$REPLY
	psvar[2]= psvar[3]=
	if __hiruzen_git_head; then
		builtin local kind=$REPLY_KIND
		# The branch is shown whole unless the prompt would then take more
		# than three quarters of the window, leaving little room to type.
		builtin local -i room=$(( ${COLUMNS:-80} * 3 / 4 - ${#psvar[1]} - 5 ))
		if [[ -n $HIRUZEN_PROMPT_HOST ]]; then
			room=$(( room - ${#${(%):-%n@%m }} ))
		fi
		(( room < 12 )) && room=12
		(( room > ${HIRUZEN_PROMPT_BRANCH:-80} )) && room=${HIRUZEN_PROMPT_BRANCH:-80}
		__hiruzen_shorten "$REPLY" $room
		if [[ $kind == branch ]]; then
			psvar[2]=$REPLY
		else
			psvar[3]=$REPLY
		fi
	fi
}

# A prompt that zsh or macOS set is replaced by hiruzen's: colours, a short
# directory name and the git branch. A prompt you or a theme set is kept.
if [[ -n $HIRUZEN_PROMPT ]] && [[ $PS1 == '%n@%m %1~ %# ' || $PS1 == '%m%# ' ]]; then
	PS1='%B%F{blue}%1v%f%b%(2V. %F{magenta}(%2v%)%f.)%(3V. %F{yellow}(%3v%)%f.) %(?..%F{red})%#%f '
	# The user and machine names never change on your own machine, so they
	# are shown only when asked for.
	if [[ -n $HIRUZEN_PROMPT_HOST ]]; then
		PS1="%F{green}%n@%m%f $PS1"
	fi
	add-zsh-hook precmd __hiruzen_prompt_precmd
fi

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

# Tell hiruzen the aliases and functions this shell defines, so it can
# suggest them and complete an alias as the command it stands for.
if [[ -z $HIRUZEN_REMOTE ]]; then
	() {
		builtin local name
		# (@) keeps the names as separate words inside the quotes below;
		# without it they are filtered as one string.
		for name in ${(k)aliases}; do
			[[ $name == *[$' =\e\a\n']* || ${aliases[$name]} == *[$'\e\a\n']* ]] && continue
			builtin printf '\e]7391;ALIAS;%s=%s\a' "$name" "${aliases[$name]}"
		done
		builtin printf '\e]7391;NAMES;%s\a' "${(j: :)${(@k)functions:#[_.+-]*}}"
	} > /dev/tty
fi

# Commands named in the terminal.directCommands setting run straight on the
# real terminal, outside hiruzen, so tools that watch the terminal's
# foreground process see them. A function of your own with the same name is
# left alone. hiruzen has checked the names, so they need no quoting.
if [[ -n $HIRUZEN_DIRECT_SOCKET && -z $HIRUZEN_REMOTE ]]; then
	() {
		builtin local name
		for name in ${=HIRUZEN_DIRECT}; do
			(( ${+functions[$name]} )) && continue
			builtin eval "function $name { command \"\${HIRUZEN_EXE:-hiruzen}\" direct -- $name \"\$@\"; }"
		done
	}
fi
