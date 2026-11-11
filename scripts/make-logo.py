#!/usr/bin/env python3
"""Write Loupe's brand mark: the line prism used by the website and the README.

Usage: scripts/make-logo.py [--check] [--preview OUT.png]

The mark is a prism drawn as one outline of a single stroke: a beam enters from the
left and leaves to the right as five thin bands, red to violet. The prism and the
beam use currentColor, so the mark takes the text color of the page it sits on.
This is not the app icon (scripts/make-icon.py draws the glass prism of Loupe.icns).

Files written:
  assets/logo.svg                  the mark, currentColor
  assets/logo-wordmark.svg         the mark and "loupe", currentColor
  assets/logo-wordmark-dark.svg    the wordmark in fixed colors for dark backgrounds
  assets/logo-wordmark-light.svg   the same for light backgrounds (the README picks one)
  site/logo.svg                    the mark, inlined into the website's header
  site/favicon.svg                 the mark with its own colors for light and dark tabs

--check writes nothing and fails if a file differs from what this script writes.
--preview renders the mark and the wordmark at 16, 32, 128, and 512 px on a light
and a dark background into one PNG, with Playwright's Chromium (CHROME, or the
newest one in ~/.cache/ms-playwright).
"""

import glob
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

STROKE = 2
RAY = 1.5
BANDS = ["#ff3b30", "#ffcc00", "#30c84b", "#438bff", "#a855f7"]
INK = {"light": "#16181d", "dark": "#ecedf0"}

APEX, LEFT, RIGHT = (14.5, 3.5), (4, 26.5), (25, 26.5)
BEAM_START = (1, 20.5)


def lerp(a: tuple[float, float], b: tuple[float, float], t: float) -> tuple[float, float]:
    return (round(a[0] + (b[0] - a[0]) * t, 2), round(a[1] + (b[1] - a[1]) * t, 2))


def segment(a: tuple[float, float], b: tuple[float, float], stroke: str, width: float) -> str:
    return f'<path d="M{a[0]:g} {a[1]:g}L{b[0]:g} {b[1]:g}" stroke="{stroke}" stroke-width="{width:g}"/>'


def mark(ink: str) -> str:
    """The prism, its beam, and the bands, in a 32-unit square."""
    beam = segment(BEAM_START, lerp(APEX, LEFT, 0.62), ink, STROKE)
    # The bands leave from points spread along the right face and fan out to the edge.
    bands = "".join(
        segment(lerp(APEX, RIGHT, 0.4 + 0.22 * i / 4), (31, 10 + 14 * i / 4), color, RAY)
        for i, color in enumerate(BANDS)
    )
    prism = (
        f'<path d="M{APEX[0]:g} {APEX[1]:g}L{RIGHT[0]:g} {RIGHT[1]:g}H{LEFT[0]:g}Z" '
        f'stroke="{ink}" stroke-width="{STROKE}"/>'
    )
    return beam + bands + prism


# "loupe" in the same stroke: x-height 13 on the prism's base line, the l as tall as the apex.
BASE, X_TOP, R = 26.5, 13.5, 6.5
MID = BASE - R


def letters(x: float) -> str:
    def o(cx: float) -> str:
        return f"M{cx - R:g} {MID:g}a{R:g} {R:g} 0 1 0 {2 * R:g} 0a{R:g} {R:g} 0 1 0 {-2 * R:g} 0"

    l_ = f"M{x:g} {APEX[1]:g}V{BASE:g}"
    x += 4.5
    o_ = o(x + R)
    x += 2 * R + 4
    u_ = f"M{x:g} {X_TOP:g}V{MID:g}a{R:g} {R:g} 0 0 0 {2 * R:g} 0M{x + 2 * R:g} {X_TOP:g}V{BASE:g}"
    x += 2 * R + 4
    p_ = f"M{x:g} {X_TOP:g}V31" + o(x + R)
    x += 2 * R + 4
    tail = (round(x + R + R * 0.7071, 2), round(MID + R * 0.7071, 2))
    e_ = f"M{x:g} {MID:g}H{x + 2 * R:g}A{R:g} {R:g} 0 1 0 {tail[0]:g} {tail[1]:g}"
    return l_ + o_ + u_ + p_ + e_


WORD_X = 41
WORD_WIDTH = 111


def svg(body: str, width: float, title: str = "Loupe", extra: str = "") -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:g} 32" fill="none" '
        f'stroke-linecap="round" stroke-linejoin="round" role="img" aria-label="{title}">'
        f"{extra}{body}</svg>\n"
    )


def wordmark(ink: str) -> str:
    return mark(ink) + f'<path d="{letters(WORD_X)}" stroke="{ink}" stroke-width="{STROKE}"/>'


FAVICON_STYLE = (
    f"<style>.i{{stroke:{INK['light']}}}"
    f"@media (prefers-color-scheme:dark){{.i{{stroke:{INK['dark']}}}}}</style>"
)


