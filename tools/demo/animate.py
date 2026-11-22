#!/usr/bin/env python3
"""Draws assets/demo-typing.svg: an animation of typing with hiruzen.

Each frame's dropdown is what `hiruzen complete` prints for the line typed so
far, run with an empty config folder and history switched off, so the picture
shows what a new user sees and nothing from the machine it was made on. The
branch shown is the current branch of the repository it is run in.

Run from the repository root after changing suggestions or the theme:
    python3 tools/demo/animate.py [path/to/hiruzen]
"""
import html
import json
import os
import subprocess
import sys
import tempfile

CELL_W, CELL_H, PAD, BAR = 8.4, 20, 16, 28
BG, FG, PROMPT, BRANCH = "#1e1e1e", "#d4d4d4", "#5fafff", "#87d787"
ROW_BG, ROW_FG, ROW_DESC, MARK = "#303030", "#d0d0d0", "#8a8a8a", "#5fafff"
SEL_BG, SEL_FG, SEL_DESC, SEL_MARK = "#005faf", "#ffffff", "#dadada", "#afffff"
NAME_W, DESC_W, MAX_ROWS = 18, 38, 6
PROMPT_TEXT, BRANCH_TEXT, SIGN = "my-project ", "(main) ", "% "

# What is on the command line in each frame, and how long it stays (ms).
# Each word after the first is accepted from the dropdown with Tab. The last
# frame is the command after Enter: it has run, and a new prompt is waiting.
RAN = "Everything up-to-date"
SCRIPT = [
    ("", 700),
    ("gi", 240),
    ("git", 500),
    ("git ", 900),
    ("git p", 160),
    ("git pu", 1100),
    ("git push ", 1000),
    ("git push origin ", 1500),
    ("git push origin main", 2200, RAN),
]


def suggestions(binary, line, config):
    env = dict(os.environ, HIRUZEN_CONFIG_DIR=config)
    out = subprocess.run([binary, "complete", line], capture_output=True, text=True, env=env).stdout
    rows = []
    for row in out.splitlines()[:MAX_ROWS]:
        name, _, description = row.partition("  ")
        rows.append((name.strip(), description.strip()))
    return rows


def text(x, y, value, fill, bold=False):
    weight = ' font-weight="bold"' if bold else ""
    # Fonts differ between viewers; fixing each run's width keeps the text on
    # the character grid the cursor and dropdown are placed by.
    length = f' textLength="{len(value) * CELL_W:.1f}" lengthAdjust="spacingAndGlyphs"' if value.strip() else ""
    return f'<text x="{x:.1f}" y="{y}" fill="{fill}"{weight}{length} xml:space="preserve">{html.escape(value)}</text>'


def prompt(x, baseline):
    out = [text(x, baseline, PROMPT_TEXT, PROMPT, bold=True)]
    x += len(PROMPT_TEXT) * CELL_W
    out.append(text(x, baseline, BRANCH_TEXT, BRANCH))
    x += len(BRANCH_TEXT) * CELL_W
    out.append(text(x, baseline, SIGN, FG))
    return out


def ran(typed, output, start_x, top):
    """The command after Enter: its output, then the next prompt."""
    out = [text(start_x, top + 14, typed, FG)]
    out.append(text(PAD, top + CELL_H + 14, output, FG))
    out.extend(prompt(PAD, top + 2 * CELL_H + 14))
    cursor_x = start_x
    out.append(f'<rect x="{cursor_x:.1f}" y="{top + 2 * CELL_H}" width="{CELL_W}" height="{CELL_H - 2}" fill="{FG}"/>')
    return out


