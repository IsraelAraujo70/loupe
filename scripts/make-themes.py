#!/usr/bin/env python3
"""Writes Loupe's built-in themes to crates/loupe-app/themes/*.json.

Each theme has a light and a dark palette with every token the app reads
(see `Palette` in crates/loupe-app/src/theme/palette.rs). Loupe's dark palette
is written out by hand: it is the look the app had before themes. The other
palettes are derived from a few seeds (a hue and chroma for the surfaces, an
accent) in OKLCH, then nudged until they pass the contrast rules of
docs/DESIGN.md:

- faint, muted, and primary text reach 4.8:1 on every opaque surface;
- white on the primary fill, the merge fill, and the state pills reaches 4.5:1;
- links reach 4.5:1 on the panel and on cards.

Run it after changing a seed: `python3 scripts/make-themes.py`. The Rust test
`theme::palette::tests::built_in_themes_pass_the_contrast_rules` checks the
same rules on the files the app embeds.
"""

import json
import math
import pathlib

OUT = pathlib.Path(__file__).resolve().parent.parent / "crates/loupe-app/themes"

# --- color math -------------------------------------------------------------


def srgb_to_linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def linear_to_srgb(c):
    c = max(0.0, min(1.0, c))
    return 12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def hex_to_rgb(value):
    value = value.lstrip("#")
    return tuple(int(value[i : i + 2], 16) / 255 for i in (0, 2, 4))


def rgb_to_hex(rgb, alpha=None):
    text = "#" + "".join(f"{round(max(0, min(1, c)) * 255):02x}" for c in rgb)
    if alpha is not None:
        text += f"{round(alpha * 255):02x}"
    return text


def oklch_to_rgb(l, c, h):
    a = c * math.cos(math.radians(h))
    b = c * math.sin(math.radians(h))
    l_ = l + 0.3963377774 * a + 0.2158037573 * b
    m_ = l - 0.1055613458 * a - 0.0638541728 * b
    s_ = l - 0.0894841775 * a - 1.2914855480 * b
    l3, m3, s3 = l_**3, m_**3, s_**3
    r = 4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3
    g = -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3
    bl = -0.0041960863 * l3 - 0.7034186147 * m3 + 1.7076147010 * s3
    return tuple(linear_to_srgb(x) for x in (r, g, bl))


def rgb_to_oklch(rgb):
    r, g, b = (srgb_to_linear(x) for x in rgb)
    l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b
    m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b
    s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b
    l_, m_, s_ = (x ** (1 / 3) for x in (l, m, s))
    L = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_
    A = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_
    B = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_
    return L, math.hypot(A, B), math.degrees(math.atan2(B, A)) % 360


def oklch(l, c, h):
    return rgb_to_hex(oklch_to_rgb(l, c, h))


