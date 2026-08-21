"""Render demo/chart.png: a static "lines"-style preview of a coinfetch run.

`lines` in coinfetch is a pixel image over the terminal grid, and VHS cannot
capture that (its ttyd pipeline does not render kitty/sixel), so the hero
image on the README is a still, drawn from the same cached CoinGecko data
the CLI would use.

Run once from the repo root:  python3 demo/render_hero.py
"""

import json
import pathlib
from datetime import datetime, timezone

import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter, MaxNLocator

CACHE = pathlib.Path.home() / ".cache" / "coinfetch"
OUT = pathlib.Path(__file__).parent / "chart.png"

BG        = "#1e1e2e"
FG        = "#cdd6f4"
MUTED     = "#585b70"
GRID      = "#313244"
COIN_COLS = {
    "bitcoin":  "#94e2d5",
    "ethereum": "#f5c2e7",
    "solana":   "#f9e2af",
}

def load(coin):
    p = CACHE / f"chart_{coin}_7d.json"
    return json.loads(p.read_text())["payload"]["prices"]

def normalise(prices, t0):
    inside = [(t, v) for t, v in prices if t >= t0]
    base = inside[0][1]
    return [(t, (v / base - 1) * 100) for t, v in inside]

def main():
    raw = {c: load(c) for c in COIN_COLS}
    t0 = max(series[0][0] for series in raw.values())
    series = {c: normalise(v, t0) for c, v in raw.items()}
    latest = {c: v[-1][1] for c, v in series.items()}
    prices = {c: raw[c][-1][1] for c in COIN_COLS}

    fig, ax = plt.subplots(figsize=(12, 6.2), dpi=120)
    fig.patch.set_facecolor(BG)
    ax.set_facecolor(BG)

    for coin, points in series.items():
        xs = [datetime.fromtimestamp(t / 1000, tz=timezone.utc) for t, _ in points]
        ys = [y for _, y in points]
        ax.plot(xs, ys, color=COIN_COLS[coin], linewidth=1.6, antialiased=True)

    for spine in ax.spines.values():
        spine.set_color(MUTED)
    ax.tick_params(colors=FG, labelsize=10)
    ax.grid(True, color=GRID, linewidth=0.5, alpha=0.6)
    ax.yaxis.set_major_formatter(FuncFormatter(lambda v, _: f"{v:+.1f}%"))
    ax.xaxis.set_major_locator(MaxNLocator(6))
    ax.xaxis.set_major_formatter(plt.matplotlib.dates.DateFormatter("%m-%d"))
    ax.set_title("7d  % change since day 1", color=FG, loc="left",
                 fontsize=11, pad=12, family="monospace")

    legend_lines = [
        f"● {c:<10}  ${prices[c]:>10,.2f}   {latest[c]:+.2f}%"
        for c in COIN_COLS
    ]
    for i, (coin, line) in enumerate(zip(COIN_COLS, legend_lines)):
        fig.text(
            0.08, 0.13 - i * 0.032, line,
            color=COIN_COLS[coin], family="monospace", fontsize=10,
        )

    plt.subplots_adjust(left=0.08, right=0.97, top=0.92, bottom=0.30)
    fig.savefig(OUT, facecolor=BG, dpi=120)
    print(f"wrote {OUT}")

if __name__ == "__main__":
    main()
