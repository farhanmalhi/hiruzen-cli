# hiruzen: loaded by zsh as .zshenv from hiruzen's private ZDOTDIR.
# Runs the user's own .zshenv, then keeps ZDOTDIR pointed at hiruzen's
# directory so the remaining startup files are also read from there.
if [[ -f $HIRUZEN_USER_ZDOTDIR/.zshenv ]]; then
	HIRUZEN_ZDOTDIR=$ZDOTDIR
	ZDOTDIR=$HIRUZEN_USER_ZDOTDIR
	if [[ $HIRUZEN_USER_ZDOTDIR != $HIRUZEN_ZDOTDIR ]]; then
		. $HIRUZEN_USER_ZDOTDIR/.zshenv
	fi
	HIRUZEN_USER_ZDOTDIR=$ZDOTDIR
	ZDOTDIR=$HIRUZEN_ZDOTDIR
fi