def luminance(hex_value):
    r, g, b = (srgb_to_linear(x) for x in hex_to_rgb(hex_value))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a, b):
    la, lb = sorted((luminance(a[:7]), luminance(b[:7])), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def shift_until(hex_value, against, ratio, step):
    """Moves `hex_value` in OKLCH lightness by `step` until it reaches
    `ratio` against every color in `against`."""
    l, c, h = rgb_to_oklch(hex_to_rgb(hex_value))
    value = hex_value
    while min(contrast(value, other) for other in against) < ratio:
        l += step
        if not 0 <= l <= 1:
            raise SystemExit(f"cannot reach {ratio}:1 from {hex_value}")
        value = oklch(l, c, h)
    return value


def shade(hex_value, factor):
    l, c, h = rgb_to_oklch(hex_to_rgb(hex_value))
    return oklch(l * factor, c, h)


def white(alpha):
    return rgb_to_hex((1, 1, 1), alpha)


def black(alpha):
    return rgb_to_hex((0, 0, 0), alpha)


def tinted(hex_value, alpha):
    return hex_value[:7] + f"{round(alpha * 255):02x}"


# --- syntax -----------------------------------------------------------------

GITHUB_DARK_SYNTAX = {
    "keyword": "#ff7b72",
    "operator": "#ff7b72",
    "string": "#a5d6ff",
    "string.special": "#a5d6ff",
    "string.special.symbol": "#a5d6ff",
    "string.escape": "#79c0ff",
    "string.regex": "#7ee787",
    "comment": "#8b949e",
    "comment.doc": "#8b949e",
    "function": "#d2a8ff",
    "constructor": "#ffa657",
    "type": "#ffa657",
    "enum": "#ffa657",
    "variant": "#79c0ff",
    "constant": "#79c0ff",
    "number": "#79c0ff",
    "boolean": "#79c0ff",
    "property": "#79c0ff",
    "attribute": "#79c0ff",
    "variable.special": "#ffa657",
    "label": "#79c0ff",
    "tag": "#7ee787",
    "title": "#79c0ff",
    "link_uri": "#a5d6ff",
    "text.literal": "#a5d6ff",
    "punctuation.special": "#ff7b72",
}

GITHUB_LIGHT_SYNTAX = {
    "keyword": "#cf222e",
    "operator": "#cf222e",
    "string": "#0a3069",
    "string.special": "#0a3069",
    "string.special.symbol": "#0a3069",
    "string.escape": "#0550ae",
    "string.regex": "#116329",
    "comment": "#59636e",
    "comment.doc": "#59636e",
    "function": "#8250df",
    "constructor": "#953800",
    "type": "#953800",
    "enum": "#953800",
    "variant": "#0550ae",
    "constant": "#0550ae",
    "number": "#0550ae",
    "boolean": "#0550ae",
    "property": "#0550ae",
    "attribute": "#0550ae",
    "variable.special": "#953800",
    "label": "#0550ae",
    "tag": "#116329",
    "title": "#0550ae",
    "link_uri": "#0a3069",
    "text.literal": "#0a3069",
    "punctuation.special": "#cf222e",
}


def syntax_styles(colors):
    styles = {}
    for key, color in colors.items():
        style = {"color": color}
        if key.startswith("comment"):
            style["italic"] = True
        if key == "title":
            style["bold"] = True
        styles[key] = style
    return styles


# --- palettes ---------------------------------------------------------------

DARK_STATES = {
    "open": "#3fb950",
    "merged": "#a371f7",
    "closed": "#f85149",
    "draft": "#8b949e",
    "pending": "#d29922",
    "open_fill": "#238636",
    "merged_fill": "#8250df",
    "closed_fill": "#cf222e",
    "draft_fill": "#57606a",
    "merge_fill": "#238636",
    "merge_fill_hover": "#1f7a30",
    "merge_fill_active": "#1a6b2a",
    "diff_added_bg": "#3fb95024",
    "diff_removed_bg": "#f8514924",
    "diff_added_text": "#7ee787",
    "diff_removed_text": "#ffa198",
}

LIGHT_STATES = {
    "open": "#1a7f37",
    "merged": "#8250df",
    "closed": "#cf222e",
    "draft": "#59636e",
    "pending": "#9a6700",
    "open_fill": "#1a7f37",
    "merged_fill": "#8250df",
    "closed_fill": "#cf222e",
    "draft_fill": "#59636e",
    "merge_fill": "#1f883d",
    "merge_fill_hover": "#1a7f37",
    "merge_fill_active": "#116329",
    "diff_added_bg": "#2da44e26",
    "diff_removed_bg": "#ff818233",
    "diff_added_text": "#116329",
    "diff_removed_text": "#a40e26",
}

# Loupe's dark palette as the app drew it before themes existed.
LOUPE_DARK = {
    "window": "#0e0f11",
    "panel": "#141518",
    "surface": "#1b1c20",
    "overlay": "#212227",
    "scrim": "#0000008c",
    "hover": "#ffffff0a",
    "selected": "#ffffff14",
    "ghost_hover": "#ffffff12",
    "segmented_track": "#1b1c20",
    "segmented_thumb": "#ffffff2e",
    "segmented_thumb_shadow": "#00000059",
    "border": "#ffffff1c",
    "border_subtle": "#ffffff0f",
    "input_border": "#ffffff33",
    "text": "#ecedf0",
    "text_muted": "#a6a9b2",
    "text_faint": "#8a8e99",
    "accent": "#438bff",
    "accent_fill": "#2f6fe4",
    "accent_fill_hover": "#2863d0",
    "accent_fill_active": "#2257ba",
    "on_accent": "#ffffff",
    "on_bright": "#0e0f11",
    "link": "#6ba4ff",
    "code_inline": "#ffffff1c",
    "secondary": "#222328",
    "secondary_hover": "#2a2b31",
    "secondary_active": "#303138",
    "skeleton": "#ffffff0f",
    "scrollbar_thumb": "#ffffff26",
    "scrollbar_thumb_hover": "#ffffff40",
    "edge_shadow": "#00000059",
    "indent_guide": "#ffffff14",
    "indent_guide_active": "#ffffff38",
    **DARK_STATES,
}


def accent_set(accent, surfaces, on_accent="#ffffff"):
    fill = shift_until(accent, [on_accent], 4.6, -0.005)
    return {
        "accent": accent,
        "accent_fill": fill,
        "accent_fill_hover": shade(fill, 0.93),
        "accent_fill_active": shade(fill, 0.86),
        "on_accent": on_accent,
    }


def dark_palette(hue, chroma, accent, overrides=None):
    surfaces = {
        "window": oklch(0.165, chroma, hue),
        "panel": oklch(0.195, chroma, hue),
        "surface": oklch(0.225, chroma * 1.1, hue),
        "overlay": oklch(0.25, chroma * 1.1, hue),
    }
    opaque = list(surfaces.values())
    text = oklch(0.94, min(chroma, 0.01), hue)
    muted = shift_until(oklch(0.74, chroma * 1.2, hue), opaque, 6.0, 0.005)
    faint = shift_until(oklch(0.64, chroma * 1.2, hue), opaque, 4.8, 0.005)
    accent_l, accent_c, accent_h = rgb_to_oklch(hex_to_rgb(accent))
    link = shift_until(oklch(accent_l + 0.06, accent_c, accent_h), opaque, 4.8, 0.005)
    palette = {
        **surfaces,
        "scrim": black(0.55),
        "hover": white(0.04),
        "selected": white(0.08),
        "ghost_hover": white(0.07),
        "segmented_track": surfaces["surface"],
        "segmented_thumb": white(0.18),
        "segmented_thumb_shadow": black(0.35),
        "border": white(0.11),
        "border_subtle": white(0.06),
        "input_border": white(0.2),
        "text": text,
        "text_muted": muted,
        "text_faint": faint,
        **accent_set(accent, opaque),
        "on_bright": surfaces["window"],
        "link": link,
        "code_inline": white(0.11),
        "secondary": oklch(0.255, chroma * 1.1, hue),
        "secondary_hover": oklch(0.285, chroma * 1.1, hue),
        "secondary_active": oklch(0.31, chroma * 1.1, hue),
        "skeleton": white(0.06),
        "scrollbar_thumb": white(0.15),
        "scrollbar_thumb_hover": white(0.25),
        "edge_shadow": black(0.35),
        "indent_guide": white(0.08),
        "indent_guide_active": white(0.22),
        **DARK_STATES,
    }
    palette.update(overrides or {})
    return palette


def light_palette(hue, chroma, accent, overrides=None):
    surfaces = {
        "window": oklch(0.955, chroma, hue),
        "panel": oklch(0.985, chroma * 0.5, hue),
        "surface": "#ffffff",
        "overlay": "#ffffff",
    }
    opaque = list(surfaces.values())
    text = oklch(0.23, min(chroma * 2, 0.02), hue)
    muted = shift_until(oklch(0.48, chroma * 2, hue), opaque, 6.0, -0.005)
    faint = shift_until(oklch(0.56, chroma * 2, hue), opaque, 4.8, -0.005)
    link = shift_until(accent, opaque, 4.8, -0.005)
    palette = {
        **surfaces,
        "scrim": black(0.35),
        "hover": black(0.04),
        "selected": black(0.07),
        "ghost_hover": black(0.06),
        "segmented_track": surfaces["window"],
        "segmented_thumb": "#ffffff",
        "segmented_thumb_shadow": black(0.18),
        "border": black(0.12),
        "border_subtle": black(0.07),
        "input_border": black(0.2),
        "text": text,
        "text_muted": muted,
        "text_faint": faint,
        **accent_set(accent, opaque),
        "on_bright": "#ffffff",
        "link": link,
        "code_inline": black(0.06),
        "secondary": oklch(0.94, chroma, hue),
        "secondary_hover": oklch(0.915, chroma, hue),
        "secondary_active": oklch(0.89, chroma, hue),
        "skeleton": black(0.06),
        "scrollbar_thumb": black(0.18),
        "scrollbar_thumb_hover": black(0.3),
        "edge_shadow": black(0.12),
        "indent_guide": black(0.08),
        "indent_guide_active": black(0.22),
        **LIGHT_STATES,
    }
    palette.update(overrides or {})
    return palette


TOKYO_DARK_SYNTAX = {
    **GITHUB_DARK_SYNTAX,
    "keyword": "#bb9af7",
    "operator": "#89ddff",
    "string": "#9ece6a",
    "string.special": "#9ece6a",
    "string.special.symbol": "#9ece6a",
    "string.escape": "#89ddff",
    "string.regex": "#b4f9f8",
    "comment": "#7a83ad",
    "comment.doc": "#7a83ad",
    "function": "#7aa2f7",
    "constructor": "#2ac3de",
    "type": "#2ac3de",
    "enum": "#2ac3de",
    "variant": "#ff9e64",
    "constant": "#ff9e64",
    "number": "#ff9e64",
    "boolean": "#ff9e64",
    "property": "#73daca",
    "attribute": "#e0af68",
    "variable.special": "#f7768e",
    "label": "#7aa2f7",
    "tag": "#f7768e",
    "title": "#7aa2f7",
    "link_uri": "#73daca",
    "text.literal": "#9ece6a",
    "punctuation.special": "#89ddff",
}

TOKYO_LIGHT_SYNTAX = {
    **GITHUB_LIGHT_SYNTAX,
    "keyword": "#7847bd",
    "operator": "#006a83",
    "string": "#485e30",
    "string.special": "#485e30",
    "string.special.symbol": "#485e30",
    "string.escape": "#006a83",
    "string.regex": "#166775",
    "comment": "#5f6488",
    "comment.doc": "#5f6488",
    "function": "#2959aa",
    "constructor": "#006c86",
    "type": "#006c86",
    "enum": "#006c86",
    "variant": "#965027",
    "constant": "#965027",
    "number": "#965027",
    "boolean": "#965027",
    "property": "#33635c",
    "attribute": "#8f5e15",
    "variable.special": "#8c4351",
    "label": "#2959aa",
    "tag": "#8c4351",
    "title": "#2959aa",
    "link_uri": "#33635c",
    "text.literal": "#485e30",
    "punctuation.special": "#006a83",
}


def themes():
    loupe_light = light_palette(264, 0.004, "#2f6fe4")
    yield "loupe", "Loupe", (loupe_light, GITHUB_LIGHT_SYNTAX), (LOUPE_DARK, GITHUB_DARK_SYNTAX)
    seeds = [
        ("grove", "Grove", 160, 0.012, "#2f9e6e", "#3fb47e"),
        ("ocean", "Ocean", 230, 0.016, "#1f7fc4", "#3d9be0"),
        ("ember", "Ember", 45, 0.014, "#c45a1f", "#e07a3f"),
        ("iris", "Iris", 290, 0.016, "#6f4fe0", "#8b6cf0"),
    ]
    for theme_id, name, hue, chroma, light_accent, dark_accent in seeds:
        yield (
            theme_id,
            name,
            (light_palette(hue, chroma, light_accent), GITHUB_LIGHT_SYNTAX),
            (dark_palette(hue, chroma, dark_accent), GITHUB_DARK_SYNTAX),
        )
    tokyo_dark = dark_palette(
        277,
        0.03,
        "#7aa2f7",
        {
            "window": "#16161e",
            "panel": "#1a1b26",
            "surface": "#1f2335",
            "overlay": "#24283b",
            "text": "#c0caf5",
            "on_bright": "#16161e",
            "open": "#9ece6a",
            "merged": "#bb9af7",
            "closed": "#f7768e",
            "pending": "#e0af68",
            "diff_added_text": "#9ece6a",
            "diff_removed_text": "#f7768e",
        },
    )
    tokyo_dark["text_muted"] = shift_until("#a9b1d6", [tokyo_dark[k] for k in ("window", "panel", "surface", "overlay")], 6.0, 0.005)
    tokyo_dark["text_faint"] = shift_until("#737aa2", [tokyo_dark[k] for k in ("window", "panel", "surface", "overlay")], 4.8, 0.005)
    tokyo_light = light_palette(
        270,
        0.01,
        "#2e7de9",
        {
            "window": "#d5d6db",
            "panel": "#e1e2e7",
            "surface": "#e9e9ed",
            "overlay": "#f2f2f5",
            "text": "#343b58",
            "segmented_track": "#d5d6db",
            "segmented_thumb": "#f2f2f5",
        },
    )
    opaque = [tokyo_light[k] for k in ("window", "panel", "surface", "overlay")]
    tokyo_light["text_muted"] = shift_until("#4c505e", opaque, 6.0, -0.005)
    tokyo_light["text_faint"] = shift_until("#6c6f85", opaque, 4.8, -0.005)
    tokyo_light["link"] = shift_until("#2e7de9", opaque, 4.8, -0.005)
    tokyo_light["accent_fill"] = shift_until("#2e7de9", ["#ffffff"], 4.6, -0.005)
    tokyo_light["accent_fill_hover"] = shade(tokyo_light["accent_fill"], 0.93)
    tokyo_light["accent_fill_active"] = shade(tokyo_light["accent_fill"], 0.86)
    tokyo_light["secondary"] = "#d0d1d8"
    tokyo_light["secondary_hover"] = "#c6c8d1"
    tokyo_light["secondary_active"] = "#bcbfca"
    yield "tokyo-night", "Tokyo Night", (tokyo_light, TOKYO_LIGHT_SYNTAX), (tokyo_dark, TOKYO_DARK_SYNTAX)


def check(theme_id, variant, palette):
    opaque = [palette[k] for k in ("window", "panel", "surface", "overlay")]
    for key, ratio in (("text", 7.0), ("text_muted", 4.8), ("text_faint", 4.8)):
        worst = min(contrast(palette[key], s) for s in opaque)
        assert worst >= ratio, f"{theme_id}/{variant}: {key} {worst:.2f}"
    for key in ("panel", "surface"):
        worst = contrast(palette["link"], palette[key])
        assert worst >= 4.5, f"{theme_id}/{variant}: link on {key} {worst:.2f}"
    for key in ("accent_fill", "merge_fill", "open_fill", "merged_fill", "closed_fill", "draft_fill"):
        worst = contrast(palette["on_accent"], palette[key])
        assert worst >= 4.5, f"{theme_id}/{variant}: on_accent on {key} {worst:.2f}"


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for theme_id, name, (light, light_syntax), (dark, dark_syntax) in themes():
        for variant, palette in (("light", light), ("dark", dark)):
            check(theme_id, variant, palette)
        document = {
            "name": name,
            "light": {**light, "syntax": syntax_styles(light_syntax)},
            "dark": {**dark, "syntax": syntax_styles(dark_syntax)},
        }
        path = OUT / f"{theme_id}.json"
        path.write_text(json.dumps(document, indent=2) + "\n")
        print(f"wrote {path.relative_to(OUT.parent.parent.parent)}")


if __name__ == "__main__":
    main()
