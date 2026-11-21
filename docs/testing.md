# What has been tested

| Platform | Shell | Status |
| --- | --- | --- |
| macOS (Apple silicon) | zsh 5.9, bash 3.2 | Tested in live sessions |
| Linux (arm64, Debian 12) | bash 5.2, zsh 5.9, fish 3.6, PowerShell 7.4 | Tested in live sessions |
| macOS | fish, PowerShell 7 | Not tested; the same scripts pass on Linux |
| Windows 11 (x64) | Windows PowerShell 5.1, cmd | Tested in live sessions |
| Windows | PowerShell 7, Git Bash | Not tested in a live session; builds and passes the unit tests in CI on Windows |

An automated suite of 71 edge cases runs the real shells on Linux: bad
quotes, pasted text, tiny and resized windows, long wrapped commands, a
deleted working directory, a broken or read-only settings folder, nested
starts, hostile input and more, in bash, zsh, fish and PowerShell.

| Run | Passed | What failed |
| --- | --- | --- |
| First | 65 of 71 | Mistyped commands were still suggested (3 cases); a file was missed in a folder of 5,000; a command recorded in bash was misread in PowerShell; one case was a fault in the test |
| After fixes | 70 of 71 | The bash-then-PowerShell case, whose fix was made after this run started |
| After that fix | 71 of 71 | Nothing |

The last case to be fixed came from hiruzen re-reading a command typed in bash by
PowerShell's rules, where a backslash is an ordinary character. The record
now notes which shell each command was typed in.

hiruzen has also been run inside tmux on Linux, and `hiruzen ssh` against
remote bash, zsh and fish on a local test server. It has not been tried over
SSH to a real server or through a jump host.

The unit tests pass on macOS, Linux and Windows.

On Windows the dropdown was tried in Windows PowerShell 5.1 and cmd: typing,
accepting with Tab, moving the highlight, folders after `cd`, a second
prompt, clearing the screen with Ctrl+L and resizing the window, all with a
command half typed. The edge-case suite does not run on Windows, and
PowerShell 7 and Git Bash have not been tried there, so treat those two as
experimental.

## Start-up time

Time from starting a shell to its first prompt, on an Apple silicon Mac with
a minimal shell configuration (median of five runs):

| Shell | Without hiruzen | With hiruzen |
| --- | --- | --- |
| zsh | 16 ms | 25 ms |
| bash | 10 ms | 26 ms |

A real configuration with a prompt theme and plugins takes far longer than
either figure, and hiruzen's share stays about the same.

## Running the edge-case suite

`tests/edge/run.sh` builds hiruzen, installs the shells and runs every case.
It is meant for a throwaway machine, and CI runs it in a container on each
push:

```sh
docker run --rm -v "$PWD":/src:ro rust:1-slim-bookworm bash /src/tests/edge/run.sh
```
