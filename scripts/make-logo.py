#!/usr/bin/env python3
"""Write Loupe's brand mark: a triangular prism seen in depth, its back face lit like a diff.

Usage: scripts/make-logo.py [--check] [--preview OUT.png]

Construction. Two equilateral triangles (apex 60 deg) of the same size, drawn as rings of one width with
mitred corners: the front face, and the back face moved straight right by DEPTH. The back face shows only
where it clears the front one by GAP, which leaves its right side and a short foot of its base. The front
face is currentColor, so it takes the text colour of the page; the back face runs from GitHub's deletion
red at the top to its addition green at the bottom, through a short orange (a straight red-to-green
blend turns brown): a change, seen through the prism. Everything is a filled polygon (no strokes, masks, or filters).

The wordmark is "loupe" in Outfit Medium (SIL Open Font License 1.1), outlined, tracked -10/1000 em. Its
x-height is 0.38 of the mark's height, its baseline sits on the bottom of the prism, and the gap between
mark and word is 0.3 of the mark's height. The favicon is the small-size cut: wider rings, a narrower gap.

This is not the app icon (scripts/make-icon.py draws the glass prism of Loupe.icns).

Files written:
  assets/logo.svg                  the mark, currentColor
  assets/logo-wordmark.svg         the mark and "loupe", currentColor
  assets/logo-wordmark-dark.svg    the wordmark in fixed colours for dark backgrounds
  assets/logo-wordmark-light.svg   the same for light backgrounds (the README picks one)
  site/logo.svg                    the wordmark, inlined into the website's header and footer
  site/favicon.svg                 the small-size cut, with its own ink for light and dark tabs

--check writes nothing and fails if a file differs from what this script writes.
--preview renders the mark at 16, 32, 64, 128, and 512 px and the wordmark, on a light and a dark
background, into one PNG, with Playwright's Chromium (CHROME, or the newest one in ~/.cache/ms-playwright).
"""

import glob
import math
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

SIDE = 170  # each face's side
RING = 20  # the width of each face's ring
DEPTH = 40  # how far right the back face sits
GAP = 9  # clear space around the front face where the back face does not show
DIFF = [(0, "#ff6b5e"), (0.35, "#f85149"), (0.5, "#f0883e"), (0.62, "#56d364"), (1, "#2ea043")]
INK = {"light": "#16181d", "dark": "#ecedf0"}

Point = tuple[float, float]
Line = tuple[Point, Point]


def num(v: float) -> str:
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def triangle(side: float, dx: float = 0.0) -> list[Point]:
    """Equilateral, apex up, centroid at (dx, 0); clockwise on screen: apex, right corner, left corner."""
    r = side / math.sqrt(3)
    return [(dx + r * math.cos(math.radians(a)), r * math.sin(math.radians(a))) for a in (-90, 30, 150)]


def cross(a: Point, b: Point, p: Point) -> float:
    return (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])


def meet(a: Line, b: Line) -> Point:
    (x1, y1), (x2, y2) = a
    (x3, y3), (x4, y4) = b
    den = (x1 - x2) * (y3 - y4) - (y1 - y2) * (x3 - x4)
    t = ((x1 - x3) * (y3 - y4) - (y1 - y3) * (x3 - x4)) / den
    return (x1 + t * (x2 - x1), y1 + t * (y2 - y1))


def inset(pts: list[Point], d: float) -> list[Point]:
    """The convex clockwise polygon with every edge moved inward by d (outward for negative d)."""
    lines = []
    for i, a in enumerate(pts):
        b = pts[(i + 1) % len(pts)]
        n = math.hypot(b[0] - a[0], b[1] - a[1])
        nx, ny = -(b[1] - a[1]) / n * d, (b[0] - a[0]) / n * d
        lines.append(((a[0] + nx, a[1] + ny), (b[0] + nx, b[1] + ny)))
    return [meet(lines[i - 1], lines[i]) for i in range(len(lines))]


def clip(pts: list[Point], edge: Line, keep_sign: float) -> list[Point]:
    """Sutherland-Hodgman: the part of the polygon on the side of `edge` where cross() has keep_sign."""
    out = []
    for i, p in enumerate(pts):
        q = pts[(i + 1) % len(pts)]
        p_in, q_in = cross(*edge, p) * keep_sign > 0, cross(*edge, q) * keep_sign > 0
        if p_in:
            out.append(p)
        if p_in != q_in:
            out.append(meet((p, q), edge))
    return out


