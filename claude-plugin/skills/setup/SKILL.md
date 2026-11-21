---
name: setup
description: Install, check or remove hiruzen, the terminal autocomplete tool, and change its settings.
when_to_use: Use when the user asks to install or set up hiruzen, when a hiruzen command is not found, or when the user wants to change hiruzen's theme or settings.
disable-model-invocation: true
allowed-tools: Bash(hiruzen doctor), Bash(hiruzen --version), Bash(hiruzen integrations status *), Bash(hiruzen settings), Bash(hiruzen settings docs)
---

hiruzen shows a dropdown of suggestions as the user types in their own
terminal. It is separate from this session: installing it changes the user's
shell, not Claude Code.

## Check first

Run `hiruzen --version`. If it works, run `hiruzen doctor`: it reports what is
set up and gives a fix for each problem it finds.

## Install

hiruzen is built from source with a Rust toolchain (1.91 or newer):

```sh
cargo install --locked --git https://github.com/farhanmalhi/hiruzen-cli
```

Then `hiruzen install` adds it to the startup file of each shell it finds.
That edits files such as `~/.zshrc`, so ask the user before running it, and
tell them it takes effect in new terminals.

## Change settings

`hiruzen settings` lists every setting and its value, and
`hiruzen settings docs` explains them. Set one with
`hiruzen settings <key> <value>`, for example
`hiruzen settings autocomplete.theme nord`. A running terminal picks up the
change at its next prompt.

## Remove

`hiruzen uninstall` removes what `hiruzen install` added. Ask before running
it. Then `cargo uninstall hiruzen-cli`.
