//! Loupe's look (docs/DESIGN.md): surfaces that get lighter as
//! they come forward, one accent, Inter and JetBrains Mono, on
//! gpui-component's 16 px `rem`. Every color comes from a theme
//! ([`themes`], [`palette`]): a JSON file with a light and a dark palette.
//! [`apply`] picks the palette Settings > Appearance asks for, maps it onto
//! gpui-component's theme so stock components match, and makes it the one
//! [`tokens`] reads, so a change repaints every window at once. [`scale`]
//! holds the sizes. Views use `cx.theme()` first and [`tokens`] second.
//!
//! On macOS the window is frosted glass: the window background is `Blurred`,
//! the theme's `background` is a translucent tint over it (gpui-component's
//! root paints it under everything), the sidebar shows that tint, and the
//! content sits on an inset panel with its own, lighter tint. Other platforms
//! keep every surface opaque.

pub mod palette;
pub mod themes;

#[cfg(test)]
mod fixed_colors;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::component::highlighter::{HighlightTheme, SyntaxColors};
use gpui_kit::component::{Colorize as _, Theme, ThemeMode};
use gpui_kit::{App, Hsla, WindowAppearance, WindowBackgroundAppearance, px, transparent_black};
use serde::{Deserialize, Serialize};

use self::palette::{Contrast, Palette, ThemeFile};
use self::themes::{DEFAULT_THEME, Themes};

/// Whether the window is frosted glass (see the module docs).
pub const GLASS: bool = cfg!(target_os = "macos");

/// The native window background: blurred where the theme paints glass.
pub fn window_background() -> WindowBackgroundAppearance {
    if GLASS {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

/// Which variant of the theme the app shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorScheme {
    /// The system's appearance, followed as it changes.
    System,
    Light,
    #[default]
    Dark,
}

impl ColorScheme {
    pub const ALL: [ColorScheme; 3] = [ColorScheme::System, ColorScheme::Light, ColorScheme::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ColorScheme::System => "System",
            ColorScheme::Light => "Light",
            ColorScheme::Dark => "Dark",
        }
    }

    /// Whether the dark palette shows, given the system's appearance.
    pub fn is_dark(self, system: WindowAppearance) -> bool {
        match self {
            ColorScheme::System => {
                matches!(
                    system,
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                )
            }
            ColorScheme::Light => false,
            ColorScheme::Dark => true,
        }
    }
}

/// How tall list rows and sidebar rows are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    Compact,
    #[default]
    Default,
    Comfortable,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Compact, Density::Default, Density::Comfortable];

    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "Compact",
            Density::Default => "Default",
            Density::Comfortable => "Comfortable",
        }
    }

    /// Points added to every row height at 100% zoom.
    fn row_delta(self) -> f32 {
        match self {
            Density::Compact => -4.,
            Density::Default => 0.,
            Density::Comfortable => 4.,
        }
    }
}

/// The interface typeface, embedded so Loupe looks the same on every
/// machine (`assets::FONTS`).
pub const UI_FONT: &str = "Inter";
/// The typeface of code, diffs, branch names, and SHAs.
pub const CODE_FONT: &str = "JetBrains Mono";

/// Settings > Appearance: the theme and how it is drawn. Saved inside
/// `settings.json`; missing fields take their defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub color_scheme: ColorScheme,
    /// The id of a built-in or user theme ([`themes::Themes`]).
    pub theme: String,
    /// Percent, [`Contrast::MIN`] to [`Contrast::MAX`].
    pub contrast: u16,
    /// How opaque the frosted window is, in percent (macOS only).
    pub glass_opacity: u16,
    pub density: Density,
    /// The interface typeface; `None` is Inter.
    pub ui_font: Option<String>,
    /// The size of body text (list rows, the sidebar, values) in points at
    /// 100% zoom; the other text sizes keep their steps from it.
    pub ui_font_size: u16,
    /// The code typeface; `None` is JetBrains Mono.
    pub mono_font: Option<String>,
    /// The size of code (diffs, logs, branches, SHAs) in points at 100% zoom.
    pub mono_font_size: u16,
}