class Mark:
    """The front face, the visible part of the back face, and the box their ink covers."""

    def __init__(self, side: float, ring: float, depth: float, gap: float):
        front = triangle(side)
        back = triangle(side, depth)
        self.front = [front, inset(front, ring)]
        # The back face shows only outside the front face grown by the gap. With the back face moved
        # straight right, that is the half-plane beyond the grown front's right side.
        grown = inset(front, -gap)
        edge = (grown[0], grown[1])
        outside = -math.copysign(1, cross(*edge, (0, 0)))
        self.back = [clip(back, edge, outside), clip(inset(back, ring), edge, outside)]
        self.gradient = (back[0][1], back[1][1])  # top to bottom of the back face
        pts = front + self.back[0]
        self.box = (min(p[0] for p in pts), min(p[1] for p in pts), max(p[0] for p in pts), max(p[1] for p in pts))

    @property
    def size(self) -> tuple[float, float]:
        return self.box[2] - self.box[0], self.box[3] - self.box[1]

    def draw(self, ink: str, origin: Point = (0, 0), scale: float = 1.0, gradient_id: str = "diff",
             ink_attr: str | None = None) -> str:
        """SVG elements with the ink box's top-left corner at `origin`, scaled by `scale`."""
        def at(p: Point) -> str:
            return f"{num(origin[0] + (p[0] - self.box[0]) * scale)} {num(origin[1] + (p[1] - self.box[1]) * scale)}"

        def path(rings: list[list[Point]]) -> str:
            return "".join("M" + "L".join(at(p) for p in r) + "Z" for r in rings)

        y0, y1 = (num(origin[1] + (y - self.box[1]) * scale) for y in self.gradient)
        stops = "".join(f'<stop offset="{num(o)}" stop-color="{c}"/>' for o, c in DIFF)
        paint = ink_attr or f'fill="{ink}"'
        return (
            f'<defs><linearGradient id="{gradient_id}" gradientUnits="userSpaceOnUse" x1="0" y1="{y0}" x2="0" y2="{y1}">'
            f"{stops}</linearGradient></defs>"
            f'<path d="{path(self.back)}" fill="url(#{gradient_id})" fill-rule="evenodd"/>'
            f'<path d="{path(self.front)}" {paint} fill-rule="evenodd"/>'
        )


MARK = Mark(SIDE, RING, DEPTH, GAP)
FAVICON = Mark(SIDE, 30, DEPTH, 12)

# "loupe" in Outfit Medium, outlined: font units (1000 to the em), baseline at y = 0, y downward.
# Glyph ink spans x 61..2359 with the tracking applied; the l rises to -720, the p descends to 200.
WORD = (
    "M61 0L61 -720L171 -720L171 0L61 0ZM503 10Q433 10 376 -23.5Q319 -57 285.5 -114Q252 -171 252 -241Q252 -311 285.5 -367"
    "Q319 -423 376 -456.5Q433 -490 503 -490Q574 -490 631 -457Q688 -424 721.5 -367.5Q755 -311 755 -241Q755 -171 721.5 -114"
    "Q688 -57 631 -23.5Q574 10 503 10ZM503 -96Q544 -96 575.5 -114.5Q607 -133 624.5 -166Q642 -199 642 -241Q642 -283 624 -315"
    "Q606 -347 575 -365.5Q544 -384 503 -384Q463 -384 431.5 -365.5Q400 -347 382.5 -315Q365 -283 365 -241Q365 -199 382.5 -166"
    "Q400 -133 431.5 -114.5Q463 -96 503 -96ZM1033 10Q972 10 924.5 -17Q877 -44 850.5 -91.5Q824 -139 824 -201L824 -480L934 -480"
    "L934 -206Q934 -171 945.5 -146.5Q957 -122 979.5 -109Q1002 -96 1033 -96Q1081 -96 1106.5 -124.5Q1132 -153 1132 -206L1132 -480"
    "L1242 -480L1242 -201Q1242 -138 1215.5 -90.5Q1189 -43 1142 -16.5Q1095 10 1033 10ZM1600 10Q1545 10 1502 -13.5Q1459 -37 1434.5 -78"
    "Q1410 -119 1410 -171L1410 -309Q1410 -361 1435 -402Q1460 -443 1503 -466.5Q1546 -490 1600 -490Q1665 -490 1717.5 -456.5"
    "Q1770 -423 1800.5 -366.5Q1831 -310 1831 -239Q1831 -169 1800.5 -112.5Q1770 -56 1717.5 -23Q1665 10 1600 10ZM1343 200L1343 -480"
    "L1453 -480L1453 -351L1434 -234L1453 -118L1453 200L1343 200ZM1582 -94Q1622 -94 1652.5 -112.5Q1683 -131 1700.5 -164"
    "Q1718 -197 1718 -240Q1718 -283 1700.5 -316Q1683 -349 1652.5 -367.5Q1622 -386 1582 -386Q1542 -386 1511 -367.5Q1480 -349 1463 -316"
    "Q1446 -283 1446 -240Q1446 -197 1463 -164Q1480 -131 1511 -112.5Q1542 -94 1582 -94ZM2136 10Q2064 10 2006.5 -22.5Q1949 -55 1915.5 -112"
    "Q1882 -169 1882 -240Q1882 -311 1915 -367.5Q1948 -424 2004.5 -457Q2061 -490 2130 -490Q2197 -490 2248.5 -459Q2300 -428 2329.5 -374"
    "Q2359 -320 2359 -251Q2359 -239 2357.5 -226.5Q2356 -214 2353 -199L1959 -199L1959 -289L2296 -289L2255 -253Q2253 -297 2238 -327"
    "Q2223 -357 2195.5 -373Q2168 -389 2128 -389Q2086 -389 2055 -371Q2024 -353 2007 -320.5Q1990 -288 1990 -243Q1990 -198 2008 -164"
    "Q2026 -130 2059 -111.5Q2092 -93 2135 -93Q2172 -93 2203.5 -106Q2235 -119 2257 -144L2327 -73Q2292 -32 2242 -11Q2192 10 2136 10Z"
)
WORD_LEFT, WORD_RIGHT, WORD_DESCENT, X_HEIGHT = 61, 2359, 200, 480


