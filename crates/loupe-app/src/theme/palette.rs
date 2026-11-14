//! A theme as data: the colors of one variant ([`Palette`]), a theme file
//! with a light and a dark variant ([`ThemeFile`]), and the hex strings they
//! are written in. Every color the app paints comes from a palette; the
//! built-in ones are `crates/loupe-app/themes/*.json`, written by
//! `scripts/make-themes.py`.

use std::collections::BTreeMap;
use std::fmt;

use gpui_kit::{Hsla, Rgba};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::store::TabGroupColor;

/// A color written as `#rrggbb` or `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub Hsla);

impl Color {
    /// Parses `#rrggbb` or `#rrggbbaa`, in either case.
    pub fn parse(text: &str) -> Option<Color> {
        let digits = text.trim().strip_prefix('#')?;
        if !matches!(digits.len(), 6 | 8) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(digits, 16).ok()?;
        let (rgb, alpha) = if digits.len() == 6 {
            (value, 0xff)
        } else {
            (value >> 8, value & 0xff)
        };
        let channel = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.;
        Some(Color(Hsla::from(Rgba {
            r: channel(16),
            g: channel(8),
            b: channel(0),
            a: alpha as f32 / 255.,
        })))
    }

    /// `#rrggbb`, with the alpha appended when the color is not opaque.
    pub fn hex(self) -> String {
        let rgba = Rgba::from(self.0);
        let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        let mut text = format!(
            "#{:02x}{:02x}{:02x}",
            byte(rgba.r),
            byte(rgba.g),
            byte(rgba.b)
        );
        if byte(rgba.a) != 0xff {
            text.push_str(&format!("{:02x}", byte(rgba.a)));
        }
        text
    }
}

/// A color GitHub or Linear sends as data (a label, a state, a project):
/// `rrggbb` with or without its `#`. These are the one kind of color that
/// does not come from the theme.
pub fn data_color(text: &str) -> Option<Hsla> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 {
        return None;
    }
    Color::parse(&format!("#{digits}")).map(|color| color.0)
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Color::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!("\"{text}\" is not a #rrggbb or #rrggbbaa color"))
        })
    }
}

/// The style of one syntax capture (`keyword`, `string`, …) in diffs and
/// code blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyntaxStyle {
    pub color: Color,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
}

