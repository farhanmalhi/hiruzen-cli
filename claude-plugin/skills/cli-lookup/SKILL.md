---
name: cli-lookup
description: Look up the valid subcommands, options and argument values of a command-line tool before writing a command, using the local hiruzen CLI. Covers about 700 tools (git, docker, kubectl, aws, npm, cargo, gh, claude and more) and needs no network.
when_to_use: Use when you are about to run or suggest a shell command and are not certain a subcommand or flag exists, when a command failed with "unknown option" or "unknown command", or when the user asks what options a tool has.
argument-hint: "[partly typed command]"
allowed-tools: Bash(hiruzen complete *), Bash(hiruzen --version)
---

`hiruzen complete` prints what can come next in a partly typed command. It
reads a completion spec for the tool, so the answer is what the tool accepts,
not a guess.

## How to query

Pass the command typed so far as one quoted argument.

- End with a space to list what can follow:
  `hiruzen complete "git rebase "`
- End with part of a word to list what completes it:
  `hiruzen complete "docker run --net"`
- Type a dash to list options only:
  `hiruzen complete "kubectl get -"`

Each line is a suggestion, followed by its description when it has one.
Options that take a value show that value's name in `hiruzen complete` only
after you add the option and a space: `hiruzen complete "git commit -m "`.

Output is capped at 30 lines. Narrow it by typing more of the word.

## Reading the result

- Exit status 1 with no output means nothing completes at that position:
  the flag or subcommand you typed is probably wrong. Check one word earlier.
- A line described as "From your history" is something this user has typed
  at that position before. It is evidence of their habits, not of validity.
- File and folder names come from the current directory.
- Live values such as branches or containers are read from this machine by
  running the tool's own listing command.

## Limits

- A tool with no spec returns only history and file names. That does not mean
  the tool has no subcommands; fall back to its `--help`.
- The specs describe a tool's interface at the time they were written. If the
  result contradicts the tool's own `--help`, trust `--help`.

If `hiruzen` is not installed, say so and suggest the `setup` skill; do not
guess at its output.
