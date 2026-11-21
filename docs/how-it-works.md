# How hiruzen works

hiruzen starts your shell inside a pseudo-terminal and passes everything
through to your real terminal. A small integration script, loaded after your
own startup files, marks where the prompt ends so hiruzen can read the command
you are typing. It draws the dropdown over the shell's output and restores
what was underneath before the shell prints again, so the shell never sees a
changed screen.

Suggestions come from three sources, ranked together:

| Source | What it gives | Example |
| --- | --- | --- |
| Command specs | The valid subcommands, options and arguments of more than 700 command-line tools | `git commit --am` suggests `--amend` |
| Live context | Values from your machine, read when needed | `git checkout ` lists your branches; `npm run ` lists the scripts in `package.json` |
| Your history | What you actually run, which moves your habitual choices to the top | If you always `git cherry-pick`, it outranks `checkout` after `git ch` |

For a command with no spec, suggestions come from your history and the files
in the current directory.

Your shell tells hiruzen its aliases and functions when it starts. They are
suggested as commands, and an alias is completed as what it stands for: with
`alias gco='git checkout'`, typing `gco ` lists branches. The shell also
reports each command's exit status. A command the shell could not find is
treated as a typing mistake and never suggested, and one that failed counts
for less than one that worked.

## How it learns from you

hiruzen adapts as you work; there is nothing to train or turn on.

| What it learns | Where from | What changes |
| --- | --- | --- |
| The commands and words you use | Your shell's history file, read at startup | Your habitual choices move to the top |
| What you use in this project | Its own record of each command and the project it ran in | A branch, host or path is suggested only in the project where you typed it |
| What you run after what | The order of commands in that record | After `git add`, `git commit` is more likely to lead |
| Your typing mistakes | Exit statuses the shell reports | A command the shell could not find is never suggested; a failed one counts for less |
| Your aliases and functions | Reported by the shell when it starts | They are suggested, and aliases complete as their commands |

Newer habits outweigh older ones throughout. hiruzen does not yet learn from
which suggestion you pick or pass over.

What you type is matched in four ways, best first: exact prefix,
case-insensitive prefix, substring, then letters in order (`ch` matches
`branch` and `fetch`). Within each group, suggestions are ordered by the
spec's own priority plus how often you have used them in the same context.
A single typed letter matches prefixes only.

The specs are 1,440 files converted from Fig's spec library. They cover 704
commands, plus separate specs for the services of large tools such as `aws`,
`az` and `gcloud`.
