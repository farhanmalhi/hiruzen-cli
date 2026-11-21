# hiruzen: loaded by login zsh as .zprofile; runs the user's own.
if [[ -f $HIRUZEN_USER_ZDOTDIR/.zprofile ]]; then
	HIRUZEN_ZDOTDIR=$ZDOTDIR
	ZDOTDIR=$HIRUZEN_USER_ZDOTDIR
	. $HIRUZEN_USER_ZDOTDIR/.zprofile
	HIRUZEN_USER_ZDOTDIR=$ZDOTDIR
	ZDOTDIR=$HIRUZEN_ZDOTDIR
fi
