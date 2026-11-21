# Changelog

All notable changes to hiruzen are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-21

The first release. hiruzen shows a dropdown of what the next word of your
command can be, as you type it.

### Added

- **Dropdown autocomplete at the shell prompt.** Suggestions for subcommands,
  options and arguments appear under the cursor as you type. `Tab` accepts,
  `Up` and `Down` move the highlight, `Esc` hides the dropdown, and `Enter`
  runs the command as typed.
- **Specs for 704 command-line tools**, converted from Fig's spec library and
  embedded in the binary. Large tools such as `aws` and `gcloud` have
  separate specs for their services.
- **Live suggestions from your machine**: git branches, tags and changed
  files, the scripts in `package.json`, and the values a tool's spec lists by
  running the tool.
- **Ranking from your shell history**, so the commands you actually run come
  first. History is read, never written.
- **Fuzzy matching**: exact prefix, case-insensitive prefix, substring, then
  letters in order.
- **Shell support**: zsh, bash, fish and PowerShell 7 on macOS and Linux;
  PowerShell 7, Windows PowerShell 5, Git Bash and cmd on Windows.
- **`hiruzen install` and `hiruzen uninstall`** add hiruzen to your shell's
  startup file and remove it again. `hiruzen init <SHELL>` prints the line to
  add by hand, and `hiruzen integrations status` shows which shells start
  hiruzen.
- **`hiruzen settings`** lists, reads and changes settings: dropdown height,
  descriptions, fuzzy search, history, commands to skip, and whether `Enter`
  accepts a suggestion. A running session picks up a change at its next
  prompt.
- **Six colour themes**: `dark`, `light`, `dracula`, `nord`, `gruvbox` and
  `solarized`. `hiruzen theme --list` previews them.
- **`hiruzen doctor`** checks the installation, the settings file, each shell
  and the current terminal, and says how to fix what it finds.
- **`hiruzen complete <COMMAND>`** prints the suggestions for a partly typed
  command without starting a session.
- **`hiruzen issue`** opens a GitHub issue form with your system details
  filled in.
- **Saved scripts.** A script is a saved command with named blanks, kept as
  a plain file. `hiruzen run <NAME>` asks for each blank and runs it, and
  `hiruzen scripts` creates, edits, shows and removes them.
- **Shell plugins.** `hiruzen plugins add` loads a directory of shell code
  (prompts, aliases, functions) into each new shell, from a local path or a
  git URL. `hiruzen plugins update` pulls the cloned ones.
- **`hiruzen evaluate`** measures how often the word you typed next was the
  first suggestion, in the top three or in the top eight, by replaying your
  own shell history. It prints totals only and runs no commands.
- **Prebuilt binaries** for macOS (Apple silicon and Intel), Linux (x64 and
  arm64) and Windows (x64), installed with Homebrew
  (`brew install farhanmalhi/tap/hiruzen`) or with the shell and PowerShell
  installer scripts attached to each release.

### Privacy and security

- hiruzen makes no network requests while you type. Suggestions that a spec
  would fetch from the network are not offered. The one command that uses the
  network is `hiruzen plugins add <URL>`, which runs `git clone` because you
  asked it to.
- JavaScript in specs runs in a sandbox with no file, network or timer
  access. The commands it asks hiruzen to run get no input, are stopped after
  3 seconds, and come from the embedded spec library only.
- If hiruzen cannot start, your shell opens as normal.

### Known limitations

- **Windows is experimental.** It builds and passes the unit tests, but the
  dropdown has not yet been tried in a live Windows terminal.
- fish and PowerShell 7 on macOS are not tested; the same scripts pass on
  Linux.
- In a pipeline, only the last command is completed.
- The keys cannot be rebound.
- Six tools (`az`, `heroku`, `shopify`, `fig`, `infracost` and
  `@usermn/sdc`) build their spec at run time, so they have no spec;
  suggestions for them come from your history and the current directory.
- Nushell, Xonsh and other shells not listed above are not supported.

[Unreleased]: https://github.com/farhanmalhi/hiruzen-cli/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/farhanmalhi/hiruzen-cli/releases/tag/v0.1.0