/// Declares [`Palette`] once with every token's documentation, and the list
/// of its tokens the theme editor walks.
macro_rules! palette {
    ($($(#[doc = $doc:literal])* $group:literal $field:ident,)*) => {
        /// Every color one variant of a theme paints with. A theme file must
        /// name them all; a missing one is an error that names it.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct Palette {
            $($(#[doc = $doc])* pub $field: Color,)*
            /// Syntax captures for diffs and code blocks. Captures it leaves
            /// out keep gpui-component's stock style.
            #[serde(default)]
            pub syntax: BTreeMap<String, SyntaxStyle>,
        }

        /// A token of [`Palette`]: its key in the file, the group the theme
        /// editor lists it under, and how to read and write it.
        pub struct Token {
            pub key: &'static str,
            pub group: &'static str,
            pub get: fn(&Palette) -> Color,
            pub set: fn(&mut Palette, Color),
        }

        /// Every token of [`Palette`], in file order.
        pub const TOKENS: &[Token] = &[
            $(Token {
                key: stringify!($field),
                group: $group,
                get: |palette| palette.$field,
                set: |palette, color| palette.$field = color,
            },)*
        ];
    };
}

palette! {
    /// The window, the sidebar, and the tab strip: the darkest layer in a
    /// dark theme. On macOS it is a tint over the blurred desktop.
    "Surfaces" window,
    /// The content panel, one step forward from the window.
    "Surfaces" panel,
    /// Cards, inputs, and the segmented track.
    "Surfaces" surface,
    /// Menus, popovers, and dialogs, drawn with a shadow.
    "Surfaces" overlay,
    /// What a modal dialog dims the window with.
    "Surfaces" scrim,
    /// The hover wash of rows. Translucent, so it works on every surface.
    "Surfaces" hover,
    /// The selection wash of rows.
    "Surfaces" selected,
    /// The hover wash of ghost buttons and menu items.
    "Surfaces" ghost_hover,
    /// The track of a segmented control, and its selected option.
    "Surfaces" segmented_track,
    "Surfaces" segmented_thumb,
    "Surfaces" segmented_thumb_shadow,
    /// Outlines of controls, cards, the panel, and overlays.
    "Borders" border,
    /// Separators inside one surface.
    "Borders" border_subtle,
    /// The outline of an input, which stands out from its card's border.
    "Borders" input_border,
    "Text" text,
    "Text" text_muted,
    /// Placeholders, disabled text, and decorative icons.
    "Text" text_faint,
    /// The one accent: focus rings, the unread dot, selection marks.
    "Accent" accent,
    /// The primary button's fill.
    "Accent" accent_fill,
    "Accent" accent_fill_hover,
    "Accent" accent_fill_active,
    /// Text on the primary, merge, and state fills.
    "Accent" on_accent,
    /// Text on the success, warning, and info fills.
    "Accent" on_bright,
    "Accent" link,
    /// The fill behind inline code in prose.
    "Accent" code_inline,
    /// Secondary buttons.
    "Controls" secondary,
    "Controls" secondary_hover,
    "Controls" secondary_active,
    "Controls" skeleton,
    "Controls" scrollbar_thumb,
    "Controls" scrollbar_thumb_hover,
    /// The shade under a list that scrolls beneath a pinned row.
    "Controls" edge_shadow,
    /// The files tree's indent guides, plain and for the selection's folder.
    "Controls" indent_guide,
    "Controls" indent_guide_active,
    "States" open,
    "States" merged,
    "States" closed,
    "States" draft,
    "States" pending,
    /// The fills of a pull request's state pill.
    "States" open_fill,
    "States" merged_fill,
    "States" closed_fill,
    "States" draft_fill,
    /// The merge button.
    "States" merge_fill,
    "States" merge_fill_hover,
    "States" merge_fill_active,
    "Diff" diff_added_bg,
    "Diff" diff_removed_bg,
    "Diff" diff_added_text,
    "Diff" diff_removed_text,
    /// The tab group colors that are not a state color: Grey, Red, Yellow,
    /// Green, and Purple are the draft, closed, pending, open, and merged
    /// fills ([`Palette::tab_group`]). White text on each passes 4.5:1.
    "Tab groups" group_blue,
    "Tab groups" group_pink,
    "Tab groups" group_cyan,
    "Tab groups" group_orange,
}

impl Palette {
    /// The fill of a tab group's name and line.
    pub fn tab_group(&self, color: TabGroupColor) -> Color {
        match color {
            TabGroupColor::Grey => self.draft_fill,
            TabGroupColor::Blue => self.group_blue,
            TabGroupColor::Red => self.closed_fill,
            TabGroupColor::Yellow => self.pending,
            TabGroupColor::Green => self.open_fill,
            TabGroupColor::Purple => self.merged_fill,
            TabGroupColor::Pink => self.group_pink,
            TabGroupColor::Cyan => self.group_cyan,
            TabGroupColor::Orange => self.group_orange,
        }
    }

    /// The palette with `percent` contrast, 100 being the theme's own. Above
    /// 100, borders and washes grow stronger and muted and faint text move
    /// toward the primary text; below, borders and washes soften while text
    /// keeps the theme's contrast, so it never gets harder to read.
    pub fn with_contrast(&self, percent: u16) -> Palette {
        let mut palette = self.clone();
        let factor = f32::from(percent.clamp(Contrast::MIN, Contrast::MAX)) / 100.;
        if (factor - 1.).abs() < f32::EPSILON {
            return palette;
        }
        let strengthen = |color: Color| {
            let alpha = color.0.a;
            if alpha >= 1. {
                return color;
            }
            Color(Hsla {
                a: (alpha * factor).clamp(0., 1.),
                ..color.0
            })
        };
        palette.border = strengthen(palette.border);
        palette.border_subtle = strengthen(palette.border_subtle);
        palette.input_border = strengthen(palette.input_border);
        palette.hover = strengthen(palette.hover);
        palette.selected = strengthen(palette.selected);
        palette.indent_guide = strengthen(palette.indent_guide);
        if factor > 1. {
            let toward = (factor - 1.).min(1.);
            palette.text_muted = mix(palette.text_muted, palette.text, toward * 0.6);
            palette.text_faint = mix(palette.text_faint, palette.text_muted, toward);
        }
        palette
    }
}

/// The Contrast setting's range, in percent.
pub struct Contrast;

impl Contrast {
    pub const MIN: u16 = 50;
    pub const MAX: u16 = 150;
    pub const DEFAULT: u16 = 100;
}

/// `from` moved `amount` (0 to 1) of the way to `to`, in RGB.
fn mix(from: Color, to: Color, amount: f32) -> Color {
    let (a, b) = (Rgba::from(from.0), Rgba::from(to.0));
    let lerp = |x: f32, y: f32| x + (y - x) * amount;
    Color(Hsla::from(Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }))
}

/// A theme: a name and a palette for each color scheme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    pub name: String,
    pub light: Palette,
    pub dark: Palette,
}

impl ThemeFile {
    /// Parses a theme file, with an error that says what is wrong.
    pub fn parse(json: &str) -> Result<ThemeFile, String> {
        let file: ThemeFile = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if file.name.trim().is_empty() {
            return Err("the theme has no name".into());
        }
        Ok(file)
    }

    /// Parses a user's theme file. A token the file does not name (one
    /// added to Loupe after the file was written) takes the value `fallback`
    /// has in the same variant, so older files keep loading; a missing
    /// variant, an unknown key, or a bad color is still an error.
    pub fn parse_user(json: &str, fallback: &ThemeFile) -> Result<ThemeFile, String> {
        let mut value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        for variant in ["light", "dark"] {
            let Some(given) = value.get_mut(variant).and_then(|v| v.as_object_mut()) else {
                continue;
            };
            let defaults = serde_json::to_value(fallback.variant(variant == "dark"))
                .map_err(|error| error.to_string())?;
            if let Some(defaults) = defaults.as_object() {
                for token in TOKENS {
                    if !given.contains_key(token.key)
                        && let Some(default) = defaults.get(token.key)
                    {
                        given.insert(token.key.to_owned(), default.clone());
                    }
                }
            }
        }
        Self::parse(&value.to_string())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    pub fn variant(&self, dark: bool) -> &Palette {
        if dark { &self.dark } else { &self.light }
    }

    pub fn variant_mut(&mut self, dark: bool) -> &mut Palette {
        if dark {
            &mut self.dark
        } else {
            &mut self.light
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Hsla, Rgba};

    use super::{Color, Contrast, TOKENS, ThemeFile};
    use crate::store::TabGroupColor;
    use crate::theme::themes::BUILT_IN;

    /// WCAG contrast ratio between two opaque colors.
    fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
        fn luminance(color: Hsla) -> f32 {
            let rgba = Rgba::from(color);
            let linear = |c: f32| {
                if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
        }
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn colors_round_trip_through_hex() {
        for text in ["#0e0f11", "#ffffff1c", "#2F6FE4", "#00000000"] {
            let color = Color::parse(text).expect(text);
            assert_eq!(color.hex(), text.to_lowercase());
        }
        assert_eq!(Color::parse("#2f6fe4ff").unwrap().hex(), "#2f6fe4");
        for bad in ["", "#", "#fff", "2f6fe4", "#2f6fe", "#gggggg", "#2f6fe4f"] {
            assert!(Color::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_theme_missing_a_token_names_it() {
        let loupe = ThemeFile::parse(BUILT_IN[0].1).expect("Loupe parses");
        let mut json: serde_json::Value =
            serde_json::from_str(BUILT_IN[1].1).expect("Grove parses");
        json["dark"].as_object_mut().unwrap().remove("text_faint");
        let error = ThemeFile::parse(&json.to_string()).unwrap_err();
        assert!(error.contains("text_faint"), "{error}");

        // A user's file written before a token existed still loads: the
        // token takes the fallback's value for that variant.
        let file = ThemeFile::parse_user(&json.to_string(), &loupe).expect("fills the token");
        assert_eq!(file.dark.text_faint, loupe.dark.text_faint);
        let grove = ThemeFile::parse(BUILT_IN[1].1).unwrap();
        assert_eq!(
            file.dark.accent, grove.dark.accent,
            "the others are its own"
        );
        json.as_object_mut().unwrap().remove("light");
        let error = ThemeFile::parse_user(&json.to_string(), &loupe).unwrap_err();
        assert!(
            error.contains("light"),
            "a missing palette is an error: {error}"
        );
        json["light"] = json["dark"].clone();
        json["light"]["sparkle"] = "#ffffff".into();
        assert!(ThemeFile::parse_user(&json.to_string(), &loupe).is_err());

        json["dark"]["text_faint"] = "blue".into();
        let error = ThemeFile::parse(&json.to_string()).unwrap_err();
        assert!(error.contains("blue"), "{error}");
    }

    #[test]
    fn the_token_list_covers_every_color_of_the_palette() {
        let file = ThemeFile::parse(BUILT_IN[0].1).expect("Loupe parses");
        let json = serde_json::to_value(&file.dark).unwrap();
        let colors = json.as_object().unwrap().len() - 1; // without `syntax`
        assert_eq!(TOKENS.len(), colors);
        let mut palette = file.dark.clone();
        for token in TOKENS {
            let color = Color::parse("#123456").unwrap();
            (token.set)(&mut palette, color);
            assert_eq!((token.get)(&palette), color, "{}", token.key);
        }
    }

    /// docs/DESIGN.md: faint text is 4.8:1 or more on every surface, and
    /// white text on the primary fill passes 4.5:1; GitHub's state pills
    /// and the merge button keep white text at 4.5:1 too.
    #[test]
    fn built_in_themes_pass_the_contrast_rules() {
        for (id, json) in BUILT_IN {
            let file = ThemeFile::parse(json).unwrap_or_else(|error| panic!("{id}: {error}"));
            for (variant, palette) in [("light", &file.light), ("dark", &file.dark)] {
                let surfaces = [
                    palette.window,
                    palette.panel,
                    palette.surface,
                    palette.overlay,
                ];
                for (name, text, ratio) in [
                    ("text", palette.text, 7.0),
                    ("text_muted", palette.text_muted, 4.8),
                    ("text_faint", palette.text_faint, 4.8),
                ] {
                    for surface in surfaces {
                        let found = contrast_ratio(text.0, surface.0);
                        assert!(
                            found >= ratio,
                            "{id}/{variant}: {name} on {surface} is {found:.2}:1"
                        );
                    }
                }
                for surface in [palette.panel, palette.surface] {
                    let found = contrast_ratio(palette.link.0, surface.0);
                    assert!(found >= 4.5, "{id}/{variant}: link on {surface} {found:.2}");
                }
                for (name, fill) in [
                    ("accent_fill", palette.accent_fill),
                    ("accent_fill_hover", palette.accent_fill_hover),
                    ("accent_fill_active", palette.accent_fill_active),
                    ("merge_fill", palette.merge_fill),
                    ("merge_fill_hover", palette.merge_fill_hover),
                    ("merge_fill_active", palette.merge_fill_active),
                    ("open_fill", palette.open_fill),
                    ("merged_fill", palette.merged_fill),
                    ("closed_fill", palette.closed_fill),
                    ("draft_fill", palette.draft_fill),
                    ("group_blue", palette.group_blue),
                ] {
                    let found = contrast_ratio(palette.on_accent.0, fill.0);
                    assert!(
                        found >= 4.5,
                        "{id}/{variant}: on_accent on {name} {found:.2}"
                    );
                }
            }
        }
    }

    /// A color in OKLab, where distance follows how different two colors
    /// look.
    fn oklab(color: Hsla) -> [f32; 3] {
        let rgba = Rgba::from(color);
        let linear = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(rgba.r), linear(rgba.g), linear(rgba.b));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }

    /// Issue #65: every theme names Pink, Cyan, and Orange in both variants
    /// (a user's file may leave them out; a built-in one may not), white
    /// text on them reads at 4.5:1 like the state pills, and each looks
    /// apart from every other group color of the same palette.
    #[test]
    fn every_theme_gives_the_new_group_colors_their_own_tone() {
        let new = [
            (TabGroupColor::Pink, "group_pink"),
            (TabGroupColor::Cyan, "group_cyan"),
            (TabGroupColor::Orange, "group_orange"),
        ];
        for (id, json) in BUILT_IN {
            let raw: serde_json::Value = serde_json::from_str(json).expect("a theme is JSON");
            let file = ThemeFile::parse(json).unwrap_or_else(|error| panic!("{id}: {error}"));
            for (variant, palette) in [("light", &file.light), ("dark", &file.dark)] {
                for (color, key) in new {
                    assert!(
                        raw[variant].get(key).is_some(),
                        "{id}/{variant} has no {key}"
                    );
                    let fill = palette.tab_group(color);
                    let found = contrast_ratio(palette.on_accent.0, fill.0);
                    assert!(
                        found >= 4.5,
                        "{id}/{variant}: on_accent on {key} {found:.2}"
                    );
                    for other in TabGroupColor::ALL {
                        if other == color {
                            continue;
                        }
                        let [a, b] = [fill, palette.tab_group(other)].map(|c| oklab(c.0));
                        let apart = a
                            .iter()
                            .zip(b)
                            .map(|(x, y)| (x - y).powi(2))
                            .sum::<f32>()
                            .sqrt();
                        assert!(
                            apart >= 0.08,
                            "{id}/{variant}: {} looks like {} ({apart:.3} apart)",
                            color.label(),
                            other.label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn contrast_strengthens_borders_and_never_weakens_text() {
        let file = ThemeFile::parse(BUILT_IN[0].1).unwrap();
        let base = &file.dark;
        let high = base.with_contrast(Contrast::MAX);
        let low = base.with_contrast(Contrast::MIN);
        assert!(high.border.0.a > base.border.0.a);
        assert!(low.border.0.a < base.border.0.a);
        let on_panel = |color: Color| contrast_ratio(color.0, base.panel.0);
        assert!(on_panel(high.text_faint) > on_panel(base.text_faint));
        assert_eq!(low.text_faint, base.text_faint);
        assert_eq!(low.text_muted, base.text_muted);
        assert_eq!(base.with_contrast(Contrast::DEFAULT), *base);
    }
}