def frame(typed, rows, start_x, top):
    out = []
    baseline = top + 14
    out.append(text(start_x, baseline, typed, FG))
    cursor_x = start_x + len(typed) * CELL_W
    out.append(f'<rect x="{cursor_x:.1f}" y="{top}" width="{CELL_W}" height="{CELL_H - 2}" fill="{FG}"/>')
    if not rows:
        return out
    # The dropdown starts under the word being typed.
    word = typed.rsplit(" ", 1)[-1]
    left = cursor_x - len(word) * CELL_W
    cols = 2 + NAME_W + 2 + DESC_W + 1
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
    return out


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "hiruzen"
    with tempfile.TemporaryDirectory() as config:
        with open(os.path.join(config, "settings.json"), "w") as handle:
            json.dump({"autocomplete.history": False, "autocomplete.onlineSuggestions": False}, handle)
        frames = [
            (step[0], suggestions(binary, step[0], config) if step[0] and len(step) == 2 else [], step[1])
            for step in SCRIPT
        ]
    outputs = {index: step[2] for index, step in enumerate(SCRIPT) if len(step) == 3}
    if not any(rows for _, rows, _ in frames):
        sys.exit("hiruzen printed no suggestions; pass the path to the binary")

    prefix = len(PROMPT_TEXT) + len(BRANCH_TEXT) + len(SIGN)
    longest = max(len(typed) for typed, _, _ in frames)
    cols = 2 + NAME_W + 2 + DESC_W + 1
    # Wide enough for the line typed and for each dropdown, which starts
    # under the word being typed.
    widest = max(
        [prefix + longest + 1]
        + [prefix + len(typed) - len(typed.rsplit(" ", 1)[-1]) + cols for typed, rows, _ in frames if rows]
    )
    width = PAD * 2 + widest * CELL_W
    height = PAD * 2 + BAR + (MAX_ROWS + 1) * CELL_H + 8
    top = PAD + BAR
    total = sum(ms for _, _, ms in frames)

    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0f}" height="{height:.0f}" viewBox="0 0 {width:.0f} {height:.0f}" '
        'font-family="ui-monospace, SFMono-Regular, Menlo, Consolas, monospace" font-size="14" role="img" '
        'aria-label="Typing git push origin main with hiruzen: a dropdown suggests each next word, '
        'and Tab accepts it, ending with the current branch">',
        "<style>",
        f".f{{opacity:0;animation-duration:{total}ms;animation-iteration-count:infinite;animation-timing-function:step-end}}",
    ]
    elapsed = 0
    for index, (_, _, ms) in enumerate(frames):
        start, end = elapsed / total * 100, (elapsed + ms) / total * 100
        elapsed += ms
        steps = [f"0%{{opacity:0}}", f"{start:.3f}%{{opacity:1}}", f"{end:.3f}%{{opacity:0}}"]
        if index == 0:
            steps = [f"0%{{opacity:1}}", f"{end:.3f}%{{opacity:0}}"]
        out.append(f"@keyframes k{index}{{{''.join(steps)}100%{{opacity:0}}}}")
        out.append(f".f{index}{{animation-name:k{index}}}")
    # Without animation (reduced motion, or a viewer that ignores CSS), the
    # last frame with a dropdown stays up on its own.
    still = max(i for i, (_, rows, _) in enumerate(frames) if rows)
    out.append(f"@media (prefers-reduced-motion:reduce){{.f{{animation:none}}.f{still}{{opacity:1}}}}")
    out.append("</style>")
    out.append(f'<rect width="100%" height="100%" rx="10" fill="{BG}"/>')
    out.append('<circle cx="22" cy="20" r="6" fill="#ff5f56"/><circle cx="42" cy="20" r="6" fill="#ffbd2e"/><circle cx="62" cy="20" r="6" fill="#27c93f"/>')
    out.extend(prompt(PAD, top + 14))
    x = PAD + prefix * CELL_W
    for index, (typed, rows, _) in enumerate(frames):
        out.append(f'<g class="f f{index}">')
        if index in outputs:
            out.extend(ran(typed, outputs[index], x, top))
        else:
            out.extend(frame(typed, rows, x, top))
        out.append("</g>")
    out.append("</svg>")
    with open("assets/demo-typing.svg", "w", encoding="utf-8") as handle:
        handle.write("\n".join(out) + "\n")
    print(f"wrote assets/demo-typing.svg ({len(frames)} frames, {total / 1000:.1f}s)")
    for typed, rows, _ in frames:
        print(f"  {typed!r:24} {', '.join(name for name, _ in rows)}")


if __name__ == "__main__":
    main()
