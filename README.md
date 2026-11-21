# hiruzen

Dropdown autocomplete for your terminal. As you type a command, hiruzen shows
what the next word can be (subcommands, options, branches, files, scripts),
so you do not have to remember it.

```
$ git ch
      checkout        Switch branches or restore working tree files
      cherry-pick     Apply the changes introduced by some existing commits
      branch          List, create, or delete branches
```

- **One binary, no runtime.** Written in Rust, with the completion specs
  embedded. Nothing else to install.
- **Offline.** hiruzen makes no network requests.
- **Cross-platform.** macOS, Linux and Windows, with zsh, bash, fish,
  PowerShell and cmd.
- **Safe to enable.** If hiruzen cannot start, your shell opens as normal.

## Contents

- [Install](#install)
- [Usage](#usage)
- [How it works](#how-it-works)
- [Shell support](#shell-support)
- [Privacy and security](#privacy-and-security)
- [Limits](#limits)
- [Troubleshooting](#troubleshooting)
- [Uninstall](#uninstall)
- [Development](#development)
- [Credits](#credits)

## Install

### 1. Build the binary

hiruzen is built from source. You need a [Rust toolchain](https://rustup.rs),
version 1.80 or newer. From a clone of this repository:

```sh
cargo install --locked --path .
```

This puts `hiruzen` in `~/.cargo/bin`, which must be on your `PATH`. Check it:

```sh
hiruzen --version
```

### 2. Try it

Run `hiruzen` in a terminal. It starts your shell with suggestions switched
on and changes no files. Type `exit` to leave.

### 3. Start it with every new terminal

Add one line to the **end** of your shell's startup file:

| Shell | File | Line to add |
| --- | --- | --- |
| zsh | `~/.zshrc` | `eval "$(hiruzen init zsh)"` |
| bash, Git Bash | `~/.bashrc` | `eval "$(hiruzen init bash)"` |
| fish | `~/.config/fish/config.fish` | `hiruzen init fish \| source` |
| PowerShell 7 | `$PROFILE` | `hiruzen init pwsh \| Out-String \| Invoke-Expression` |
| Windows PowerShell 5 | `$PROFILE` | `hiruzen init powershell \| Out-String \| Invoke-Expression` |
| cmd | none | Run `hiruzen --shell cmd`, or set it as the command line of a terminal profile |

The line only acts in an interactive shell that is not already inside
hiruzen. It does nothing if the `hiruzen` binary is missing, and if hiruzen
fails to start, the shell carries on without it. Scripts, `ssh host command`
and other non-interactive shells are not affected.

## Usage

### Keys

While suggestions are showing:

| Key | Action |
| --- | --- |
| `Tab` | Accept the highlighted suggestion |
| `Down` / `Up` | Move the highlight |
| `Esc` | Hide suggestions for the current word |
| `Enter` | Run the command as typed, without accepting a suggestion |

`Up` on the first suggestion goes to your shell's history as usual. While you
are browsing history with the arrow keys, suggestions stay hidden until you
type again. When no suggestions are showing, every key goes to your shell
unchanged, so `Tab` does your shell's own completion.

The dropdown shows up to eight suggestions at a time and scrolls through the
rest. It opens below the cursor, or above it when there is no room below.

### Commands

```
hiruzen [OPTIONS]              Start your shell with suggestions
hiruzen init <SHELL>           Print the line that starts hiruzen with each new terminal
hiruzen complete <COMMAND>     Print the suggestions for a partly typed command
hiruzen --help | --version
```

| Option | Meaning |
| --- | --- |
| `-s`, `--shell <SHELL>` | Shell to start: `zsh`, `bash`, `fish`, `pwsh`, `powershell` or `cmd`. Default: `$SHELL` on macOS and Linux (bash if it is unset or not supported); on Windows, PowerShell 7 if installed, otherwise Windows PowerShell. |
| `-l`, `--login` | Start it as a login shell. |
| `--auto` | Exit quietly when hiruzen is already running or there is no terminal. Used by the line `hiruzen init` prints; you should not need it yourself. |

`hiruzen` exits with the exit status of the shell it ran, or with 2 if it
could not start.

### Checking what hiruzen knows

`hiruzen complete` prints the suggestions for a partly typed command without
starting a session. Use it to see what hiruzen offers for a tool, or to
report a problem:

```sh
hiruzen complete "git ch"
hiruzen complete "docker run --"
hiruzen complete "git checkout "         # trailing space: complete the next word
hiruzen complete --shell pwsh "git ch"   # as another shell would see it
```

It prints up to 30 suggestions, and exits with 1 when there is nothing to
complete at that position.

### Detecting a hiruzen session

Inside a session the environment variable `HIRUZEN` is set to `1`. Your own
startup scripts can test it.

## How it works

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

What you type is matched in four ways, best first: exact prefix,
case-insensitive prefix, substring, then letters in order (`ch` matches
`branch` and `fetch`). Within each group, suggestions are ordered by the
spec's own priority plus how often you have used them in the same context.
A single typed letter matches prefixes only.

The specs are 1,440 files converted from Fig's spec library. They cover 704
commands, plus separate specs for the services of large tools such as `aws`,
`az` and `gcloud`.

## Shell support

| Shell | Platforms | History read from | Notes |
| --- | --- | --- | --- |
| zsh | macOS, Linux | `$HISTFILE`, otherwise `.zsh_history` in `$ZDOTDIR` or your home directory | |
| bash | macOS, Linux, Windows | `~/.bash_history` | On Windows, `bash` means Git Bash from [Git for Windows](https://gitforwindows.org) |
| fish | macOS, Linux | `fish/fish_history` under `$XDG_DATA_HOME` or `~/.local/share` | |
| PowerShell 7 (`pwsh`) | Windows, macOS, Linux | PSReadLine's `ConsoleHost_history.txt` | |
| Windows PowerShell 5 (`powershell`) | Windows | PSReadLine's `ConsoleHost_history.txt` | |
| cmd | Windows | none | Suggestions come from specs and live context only |

## Privacy and security

- **No network access.** hiruzen sends nothing anywhere. The specs are inside
  the binary.
- **History is read, never written.** hiruzen reads the last 20,000 lines of
  your shell's history file at startup. It keeps commands you run during a
  session in memory only, and stores nothing on disk.
- **It runs local commands as you type.** Live suggestions come from running
  read-only commands in the current directory: `git` to list branches, tags
  and changed files, and the listing commands that a tool's spec names (for
  example a spec may run its tool to list its resources). Each command has no
  input, is stopped after 1.5 seconds, and has its result reused for 4
  seconds. These commands come from the embedded spec library, not from the
  directory you are in.
- **Temporary files.** The integration scripts are written to a private
  directory (`hiruzen-<pid>` under the system temp directory, mode `0700` on
  macOS and Linux) and removed when the session ends.
- **PowerShell execution policy.** hiruzen starts PowerShell with
  `-ExecutionPolicy Bypass` so that its integration script can load. This
  applies to that PowerShell process only and does not change your system
  setting.

## Limits

- Suggestions are valid and well ranked, not guaranteed correct: hiruzen
  cannot know which of several valid words you intend.
- Suggestions appear only at a shell prompt, with the cursor at the end of
  what you have typed. They are hidden inside full-screen programs such as
  `vim` and `less`.
- In a pipeline, only the last command is completed.
- Many specs compute some suggestions with JavaScript, which hiruzen does not
  run. Native replacements exist for git, npm and yarn dependencies and
  scripts, ssh hosts, make targets and directory arguments. For other tools
  the output of the spec's command is parsed heuristically (first column, or
  the name field of JSON output), and suggestions that need a pure JavaScript
  function (for example Kubernetes resource names) are not offered. Their
  subcommands and options still complete.
- Colours, keys and the number of rows shown are fixed. There is no
  configuration file.
- Nushell, Xonsh and other shells not listed above are not supported.

## Troubleshooting

| Problem | What to do |
| --- | --- |
| `hiruzen: command not found` | Add `~/.cargo/bin` to your `PATH`, above the `hiruzen init` line in your startup file. |
| New terminals open without suggestions | Check that the `init` line is at the end of the startup file and that `hiruzen --version` works in that shell. Then run `hiruzen` by hand: it prints the reason if it cannot start. |
| `hiruzen: already running in this terminal` | You are inside a session already. Nothing more is needed. |
| `hiruzen: needs an interactive terminal` | hiruzen was started with its input or output redirected. Run it directly in a terminal. |
| `hiruzen: Git Bash was not found` | Install [Git for Windows](https://gitforwindows.org), or choose another shell with `--shell`. |
| `hiruzen: <shell> was not found on PATH` | Install that shell, or name one you have with `--shell`. |
| A tool gives no suggestions, or wrong ones | Run `hiruzen complete "<what you typed>"` to see what hiruzen offers. Include that output when you report the problem. |
| You need a shell without hiruzen | Start one with the variable set, for example `HIRUZEN=1 zsh`. The `init` line then skips hiruzen. |

## Uninstall

1. Remove the `hiruzen init` line from your shell's startup file.
2. Open a new terminal, then run `cargo uninstall hiruzen`.

hiruzen leaves no configuration or data files behind.

## Development

```sh
cargo build                              # debug build
cargo run                                # start a session with the debug build
cargo test                               # unit tests, including parsing every embedded spec
cargo clippy --locked -- -D warnings     # lints; CI fails on any warning
```

CI builds, tests and lints every push to `main` and every pull request on
Ubuntu, macOS and Windows, so changes must pass on all three.

### Source layout

| Path | Contents |
| --- | --- |
| `src/main.rs` | Command-line parsing and the `init` and `complete` commands |
| `src/engine/` | Tokenizer, spec walker, generators (live context), history ranking |
| `src/specs.rs` | Spec model and the embedded archive reader |
| `src/term/` | Screen model, dropdown layout, prompt markers, the pseudo-terminal session |
| `src/shell.rs`, `shell/` | Per-shell launch logic and integration scripts |
| `specs/specs.bin` | The embedded spec archive (generated, committed) |
| `tools/specgen/` | Converts the Fig spec library into `specs/specs.bin` |

### Updating the specs

`specs/specs.bin` is generated from
[@withfig/autocomplete](https://github.com/withfig/autocomplete) and committed,
so building hiruzen does not need Node. To regenerate it you need Node 22.15
or newer:

```sh
cd tools/specgen
npm install
npm run build
```

To move to a newer spec library, change the `@withfig/autocomplete` version in
`tools/specgen/package.json` first. Run `cargo test` afterwards: it checks that
every spec in the new archive still parses.

## Credits

The completion specs come from Fig's open-source spec library, and the
approach and shell integration scripts are adapted from Microsoft's
inshellisense. Both are MIT licensed; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
