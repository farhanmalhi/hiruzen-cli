#!/bin/bash
# Runs the edge-case suite: builds hiruzen, installs the shells it supports,
# and drives each one in a pseudo-terminal (see edge.py).
#
# Meant for a throwaway Debian or Ubuntu machine, as root: it installs
# packages and creates directories under /. CI runs it in a container.
#
#   docker run --rm -v "$PWD":/src:ro rust:1-slim-bookworm bash /src/tests/edge/run.sh
set -u
src=${HIRUZEN_SRC:-$(cd "$(dirname "$0")/../.." && pwd)}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/tmp/hiruzen-target}
export DEBIAN_FRONTEND=noninteractive TERM=xterm-256color HOME=/root

apt-get update -qq >/dev/null
apt-get install -y -qq zsh fish git python3 python3-pip curl ca-certificates less sudo >/dev/null
apt-get install -y -qq libicu72 >/dev/null 2>&1 || apt-get install -y -qq 'libicu7[0-9]' >/dev/null 2>&1 || true
pip3 install -q --break-system-packages pyte >/dev/null 2>&1 || pip3 install -q pyte >/dev/null

# PowerShell, from its release archive.
case "$(uname -m)" in aarch64 | arm64) arch=arm64 ;; *) arch=x64 ;; esac
pwsh_version=7.4.6
curl -sSL -m 600 -o /tmp/pwsh.tgz "https://github.com/PowerShell/PowerShell/releases/download/v${pwsh_version}/powershell-${pwsh_version}-linux-${arch}.tar.gz"
mkdir -p /opt/pwsh && tar xzf /tmp/pwsh.tgz -C /opt/pwsh && chmod +x /opt/pwsh/pwsh && ln -sf /opt/pwsh/pwsh /usr/local/bin/pwsh

(cd "$src" && cargo build --locked --release) || exit 1
install -m 755 "$CARGO_TARGET_DIR/release/hiruzen" /usr/local/bin/hiruzen

# The fixtures the cases expect.
mkdir -p /proj/src /proj/spaced /other /big /home-alias/.config/fish
cd /proj && git init -q . && git config user.email t@example.com && git config user.name t &&
	echo hi > a.txt && git add . && git commit -qm init && git branch feature/login
echo contents-of-spaced-file > "/proj/spaced/my file.txt"
(cd /big && for i in $(seq 1 5000); do : > "file-$i"; done)
printf 'PS1="\\w \\$ "\n' > /root/.bashrc
printf 'PS1="%%~ %%# "\n' > /root/.zshrc
printf 'PS1="\\w \\$ "\nalias gco="git checkout"\nmkcd() { mkdir -p "$1" && cd "$1"; }\n' > /home-alias/.bashrc
printf 'PS1="%%~ %%# "\nalias gco="git checkout"\nmkcd() { mkdir -p "$1" && cd "$1"; }\n' > /home-alias/.zshrc
printf 'function mkcd\n  mkdir -p $argv[1]; and cd $argv[1]\nend\n' > /home-alias/.config/fish/config.fish

python3 "$src/tests/edge/edge.py"
