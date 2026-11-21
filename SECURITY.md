# Security Policy

## Supported Versions

hiruzen is released from the `main` branch. Security fixes go into the latest
release only.

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |
| < 0.1   | :x:                |

## Reporting a Vulnerability

Please do not open a public issue for a security problem.

Report it privately through GitHub: open the
[Security tab](https://github.com/farhanmalhi/hiruzen-cli/security) of this
repository and choose **Report a vulnerability**. Include:

- the hiruzen version (`hiruzen --version`), your operating system and shell
- what you typed, or the spec and directory involved, and what happened
- what an attacker could gain from it

What to expect:

- an acknowledgement within 5 working days
- an assessment, with a decision to accept or decline, within 14 days
- for an accepted report, a fix in the next release and credit in the release
  notes, unless you ask not to be named
- for a declined report, a short explanation of why

Please give us a reasonable time to release a fix before you publish details.

## What counts

hiruzen runs inside your terminal session, so these are the areas we most want
to hear about:

- a completion spec, or the directory you are in, making hiruzen run a command
  other than the read-only listing commands described in the README
- spec JavaScript escaping the embedded sandbox (reaching files, the network
  or timers)
- the shell integration scripts or the temporary directory being readable or
  replaceable by another user
- keystrokes or terminal output being stored or sent anywhere

Bugs in the tools that hiruzen completes (for example `git` or `kubectl`)
belong to those projects.
