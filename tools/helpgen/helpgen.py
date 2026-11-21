#!/usr/bin/env python3
"""Builds a completion spec for a command from its --help output.

For tools the Fig spec library does not cover. It understands the help layout
most CLI frameworks print: an "Options:" section and a "Commands:" section of
two-space-indented entries, each followed by its description. Subcommands are
followed by running `<command> <subcommand> --help`.

usage: helpgen.py COMMAND [--depth N] > specs/COMMAND.json

Only `--help` is ever run. Review the result before committing it.
"""
import json
import re
import subprocess
import sys

MAX_DESCRIPTION = 140


def help_text(argv):
    try:
        result = subprocess.run(argv + ["--help"], capture_output=True, text=True, timeout=20, stdin=subprocess.DEVNULL)
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return result.stdout or result.stderr


def sections(text):
    """Maps a section title ("options", "commands") to its entries: (head, description)."""
    found = {}
    title = None
    entry = None
    for line in text.splitlines():
        heading = re.match(r"^([A-Za-z][A-Za-z ]*):\s*$", line)
        if heading:
            title = heading.group(1).strip().lower()
            found.setdefault(title, [])
            entry = None
            continue
        if title is None or not line.strip():
            continue
        indent = len(line) - len(line.lstrip())
        if indent == 2:
            parts = re.split(r"\s{2,}", line.strip(), maxsplit=1)
            entry = [parts[0], parts[1] if len(parts) > 1 else ""]
            found[title].append(entry)
        elif indent > 2 and entry is not None:
            entry[1] = (entry[1] + " " + line.strip()).strip()
    return found


def shorten(description):
    description = " ".join(description.split())
    first = re.split(r"(?<=[.!?])\s", description, maxsplit=1)[0]
    if len(first) > MAX_DESCRIPTION:
        first = first[: MAX_DESCRIPTION - 1].rstrip() + "…"
    return first.rstrip(".")


def arguments(text):
    """`<name>`, `[name]` and `<name...>` in a usage fragment."""
    args = []
    for required, optional in re.findall(r"<([^>]+)>|\[([^\]]+)\]", text):
        name = required or optional
        if name == "options":
            continue
        arg = {"name": name.rstrip(".")}
        if name.endswith("..."):
            arg["isVariadic"] = True
        if optional:
            arg["isOptional"] = True
        args.append(arg)
    return args


def option(head, description):
    names = []
    usage = ""
    for part in head.split(", "):
        match = re.match(r"^(--?[A-Za-z0-9][\w-]*)(.*)$", part.strip())
        if not match:
            continue
        names.append(match.group(1))
        usage = match.group(2) or usage
    if not names:
        return None
    spec = {"name": names if len(names) > 1 else names[0], "description": shorten(description)}
    args = arguments(usage)
    if args:
        choices = re.search(r"\(choices:\s*([^)]*)\)", description)
        if choices:
            args[0]["suggestions"] = [c.strip().strip('"') for c in choices.group(1).split(",") if c.strip()]
        spec["args"] = args[0] if len(args) == 1 else args
    return spec


def build(argv, description, depth):
    found = sections(help_text(argv))
    spec = {"name": argv[-1], "description": shorten(description)}
    options = [o for o in (option(head, text) for head, text in found.get("options", [])) if o]
    if options:
        spec["options"] = options
    subcommands = []
    for head, text in found.get("commands", []):
        words = head.split()
        names = words[0].split("|")
        if names[0] == "help":
            continue
        if depth > 0:
            sub = build(argv + [names[0]], text, depth - 1)
        else:
            sub = {"name": names[0], "description": shorten(text)}
        sub["name"] = names if len(names) > 1 else names[0]
        args = arguments(" ".join(words[1:]))
        if args:
            sub["args"] = args[0] if len(args) == 1 else args
        subcommands.append(sub)
    if subcommands:
        spec["subcommands"] = subcommands
    return spec


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    depth = 3
    if "--depth" in sys.argv:
        depth = int(sys.argv[sys.argv.index("--depth") + 1])
    command = sys.argv[1]
    text = help_text([command])
    if not text.strip():
        sys.exit(f"{command} --help printed nothing")
    first = next((line.strip() for line in text.splitlines()[1:] if line.strip() and not line.startswith(" ")), "")
    spec = build([command], first.split(" - ")[0] if first else command, depth)
    json.dump(spec, sys.stdout, indent=1, ensure_ascii=False)
    print()


if __name__ == "__main__":
    main()