def place(d: str, scale: float, dx: float, dy: float) -> str:
    """Scale absolute path data (every number belongs to an x, y pair) and move it by (dx, dy)."""
    nums = iter(range(10**9))
    return re.sub(
        r"-?\d+(?:\.\d+)?",
        lambda m: num(float(m.group()) * scale + (dx if next(nums) % 2 == 0 else dy)),
        d,
    )


def svg(width: float, height: float, body: str, extra: str = "") -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {num(width)} {num(height)}" '
        f'role="img" aria-label="Loupe">{extra}{body}</svg>\n'
    )


def mark_svg(ink: str) -> str:
    w, h = MARK.size
    return svg(w, h, MARK.draw(ink))


def wordmark_svg(ink: str) -> str:
    w, h = MARK.size
    k = 0.38 * h / X_HEIGHT
    left = w + 0.3 * h
    word = place(WORD, k, left - WORD_LEFT * k, h)
    return svg(left + (WORD_RIGHT - WORD_LEFT) * k, h + WORD_DESCENT * k,
               MARK.draw(ink) + f'<path d="{word}" fill="{ink}"/>')


def favicon_svg() -> str:
    """The small-size cut, centred in a 256 box with 8 units of padding."""
    w, h = FAVICON.size
    scale = 240 / max(w, h)
    origin = ((256 - w * scale) / 2, (256 - h * scale) / 2)
    style = f"<style>.i{{fill:{INK['light']}}}@media (prefers-color-scheme:dark){{.i{{fill:{INK['dark']}}}}}</style>"
    return svg(256, 256, FAVICON.draw("", origin, scale, ink_attr='class="i"'), extra=style)


def files() -> dict[Path, str]:
    return {
        ROOT / "assets/logo.svg": mark_svg("currentColor"),
        ROOT / "assets/logo-wordmark.svg": wordmark_svg("currentColor"),
        ROOT / "assets/logo-wordmark-dark.svg": wordmark_svg(INK["dark"]),
        ROOT / "assets/logo-wordmark-light.svg": wordmark_svg(INK["light"]),
        ROOT / "site/logo.svg": wordmark_svg("currentColor"),
        ROOT / "site/favicon.svg": favicon_svg(),
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
    sizes = [512, 128, 64, 32, 16]
    mark = mark_svg("currentColor").strip()
    word = wordmark_svg("currentColor").strip()
    favicon = favicon_svg().strip()
    w, h = MARK.size
    panels = []
    for scheme, bg in (("light", "#ffffff"), ("dark", "#0e0f11")):
        cells = "".join(
            f'<figure>{mark.replace("<svg ", f"<svg width=\"{n}\" height=\"{n * h / w:.1f}\" ")}<figcaption>{n} px</figcaption></figure>'
            for n in sizes
        )
        zoom = "".join(
            f'<figure><canvas data-size="{n}" data-scheme="{scheme}" width="{n * 8}" height="{n * 8}"></canvas>'
            f"<figcaption>favicon, {n} px at 8x</figcaption></figure>"
            for n in (16, 32)
        )
        words = "".join(
            f'<figure>{word.replace("<svg ", f"<svg height=\"{px}\" ")}<figcaption>wordmark {px} px</figcaption></figure>'
            for px in (24, 40, 96)
        )
        panels.append(
            f'<section style="background:{bg};color:{INK[scheme]}"><h2>{scheme}</h2>'
            f'<div class="row">{cells}</div><div class="row">{zoom}{words}</div></section>'
        )
    page = f"""<!doctype html><meta charset="utf-8"><style>
body{{margin:0;font:13px/1.4 system-ui,sans-serif}}
section{{padding:24px 28px}} h2{{margin:0 0 12px;font-size:14px;font-weight:600;text-transform:uppercase;letter-spacing:.08em;opacity:.7}}
.row{{display:flex;align-items:flex-end;gap:36px;flex-wrap:wrap;margin-bottom:22px}}
figure{{margin:0;display:flex;flex-direction:column;align-items:center;gap:8px}} figcaption{{opacity:.65}}
svg{{display:block}} canvas{{image-rendering:pixelated}}
</style>{''.join(panels)}
<script>
// Rasterise the favicon at its true size, then draw it 8x larger pixel by pixel: what a tab really gets.
const favicon = {favicon!r};
for (const c of document.querySelectorAll('canvas')) {{
  const n = +c.dataset.size;
  const src = c.dataset.scheme === 'dark' ? favicon.replace('<style>', '<style>.i{{fill:{INK["dark"]}!important}}') : favicon;
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
             "--window-size=1200,1780", f"--screenshot={out.resolve()}", html.as_uri()],
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
