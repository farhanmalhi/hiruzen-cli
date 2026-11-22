#!/bin/sh
# Prints how many times each release's programs were downloaded.
#
# Homebrew, the install scripts and manual downloads all fetch the archives
# from GitHub releases, so this covers nearly every install. Checksums,
# manifests and the source archive are left out: installers fetch them
# alongside the program, and scanners fetch every file once, which would
# count the same install several times.
#
# Usage: tools/downloads.sh [owner/repo]
# Set GH_TOKEN to raise the API rate limit (not required).
set -eu

repo="${1:-farhanmalhi/hiruzen-cli}"
url="https://api.github.com/repos/$repo/releases?per_page=100"

if [ -n "${GH_TOKEN:-}" ]; then
  json=$(curl -fsSL -H "Authorization: Bearer $GH_TOKEN" "$url")
else
  json=$(curl -fsSL "$url")
fi

printf '%s' "$json" | python3 -c '
import json, sys

releases = json.load(sys.stdin)
total = 0
print("%-16s %9s   %s" % ("release", "downloads", "per platform"))
for release in releases:
    archives = [
        a for a in release["assets"]
        if a["name"].endswith((".tar.xz", ".zip")) and not a["name"].startswith("source")
    ]
    count = sum(a["download_count"] for a in archives)
    total += count
    platforms = ", ".join(
        "%s %d" % (a["name"].replace("hiruzen-cli-", "").split(".")[0], a["download_count"])
        for a in archives if a["download_count"]
    )
    print("%-16s %9d   %s" % (release["tag_name"], count, platforms))
print("%-16s %9d" % ("total", total))
'