def files() -> dict[Path, str]:
    favicon = mark("currentColor").replace('stroke="currentColor"', 'class="i"')
    return {
        ROOT / "assets/logo.svg": svg(mark("currentColor"), 32),
        ROOT / "assets/logo-wordmark.svg": svg(wordmark("currentColor"), WORD_WIDTH),
        ROOT / "assets/logo-wordmark-dark.svg": svg(wordmark(INK["dark"]), WORD_WIDTH),
        ROOT / "assets/logo-wordmark-light.svg": svg(wordmark(INK["light"]), WORD_WIDTH),
        ROOT / "site/logo.svg": svg(mark("currentColor"), 32),
        ROOT / "site/favicon.svg": svg(favicon, 32, extra=FAVICON_STYLE),
    }


def chrome() -> str:
    found = os.environ.get("CHROME") or next(
        iter(sorted(glob.glob(os.path.expanduser("~/.cache/ms-playwright/chromium-*/chrome-linux64/chrome")), reverse=True)),
        None,
    )
    if not found:
        sys.exit("no Chromium: set CHROME or install Playwright's chromium")
    return found


def preview(out: Path) -> None:
    sizes = [16, 32, 128, 512]
    mark_svg = svg(mark("currentColor"), 32)
    word_svg = svg(wordmark("currentColor"), WORD_WIDTH)
    panels = []
    for scheme, bg in (("light", "#ffffff"), ("dark", "#0e0f11")):
        cells = "".join(
            f'<figure><div style="width:{n}px;height:{n}px">{mark_svg.replace("<svg ", f"<svg width=\"{n}\" height=\"{n}\" ")}</div>'
            f"<figcaption>{n} px</figcaption></figure>"
            for n in sizes
        )
        zoom = "".join(
            f'<figure><canvas data-size="{n}" width="{n * 8}" height="{n * 8}"></canvas><figcaption>{n} px, 8x</figcaption></figure>'
            for n in (16, 32)
        )
        words = "".join(
            f'<figure><div style="height:{h}px">{word_svg.replace("<svg ", f"<svg height=\"{h}\" ")}</div><figcaption>wordmark {h} px</figcaption></figure>'
            for h in (16, 32, 96)
        )
        panels.append(
            f'<section style="background:{bg};color:{INK[scheme]}"><h2>{scheme}</h2>'
            f'<div class="row">{cells}</div><div class="row">{zoom}{words}</div></section>'
        )
    page = f"""<!doctype html><meta charset="utf-8"><style>
body{{margin:0;font:13px/1.4 system-ui,sans-serif}}
section{{padding:24px 28px}} h2{{margin:0 0 12px;font-size:14px;font-weight:600;text-transform:uppercase;letter-spacing:.08em;opacity:.7}}
.row{{display:flex;align-items:flex-end;gap:36px;flex-wrap:wrap;margin-bottom:20px}}
figure{{margin:0;display:flex;flex-direction:column;align-items:center;gap:8px}} figcaption{{opacity:.65}}
svg{{display:block}} canvas{{image-rendering:pixelated}}
</style>{''.join(panels)}
<script>
// Draw the 16 and 32 px renders 8x larger, pixel by pixel, to show what a tab really gets.
for (const c of document.querySelectorAll('canvas')) {{
  const n = +c.dataset.size, color = getComputedStyle(c).color;
  const src = {mark_svg.strip()!r}.replaceAll('currentColor', color);
  const img = new Image(n, n);
  img.onload = () => {{
    const small = document.createElement('canvas'); small.width = small.height = n;
    small.getContext('2d').drawImage(img, 0, 0, n, n);
    const ctx = c.getContext('2d'); ctx.imageSmoothingEnabled = false;
    ctx.drawImage(small, 0, 0, n * 8, n * 8);
  }};
  img.src = 'data:image/svg+xml,' + encodeURIComponent(src);
}}
</script>"""
    with tempfile.TemporaryDirectory() as tmp:
        html = Path(tmp) / "preview.html"
        html.write_text(page)
        subprocess.run(
            [chrome(), "--headless=new", "--no-sandbox", "--hide-scrollbars", "--virtual-time-budget=3000",
             "--window-size=1000,2240", f"--screenshot={out.resolve()}", html.as_uri()],
            check=True, capture_output=True,
        )
    print(f"wrote {out}")


def main() -> None:
    args = sys.argv[1:]
    if args == ["--check"]:
        stale = [p for p, text in files().items() if not p.exists() or p.read_text() != text]
        if stale:
            sys.exit("out of date, run scripts/make-logo.py: " + ", ".join(str(p.relative_to(ROOT)) for p in stale))
        print("logo files are up to date")
        return
    if len(args) == 2 and args[0] == "--preview":
        preview(Path(args[1]))
        return
    if args:
        sys.exit(__doc__)
    for path, text in files().items():
        path.write_text(text)
        print(f"wrote {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