impl Appearance {
    pub const UI_FONT_SIZES: [u16; 5] = [12, 13, 14, 15, 16];
    pub const MONO_FONT_SIZES: [u16; 6] = [11, 12, 13, 14, 15, 16];
    pub const GLASS_MIN: u16 = 40;
    pub const GLASS_MAX: u16 = 100;

    pub fn ui_font_family(&self) -> &str {
        self.ui_font.as_deref().unwrap_or(UI_FONT)
    }

    pub fn mono_font_family(&self) -> &str {
        self.mono_font.as_deref().unwrap_or(CODE_FONT)
    }
}

impl Default for Appearance {
    fn default() -> Appearance {
        Appearance {
            color_scheme: ColorScheme::default(),
            theme: DEFAULT_THEME.to_owned(),
            contrast: Contrast::DEFAULT,
            glass_opacity: 80,
            density: Density::default(),
            ui_font: None,
            ui_font_size: 13,
            mono_font: None,
            mono_font_size: 12,
        }
    }
}

/// The palette [`tokens`] reads, with the glass opacity it is drawn at.
struct Active {
    palette: Palette,
    dark: bool,
    glass: f32,
}

thread_local! {
    // GPUI builds every frame on the main thread, the one thread that calls
    // `apply`, so the palette needs no lock. Tests each run their own app on
    // their own thread and see only the palette they applied.
    static ACTIVE: RefCell<Rc<Active>> = RefCell::new(Rc::new(Active {
        palette: Themes::default_palette(true),
        dark: true,
        glass: 0.8,
    }));
}

fn active() -> Rc<Active> {
    ACTIVE.with(|active| active.borrow().clone())
}

/// Whether the palette on screen is a dark one.
pub fn is_dark() -> bool {
    active().dark
}

/// Colors the stock theme has no slot for (diff lines, PR states), read from
/// the palette on screen.
pub mod tokens {
    use gpui_kit::Hsla;

    use super::{GLASS, active};

