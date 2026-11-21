# Changelog

All notable changes to hiruzen are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Suggestions follow the project you are in.** hiruzen now records the
  commands you run together with the project (git repository) each ran in,
  and ranks by that project's habits first. Values such as branch names are
  only suggested from the project they were typed in.
- **The dropdown says when it is searching online.** A word such as the
  package in `npm install -g rea` is looked up in the registry. What hiruzen already
  knows is now listed at once, with a `Searching online…` line under it
  until the registry answers. The search starts when you pause typing, so a
  word typed quickly is looked up once and not once per letter, and a word
  already looked up in the last minute is not looked up again.
- **The dropdown says when there is no internet.** If a search cannot reach
  the network, the dropdown shows `No internet` and lists what your machine
  knows. hiruzen waits 20 seconds before trying the network again.
- **A better stock prompt in zsh.** If your prompt is the one zsh or macOS
  ships, hiruzen now colours it, shows the git branch in brackets
  (`my-project (main) %`) and leaves out your user and machine name. The
  branch is shown whole unless the window is too narrow for it; a name that
  is too long loses its middle. With no branch checked out it shows the
  first seven characters of the commit. A prompt you or a theme set is left
  as it is. Set `prompt.enable` to `false` to keep the stock prompt,
  `prompt.showHost` to `true` to get `you@host` back, and
  `prompt.directoryLength` or `prompt.branchLength` to change the lengths.

- **Aliases and shell functions.** They are suggested as commands, and an
  alias is completed as the command it stands for (zsh, bash and fish).
- **Mistyped and failed commands.** The shells report each command's exit
  status. A command that was not found is never learned, and a failed one
  counts for less.
- **The command you ran just before helps pick the next one.** On public
  data from 50 users this raised the right command being the first
  suggestion from 91.2% to 96.8%, with two letters typed.
- **`hiruzen evaluate --file`** measures suggestion quality on any file of
  commands, and the report now separates what could not be known from how
  well the rest was ordered.

### Changed

- **Online suggestions are now on by default.** Specs may search a package
  registry for the word you are typing, which sends that word to the
  registry. Set `autocomplete.onlineSuggestions` to `false` to keep
  everything on your machine, as before.
- **`git push` and `git pull` offer the current branch first.** For
  `git checkout`, `switch`, `merge` and `rebase` it is listed after the
  other branches.
- **Enter now inserts the suggestion you picked.** After you move the
  highlight with `Up` or `Down`, `Enter` inserts that suggestion instead of
  running the command. If you have not moved the highlight, `Enter` still
  runs the command as typed. Set `autocomplete.enterAccepts` to `false` for
  the old behaviour.

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
