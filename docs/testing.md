# What has been tested

| Platform | Shell | Status |
| --- | --- | --- |
| macOS (Apple silicon) | zsh 5.9, bash 3.2 | Tested in live sessions |
| Linux (arm64, Debian 12) | bash 5.2, zsh 5.9, fish 3.6, PowerShell 7.4 | Tested in live sessions |
| macOS | fish, PowerShell 7 | Not tested; the same scripts pass on Linux |
| Windows | PowerShell 7, Windows PowerShell 5, Git Bash, cmd | Builds and passes the unit tests in CI on Windows; the dropdown has not yet been tried in a live Windows terminal |

An automated suite of 71 edge cases runs the real shells on Linux: bad
quotes, pasted text, tiny and resized windows, long wrapped commands, a
deleted working directory, a broken or read-only settings folder, nested
starts, hostile input and more, in bash, zsh, fish and PowerShell.

| Run | Passed | What failed |
| --- | --- | --- |
| First | 65 of 71 | Mistyped commands were still suggested (3 cases); a file was missed in a folder of 5,000; a command recorded in bash was misread in PowerShell; one case was a fault in the test |
| After fixes | 70 of 71 | The bash-then-PowerShell case, whose fix was made after this run started |

The remaining case came from hiruzen re-reading a command typed in bash by
PowerShell's rules, where a backslash is an ordinary character. The record
now notes which shell each command was typed in.

hiruzen has also been run inside tmux on Linux, and `hiruzen ssh` against
remote bash, zsh and fish on a local test server. It has not been tried over
SSH to a real server or through a jump host.

The unit tests pass on macOS, Linux and Windows. Until the dropdown has been
used in a real Windows terminal, treat Windows support as experimental.
