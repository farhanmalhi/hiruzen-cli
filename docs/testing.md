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
starts, hostile input and more, in bash, zsh, fish and PowerShell. Its first
run passed 65. Of the six failures, four were bugs that are now fixed
(mistyped commands still being suggested, and a file missed in a folder of
5,000), one is the PowerShell limit listed under [Limits](../README.md#limits), and one
was a fault in the test itself.

hiruzen has also been run inside tmux on Linux, and `hiruzen ssh` against
remote bash, zsh and fish on a local test server. It has not been tried over
SSH to a real server or through a jump host.

The unit tests pass on macOS, Linux and Windows. Until the dropdown has been
used in a real Windows terminal, treat Windows support as experimental.
