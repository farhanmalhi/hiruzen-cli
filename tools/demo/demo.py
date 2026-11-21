#!/usr/bin/env python3
"""Draws assets/demo.svg: a picture of the hiruzen dropdown in the dark theme.

The rows are what `hiruzen complete "git ch"` prints; the colours are the
dark theme's (src/settings.rs). Run from the repository root after changing
either:  python3 tools/demo/demo.py
"""
import html
import subprocess
import sys

CELL_W, CELL_H, PAD = 8.4, 20, 16
BG, FG, PROMPT, BRANCH = "#1e1e1e", "#d4d4d4", "#5fafff", "#87d787"
ROW_BG, ROW_FG, ROW_DESC, MARK = "#303030", "#d0d0d0", "#8a8a8a", "#5fafff"
SEL_BG, SEL_FG, SEL_DESC, SEL_MARK = "#005faf", "#ffffff", "#dadada", "#afffff"
NAME_W, DESC_W = 16, 46


def suggestions(line, limit):
    binary = sys.argv[1] if len(sys.argv) > 1 else "hiruzen"
    out = subprocess.run([binary, "complete", line], capture_output=True, text=True).stdout
    rows = []
    for row in out.splitlines()[:limit]:
        name, _, description = row.partition("  ")
        rows.append((name.strip(), description.strip()))
    return rows


def text(x, y, value, fill, bold=False):
    weight = ' font-weight="bold"' if bold else ""
    return f'<text x="{x:.1f}" y="{y}" fill="{fill}"{weight} xml:space="preserve">{html.escape(value)}</text>'


def main():
    rows = suggestions("git ch", 6)
    if not rows:
        sys.exit("hiruzen printed no suggestions; pass the path to the binary")
    prompt, typed = "my-project ", "git ch"
    cols = 2 + NAME_W + 2 + DESC_W + 1
    # The dropdown starts under the typed word, after the prompt.
    indent = len(prompt) + 7 + 2
    width = PAD * 2 + (indent + cols) * CELL_W
    height = PAD * 2 + 28 + (len(rows) + 1) * CELL_H + 8
    top = PAD + 28
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}" viewBox="0 0 {width:.0f} {height:.0f}" '
        'font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="14" role="img" '
        'aria-label="A terminal with git ch typed and a dropdown of matching git subcommands underneath">',
        f'<rect width="100%" height="100%" rx="10" fill="{BG}"/>',
        '<circle cx="22" cy="20" r="6" fill="#ff5f56"/><circle cx="42" cy="20" r="6" fill="#ffbd2e"/><circle cx="62" cy="20" r="6" fill="#27c93f"/>',
    ]
    baseline = top + 14
    x = PAD
    out.append(text(x, baseline, prompt, PROMPT, bold=True))
    x += len(prompt) * CELL_W
    out.append(text(x, baseline, "(main) ", BRANCH))
    x += 7 * CELL_W
    out.append(text(x, baseline, "% ", FG))
    x += 2 * CELL_W
    start = x
    out.append(text(x, baseline, typed, FG))
    x += len(typed) * CELL_W
    out.append(f'<rect x="{x:.1f}" y="{top}" width="{CELL_W}" height="{CELL_H - 2}" fill="{FG}"/>')
    left = start
    for index, (name, description) in enumerate(rows):
        y = top + (index + 1) * CELL_H
        selected = index == 0
        bg, fg, desc, mark = (SEL_BG, SEL_FG, SEL_DESC, SEL_MARK) if selected else (ROW_BG, ROW_FG, ROW_DESC, MARK)
        out.append(f'<rect x="{left:.1f}" y="{y}" width="{cols * CELL_W:.1f}" height="{CELL_H}" fill="{bg}"/>')
        base = y + 14
        out.append(text(left, base, ">", mark))
        label = name + (" ⇥" if selected else "")
        out.append(text(left + 2 * CELL_W, base, label[:NAME_W], fg, bold=selected))
        shown = description if len(description) <= DESC_W else description[: DESC_W - 1] + "…"
        out.append(text(left + (2 + NAME_W + 2) * CELL_W, base, shown, desc))
    out.append("</svg>")
    with open("assets/demo.svg", "w", encoding="utf-8") as handle:
        handle.write("\n".join(out) + "\n")
    print(f"wrote assets/demo.svg ({len(rows)} suggestions)")


if __name__ == "__main__":
    main()