    macro_rules! tokens {
        ($($(#[doc = $doc:literal])* $name:ident => $field:ident,)*) => {
            $($(#[doc = $doc])* pub fn $name() -> Hsla {
                active().palette.$field.0
            })*
        };
    }

    /// The darkest layer: the window, the sidebar, and the tab strip. On
    /// macOS it is a tint over the blurred desktop, as opaque as
    /// Settings > Appearance > Glass opacity says.
    pub fn glass() -> Hsla {
        let active = active();
        active
            .palette
            .window
            .0
            .opacity(if GLASS { active.glass } else { 1.0 })
    }
    /// The content panel, one step forward from the window, a little more
    /// see-through than the window on macOS.
    pub fn panel() -> Hsla {
        let active = active();
        active
            .palette
            .panel
            .0
            .opacity(if GLASS { active.glass * 0.9 } else { 1.0 })
    }

    tokens! {
        /// Cards, inputs, and the segmented track: one step above the panel.
        surface => surface,
        /// Menus, popovers, and dialogs: the top layer, drawn with a shadow.
        overlay => overlay,
        /// Hover and selection washes, translucent so one value works on
        /// every surface.
        glass_hover => hover,
        glass_selected => selected,
        /// The segmented control's thumb: a fill one clear step above the
        /// track.
        segmented_track => segmented_track,
        segmented_thumb => segmented_thumb,
        segmented_thumb_shadow => segmented_thumb_shadow,
        /// The window's color, opaque.
        background => window,
        /// Outlines of controls, cards, the panel, and overlays.
        border => border,
        /// Separators inside one surface.
        border_subtle => border_subtle,
        text => text,
        text_muted => text_muted,
        /// Placeholders, disabled text, and decorative icons: 4.8:1 or more
        /// on every surface.
        text_faint => text_faint,
        /// The one accent: focus, selection marks, links.
        accent => accent,
        /// Text on the primary fill and on a state pill.
        on_accent => on_accent,
        link => link,
        /// The fill behind inline code in prose.
        code_inline => code_inline,
        /// The shade under a list scrolling beneath a pinned row.
        edge_shadow => edge_shadow,
        open => open,
        merged => merged,
        closed => closed,
        draft => draft,
        pending => pending,
        /// A pull request's state pill: the state darkened until white text
        /// on it passes 4.5:1.
        open_fill => open_fill,
        merged_fill => merged_fill,
        closed_fill => closed_fill,
        draft_fill => draft_fill,
        diff_added_bg => diff_added_bg,
        diff_removed_bg => diff_removed_bg,
        diff_added_text => diff_added_text,
        diff_removed_text => diff_removed_text,
        /// The files tree's indent guides, plain and for the selection's
        /// folder.
        indent_guide => indent_guide,
        indent_guide_active => indent_guide_active,
    }
}

/// The theme's base font size at 100%: one `rem`. It is gpui-component's
/// own default, so every scale helper (`px_2`, `h_6`, `size_4`, `text_sm`)
/// lands on a whole pixel of the 4 px grid, and stock components and
/// Loupe's views share one set of sizes.
const BASE_FONT_SIZE: f32 = 16.;

/// The whole-window zoom, like a browser's: a percentage of the 100% layout.
/// Only the values in [`Zoom::STEPS`] exist; a stored value off the steps
/// snaps to the nearest one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u16", into = "u16")]
pub struct Zoom(u16);

impl Zoom {
    /// Percentages Cmd+= and Cmd+- step through.
    pub const STEPS: [u16; 6] = [80, 90, 100, 110, 125, 150];
    pub const ACTUAL_SIZE: Zoom = Zoom(100);

    /// The next step up, or the largest when already there.
    pub fn zoomed_in(self) -> Zoom {
        let next = Self::STEPS.iter().find(|&&step| step > self.0);
        Zoom(*next.unwrap_or(&self.0))
    }

    /// The next step down, or the smallest when already there.
    pub fn zoomed_out(self) -> Zoom {
        let previous = Self::STEPS.iter().rev().find(|&&step| step < self.0);
        Zoom(*previous.unwrap_or(&self.0))
    }

    pub fn percent(self) -> u16 {
        self.0
    }

    pub fn factor(self) -> f32 {
        f32::from(self.0) / 100.
    }
}

impl Default for Zoom {
    fn default() -> Zoom {
        Zoom::ACTUAL_SIZE
    }
}

impl From<u16> for Zoom {
    fn from(percent: u16) -> Zoom {
        let nearest = Self::STEPS
            .iter()
            .min_by_key(|&&step| step.abs_diff(percent))
            .copied()
            .unwrap_or(100);
        Zoom(nearest)
    }
}

impl From<Zoom> for u16 {
    fn from(zoom: Zoom) -> u16 {
        zoom.0
    }
}

/// The current zoom factor as `f32` bits, for [`scale::dp`], which sizing code
/// calls without a context. [`apply_zoom`] is the only writer; it starts at
/// 1.0.
static ZOOM_FACTOR: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// Zooms every window to `zoom`: the theme's font size is the window's `rem`
/// (gpui-component's root sets it each frame), so everything sized in `rem`
/// follows, and [`scale::dp`] scales the fixed sizes by the same factor.
pub fn apply_zoom(zoom: Zoom, cx: &mut App) {
    ZOOM_FACTOR.store(zoom.factor().to_bits(), Ordering::Relaxed);
    Theme::global_mut(cx).font_size = px(BASE_FONT_SIZE * zoom.factor());
    cx.refresh_windows();
}

/// Loupe's type and spacing scale (docs/DESIGN.md), in `rem`
/// so it zooms with the theme's 16 px base. The sizes are the kit's: 12, 14,
/// and 20 px text on a 4 px grid.
pub mod scale {
    use std::sync::atomic::{AtomicU32, Ordering};

    use gpui_kit::{Pixels, Rems, px, rems};

    use super::{Density, ZOOM_FACTOR};

    const BASE: f32 = super::BASE_FONT_SIZE;

    /// Body text in points at 100% zoom, as `f32` bits ([`set_text`]).
    static BODY_SIZE: AtomicU32 = AtomicU32::new(0x4150_0000); // 13.0
    /// Code text in points at 100% zoom, as `f32` bits ([`set_text`]).
    static CODE_SIZE: AtomicU32 = AtomicU32::new(0x4140_0000); // 12.0
    /// Points added to row heights, as `f32` bits ([`set_density`]).
    static ROW_DELTA: AtomicU32 = AtomicU32::new(0);

    fn load(value: &AtomicU32) -> f32 {
        f32::from_bits(value.load(Ordering::Relaxed))
    }

    /// Sets the body and code text sizes (Settings > Appearance >
    /// Typography). The other text sizes keep their steps from the body.
    pub(super) fn set_text(body: u16, code: u16) {
        BODY_SIZE.store(f32::from(body.clamp(10, 20)).to_bits(), Ordering::Relaxed);
        CODE_SIZE.store(f32::from(code.clamp(10, 20)).to_bits(), Ordering::Relaxed);
    }

    /// Sets the row heights (Settings > Appearance > Density).
    pub(super) fn set_density(density: Density) {
        ROW_DELTA.store(density.row_delta().to_bits(), Ordering::Relaxed);
    }

    /// A text size `step` points off the body size, in `rem`.
    fn text(step: f32) -> Rems {
        rems((load(&BODY_SIZE) + step) / BASE)
    }

    /// `size` points at 100% zoom, scaled by the current zoom. For sizes an
    /// API takes as `Pixels` (avatars, icons, fixed rows and columns), so they
    /// grow with the text. Window chrome that lines up with the traffic
    /// lights stays in plain `px`.
    pub fn dp(size: f32) -> Pixels {
        px(size * f32::from_bits(ZOOM_FACTOR.load(Ordering::Relaxed)))
    }

    /// The title of a pull request or an issue, and the sign-in heading:
    /// 18 px at the default text size, the largest text on a page.
    pub fn heading() -> Rems {
        text(5.)
    }
    /// Page header and card titles, 14 px (semibold where it is set).
    pub fn title() -> Rems {
        text(1.)
    }
    /// List rows, the sidebar, and values, 13 px by default: Linear's
    /// density. Prose and stock controls stay on gpui-component's 14 px
    /// `text_sm`.
    pub fn body() -> Rems {
        text(0.)
    }
    /// Author, age, counts, identifiers, 12 px: gpui-component's `text_xs`.
    pub fn meta() -> Rems {
        text(-1.)
    }
    /// Section labels, 12 px (medium, muted, sentence case where it is set).
    pub fn label() -> Rems {
        text(-1.)
    }
    /// Code outside the diff (logs, branches, SHAs, snippets), 12 px by
    /// default; the diff reads the same size from the theme.
    pub fn code() -> Rems {
        rems(load(&CODE_SIZE) / BASE)
    }
    /// The one left and right edge of every bar, list, and body, 16 px.
    pub fn content_inset() -> Rems {
        rems(16. / BASE)
    }
    /// Gap between sections, 24 px.
    pub fn section_gap() -> Rems {
        rems(24. / BASE)
    }
    /// Card padding, 12 px.
    pub fn card_padding() -> Rems {
        rems(12. / BASE)
    }
    /// List rows (pull requests, issues, notifications), 32 px at the
    /// default density.
    pub fn row_height() -> Rems {
        rems((32. + load(&ROW_DELTA)) / BASE)
    }
    /// The toolbar under a page header (tabs, filters), 40 px.
    pub fn toolbar_height() -> Rems {
        rems(40. / BASE)
    }
    /// Side columns (the pull request's reviewers, labels, links), 296 px.
    pub fn side_width() -> Rems {
        rems(296. / BASE)
    }
    /// Sidebar and menu rows, 28 px at the default density.
    pub fn nav_row_height() -> Rems {
        rems((28. + load(&ROW_DELTA)) / BASE)
    }
    /// A fixed row height in `Pixels`: `size` points at the default density,
    /// shifted by the density and scaled by the zoom.
    pub fn row_dp(size: f32) -> Pixels {
        dp(size + load(&ROW_DELTA))
    }
}

/// Loads the embedded fonts and shows the theme Settings asks for (the
/// default theme when Settings is not set yet, as in tests).
pub fn init(cx: &mut App) {
    if let Err(error) = cx.text_system().add_fonts(crate::assets::fonts()) {
        eprintln!("loupe: the embedded fonts did not load: {error}");
    }
    apply(cx);
}

/// The appearance Settings holds, or the default one.
fn appearance(cx: &App) -> Appearance {
    cx.try_global::<crate::store::Settings>()
        .map(|settings| settings.appearance.clone())
        .unwrap_or_default()
}

/// Shows the theme, color scheme, contrast, fonts, and density that
/// Settings > Appearance holds, in every window, without a restart.
pub fn apply(cx: &mut App) {
    let appearance = appearance(cx);
    let themes = Themes::get(cx);
    let entry = themes.resolve(&appearance.theme);
    let dark = appearance.color_scheme.is_dark(cx.window_appearance());
    let palette = entry.file.variant(dark).with_contrast(appearance.contrast);
    let glass = f32::from(
        appearance
            .glass_opacity
            .clamp(Appearance::GLASS_MIN, Appearance::GLASS_MAX),
    ) / 100.;
    ACTIVE.with(|active| {
        *active.borrow_mut() = Rc::new(Active {
            palette: palette.clone(),
            dark,
            glass,
        })
    });
    scale::set_text(appearance.ui_font_size, appearance.mono_font_size);
    scale::set_density(appearance.density);

    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
    let zoom = cx
        .try_global::<crate::store::Settings>()
        .map(|settings| settings.zoom)
        .unwrap_or_default();
    Theme::update(cx, |theme| {
        theme.font_family = appearance.ui_font_family().to_owned().into();
        theme.mono_font_family = appearance.mono_font_family().to_owned().into();
        theme.mono_font_size = px(f32::from(appearance.mono_font_size));
        theme.radius = px(6.);
        theme.radius_lg = px(8.);
        theme.font_size = px(BASE_FONT_SIZE * zoom.factor());
        // Overlays (menus, popovers, dialogs) cast a shadow; surfaces in the
        // plane of the window do not.
        theme.shadow = true;
        map_colors(theme, &palette, dark);
        theme.highlight_theme = highlight_theme(&theme.highlight_theme, &palette);
    });
}

/// Maps a palette onto gpui-component's color slots.
fn map_colors(theme: &mut Theme, palette: &Palette, dark: bool) {
    let clear = transparent_black();
    let hover = tokens::glass_hover();
    let selected = tokens::glass_selected();
    let colors = &mut theme.colors;
    colors.background = tokens::glass();
    colors.foreground = palette.text.0;
    colors.border = palette.border.0;
    colors.muted = palette.surface.0;
    colors.muted_foreground = palette.text_muted.0;
    // gpui-component's `accent` is the hover wash of ghost buttons and menu
    // items, not Loupe's accent color.
    colors.accent = palette.ghost_hover.0;
    colors.accent_foreground = palette.text.0;
    colors.popover = palette.overlay.0;
    colors.overlay = palette.scrim.0;
    colors.popover_foreground = palette.text.0;
    colors.primary = palette.accent_fill.0;
    colors.primary_hover = palette.accent_fill_hover.0;
    colors.primary_active = palette.accent_fill_active.0;
    colors.primary_foreground = palette.on_accent.0;
    colors.group_box = palette.surface.0;
    colors.group_box_foreground = palette.text.0;
    colors.secondary = palette.secondary.0;
    colors.secondary_hover = palette.secondary_hover.0;
    colors.secondary_active = palette.secondary_active.0;
    colors.secondary_foreground = palette.text.0;
    // An input's outline stands out from the border of the card it sits
    // on; Settings also fills its fields with the window color.
    colors.input = palette.input_border.0;
    colors.ring = palette.accent.0;
    colors.caret = palette.accent.0;
    colors.selection = palette.accent.0.opacity(0.3);
    // State colors: the diff, Tags, checks and state buttons read these, so
    // they carry the same green and red as the pull request states.
    let on_bright = palette.on_bright.0;
    let on_accent = palette.on_accent.0;
    let shade = |mut color: Hsla, factor: f32| {
        color.l *= factor;
        color
    };
    let open = palette.open.0;
    (colors.success, colors.success_hover, colors.success_active) =
        (open, shade(open, 0.9), shade(open, 0.8));
    colors.success_foreground = on_bright;
    let closed = palette.closed.0;
    (colors.danger, colors.danger_hover, colors.danger_active) =
        (closed, shade(closed, 0.9), shade(closed, 0.8));
    colors.danger_foreground = on_accent;
    let pending = palette.pending.0;
    (colors.warning, colors.warning_hover, colors.warning_active) =
        (pending, shade(pending, 0.9), shade(pending, 0.8));
    colors.warning_foreground = on_bright;
    let link = palette.link.0;
    (colors.info, colors.info_hover, colors.info_active) =
        (link, shade(link, 0.9), shade(link, 0.8));
    colors.info_foreground = on_bright;
    colors.link = link;
    colors.link_hover = link;
    colors.link_active = link;
    // Chrome and list backgrounds stay clear so the window and the content
    // panel show through instead of a second tint.
    colors.sidebar = clear;
    colors.sidebar_foreground = palette.text.0;
    colors.sidebar_border = palette.border.0;
    colors.sidebar_accent = selected;
    colors.sidebar_accent_foreground = palette.text.0;
    colors.list = clear;
    colors.list_hover = hover;
    colors.list_active = selected;
    colors.list_even = clear;
    colors.table = clear;
    colors.table_hover = hover;
    colors.table_active = selected;
    colors.tab_bar = clear;
    colors.tab = clear;
    colors.tab_active = clear;
    // The segmented control's track: its selected segment is drawn in
    // `background`, so the track must differ from it to show the selection.
    colors.tab_bar_segmented = palette.segmented_track.0;
    colors.slider_bar = palette.accent_fill.0;
    colors.slider_thumb = palette.on_accent.0;
    // Progress (files viewed) is neutral: color only reports a state.
    colors.progress_bar = palette.text.0;
    colors.drag_border = palette.accent.0;
    colors.drop_target = palette.accent.0.opacity(0.25);
    colors.list_active_border = palette.accent.0;
    colors.window_border = palette.border.0;
    colors.tab_foreground = palette.text_muted.0;
    colors.tab_active_foreground = palette.text.0;
    colors.title_bar = clear;
    colors.title_bar_border = palette.border.0;
    colors.skeleton = palette.skeleton.0;
    colors.scrollbar = clear;
    colors.scrollbar_thumb = palette.scrollbar_thumb.0;
    colors.scrollbar_thumb_hover = palette.scrollbar_thumb_hover.0;
    colors.button_primary = colors.primary;
    colors.button_primary_hover = colors.primary_hover;
    colors.button_primary_active = colors.primary_active;
    colors.button_primary_foreground = colors.primary_foreground;
    // The merge button's fill is deeper than the state green, so white text
    // on it passes 4.5:1.
    colors.button_success = palette.merge_fill.0;
    colors.button_success_hover = palette.merge_fill_hover.0;
    colors.button_success_active = palette.merge_fill_active.0;
    colors.button_success_foreground = on_accent;
    // A filled destructive button uses the deep red of the closed pill, so
    // its white label passes 4.5:1 like the other fills.
    let closed_fill = palette.closed_fill.0;
    colors.button_danger = closed_fill;
    colors.button_danger_hover = shade(closed_fill, 0.92);
    colors.button_danger_active = shade(closed_fill, 0.84);
    colors.button_danger_foreground = on_accent;
    // Default and outline buttons, and the switch, derive from the palette
    // the way gpui-component derives them from its own theme file.
    colors.button = if dark {
        palette.input_border.0.mix_oklab(clear, 0.3)
    } else {
        palette.surface.0
    };
    colors.button_foreground = palette.text.0;
    colors.button_hover = palette.input_border.0.mix_oklab(clear, 0.5);
    colors.button_active = palette.input_border.0.mix_oklab(clear, 0.7);
    colors.switch = palette.secondary_active.0;
    colors.switch_thumb = palette.window.0;
    colors.list_head = palette.surface.0;
    colors.table_head = palette.surface.0;
    colors.table_head_foreground = palette.text_muted.0;
    colors.accordion = palette.surface.0;
}

/// Syntax colors for the diff, review threads, and markdown code blocks:
/// the palette's captures over the stock theme of the same mode. Captures
/// the palette leaves out keep the stock style.
fn highlight_theme(base: &HighlightTheme, palette: &Palette) -> Arc<HighlightTheme> {
    let mut syntax = serde_json::to_value(&base.style.syntax).unwrap_or_default();
    if let Some(target) = syntax.as_object_mut() {
        for (key, style) in &palette.syntax {
            let mut value = serde_json::json!({ "color": style.color.hex() });
            if style.italic {
                value["font_style"] = "italic".into();
            }
            if style.bold {
                value["font_weight"] = 700.into();
            }
            target.insert(key.clone(), value);
        }
    }
    let mut theme = base.clone();
    theme.name = "Loupe".into();
    if let Ok(syntax) = serde_json::from_value::<SyntaxColors>(syntax) {
        theme.style.syntax = syntax;
    }
    Arc::new(theme)
}

/// The theme the app shows now, for previews that draw the other variant.
pub fn current_theme(cx: &App) -> ThemeFile {
    Themes::get(cx).resolve(&appearance(cx).theme).file.clone()
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::highlighter::HighlightTheme;
    use gpui_kit::{HighlightStyle, Hsla, WindowAppearance};

    use super::palette::Color;
    use super::themes::Themes;
    use super::{ColorScheme, Zoom, highlight_theme};

    #[test]
    fn zoom_steps_clamp_at_both_ends() {
        let mut zoom = Zoom::ACTUAL_SIZE;
        let mut seen = vec![zoom.percent()];
        for _ in 0..10 {
            zoom = zoom.zoomed_in();
            seen.push(zoom.percent());
        }
        assert_eq!(&seen[..5], &[100, 110, 125, 150, 150]);
        for _ in 0..10 {
            zoom = zoom.zoomed_out();
        }
        assert_eq!(zoom.percent(), 80);
        assert_eq!(zoom.zoomed_out().percent(), 80);
        assert_eq!(Zoom::from(80).zoomed_in().percent(), 90);
    }

    #[test]
    fn stored_zoom_snaps_to_a_step() {
        let zoom: Zoom = serde_json::from_str("130").expect("a number");
        assert_eq!(zoom.percent(), 125);
        assert_eq!(serde_json::to_string(&Zoom::from(400)).unwrap(), "150");
        assert_eq!(Zoom::from(0).percent(), 80);
    }

    #[test]
    fn the_color_scheme_follows_the_system_only_when_asked() {
        for system in [WindowAppearance::Light, WindowAppearance::VibrantLight] {
            assert!(!ColorScheme::System.is_dark(system));
            assert!(ColorScheme::Dark.is_dark(system));
        }
        for system in [WindowAppearance::Dark, WindowAppearance::VibrantDark] {
            assert!(ColorScheme::System.is_dark(system));
            assert!(!ColorScheme::Light.is_dark(system));
        }
    }

    #[test]
    fn syntax_captures_of_the_palette_reach_the_highlight_theme() {
        let palette = Themes::default_palette(true);
        let theme = highlight_theme(&HighlightTheme::default_dark(), &palette);
        let color = |style: Option<gpui_kit::component::highlighter::ThemeStyle>| {
            style
                .map(HighlightStyle::from)
                .and_then(|style| style.color)
        };
        let keyword: Hsla = Color::parse("#ff7b72").unwrap().0;
        assert_eq!(color(theme.style.syntax.keyword), Some(keyword));
        let string: Hsla = Color::parse("#a5d6ff").unwrap().0;
        assert_eq!(color(theme.style.syntax.string), Some(string));
        // A capture the palette leaves out keeps the stock style.
        assert_eq!(
            theme.style.syntax.punctuation,
            HighlightTheme::default_dark().style.syntax.punctuation
        );

        let light = Themes::default_palette(false);
        let theme = highlight_theme(&HighlightTheme::default_light(), &light);
        assert_eq!(
            color(theme.style.syntax.keyword),
            Some(Color::parse("#cf222e").unwrap().0)
        );
    }
}
