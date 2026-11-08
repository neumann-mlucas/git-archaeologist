# /// script
# requires-python = ">=3.11"
# dependencies = ["matplotlib"]
# ///
"""README chart: lines written per year, split into alive at HEAD vs deleted.

    git-archaeologist survival --format tsv > survival.tsv
    uv run docs/survival_chart.py survival.tsv ratatui

Writes docs/survival-light.svg and docs/survival-dark.svg. Colors are two
steps of one validated blue ramp per theme (alive = strong step).
"""

import csv
import sys
from collections import defaultdict

import matplotlib

matplotlib.use("svg")
import matplotlib.pyplot as plt

THEMES = {  # surfaces are GitHub's page backgrounds
    "light": dict(alive="#1c5cab", dead="#86b6ef", ink="#52514e",
                  muted="#898781", grid="#e1e0d9", gap="#ffffff"),
    "dark": dict(alive="#9ec5f4", dead="#256abf", ink="#c3c2b7",
                 muted="#898781", grid="#2c2c2a", gap="#0d1117"),
}


def load(path):
    born, alive = defaultdict(int), defaultdict(int)
    with open(path) as f:
        for r in csv.DictReader(f, delimiter="\t"):
            y = int(str(r["birth_bucket"])[:4])
            born[y] += int(r["born"])
            alive[y] += int(r["alive"])
    years = sorted(born)
    return years, [alive[y] for y in years], [born[y] - alive[y] for y in years]


def draw(years, alive, dead, repo, theme, out):
    t = THEMES[theme]
    fig, ax = plt.subplots(figsize=(8, 3.4))
    bar = dict(width=0.72, edgecolor=t["gap"], linewidth=2)
    ax.bar(years, alive, color=t["alive"], label="alive at HEAD", **bar)
    ax.bar(years, dead, bottom=alive, color=t["dead"], label="deleted since", **bar)
    top = max(a + d for a, d in zip(alive, dead))
    for y, a, d in zip(years, alive, dead):
        # A bar too short to see gets its size, not a share of nothing.
        label = f"{a + d} lines" if a + d < top * 0.03 else f"{100 * a / (a + d):.0f}%"
        ax.text(y, a + d + top * 0.02, label,
                ha="center", va="bottom", color=t["ink"], fontsize=9)
    ax.set_title(f"{repo}: lines written per year, and the share still alive",
                 loc="left", color=t["ink"], fontsize=11)
    ax.set_xticks(years)
    ax.yaxis.set_major_formatter(lambda v, _: f"{v / 1000:.0f}k")
    ax.set_ylim(0, top * 1.12)
    ax.grid(axis="y", color=t["grid"], linewidth=0.6)
    ax.set_axisbelow(True)
    for s in ("top", "right", "left"):
        ax.spines[s].set_visible(False)
    ax.spines["bottom"].set_color(t["grid"])
    ax.tick_params(colors=t["muted"], length=0, labelsize=9)
    ax.legend(loc="upper left", frameon=False, fontsize=9, labelcolor=t["ink"],
              reverse=True)
    fig.tight_layout()
    fig.savefig(out, transparent=True)


if __name__ == "__main__":
    tsv, repo = sys.argv[1], sys.argv[2]
    years, alive, dead = load(tsv)
    for theme in THEMES:
        draw(years, alive, dead, repo, theme, f"docs/survival-{theme}.svg")
