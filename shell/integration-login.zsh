# hiruzen: loaded by login zsh as .zlogin, the last startup file. Runs the
# user's own and leaves ZDOTDIR as the user had it.
ZDOTDIR=$HIRUZEN_USER_ZDOTDIR
if [[ -f $ZDOTDIR/.zlogin ]]; then
	. $ZDOTDIR/.zlogin
fi
