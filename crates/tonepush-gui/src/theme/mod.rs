//! TonePush's visual system, as the 2026-10-01 redesign specifies it
//! (`docs/design/redesign-2026-10-01`, whose `app.css` is the source of every
//! value here).
//!
//! Every colour is a named token read from the current palette each frame,
//! so switching between the dark and light themes recolours everything at
//! once. One typeface, Inter as fastframe-fonts bundles it, at four weights
//! and with its tabular figures frozen into the character map, so a reading
//! keeps its width while a knob turns. One category palette for HX and PRO
//! tones alike, and the pedal's own LED colours for what lights under a foot.
//!
//! [`widgets`] holds the components (buttons, segmented controls, chips,
//! tags, dialogs, banners, menus, knobs and switches) and [`paint`] the
//! drawing they share. [`legacy`] is the editor's earlier widget set, drawn
//! in the same tokens until the pages that use it are redrawn.

// The redesigned pages adopt these components stage by stage; until the last
// of them lands, some are defined before their first use.
#![allow(dead_code)]

use std::cell::Cell;

use egui::{Color32, Stroke, Vec2};

pub mod legacy;
pub mod paint;
pub mod widgets;

pub use legacy::*;
pub use widgets::*;

// ---------------------------------------------------------------------------
// Palettes

/// Every colour the interface draws with, by role.
#[derive(Debug)]
pub struct Palette {
    pub dark: bool,
    /// The chain board and field wells.
    pub bg_deep: Color32,
    /// The window, the sidebar and pages.
    pub bg: Color32,
    /// Cards, tiles and faceplates.
    pub panel: Color32,
    /// Controls at rest.
    pub raised: Color32,
    /// Hover, selected rows, the active segment.
    pub hover: Color32,
    pub pressed: Color32,
    /// A bypassed tile.
    pub tile_off: Color32,
    /// Hairlines inside a surface.
    pub line_soft: Color32,
    /// Panel edges and dividers.
    pub line: Color32,
    /// Control outlines.
    pub line_strong: Color32,
    /// The signal.
    pub wire: Color32,
    /// The board's dots.
    pub dot: Color32,
    pub text: Color32,
    pub text_soft: Color32,
    pub muted: Color32,
    pub faint: Color32,
    /// Your next action, and nothing else.
    pub accent: Color32,
    pub accent_bright: Color32,
    /// Text and icons on the amber gradient.
    pub accent_ink: Color32,
    pub accent_soft: Color32,
    pub accent_line: Color32,
    /// Edited and not saved; a difference.
    pub hot: Color32,
    pub hot_soft: Color32,
    /// Connected, protected, verified.
    pub ok: Color32,
    pub ok_soft: Color32,
    /// Destructive actions and failures.
    pub danger: Color32,
    pub danger_soft: Color32,
    /// Neutral notes.
    pub info: Color32,
    pub info_soft: Color32,
    /// A footswitch LED that is not lit.
    pub led_off: Color32,
    /// What dims the window behind a dialog.
    pub scrim: Color32,
    /// The one shadow, under floating layers.
    pub shadow: Color32,
    /// A knob's face, top and bottom of its gradient, and its edge.
    pub knob_top: Color32,
    pub knob_bottom: Color32,
    pub knob_edge: Color32,
    /// The drop shadow under a knob's face.
    pub knob_shadow: Color32,
    /// A footswitch's stomp button, centre and rim of its gradient, and edge.
    pub switch_top: Color32,
    pub switch_bottom: Color32,
    pub switch_edge: Color32,
    /// The category palette, in [`Category`] order.
    pub categories: [Color32; Category::COUNT],
}

const fn hex(value: u32) -> Color32 {
    Color32::from_rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// A colour with an alpha, as CSS writes `rgba(r, g, b, a)`.
const fn hexa(value: u32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
        (alpha * 255.0 + 0.5) as u8,
    )
}

pub const DARK: Palette = Palette {
    dark: true,
    bg_deep: hex(0x0d0f12),
    bg: hex(0x121418),
    panel: hex(0x1a1d23),
    raised: hex(0x22262e),
    hover: hex(0x2a2f38),
    pressed: hex(0x333945),
    tile_off: hex(0x16181d),
    line_soft: hex(0x1f2329),
    line: hex(0x2a2e36),
    line_strong: hex(0x3a404b),
    wire: hex(0x4a505c),
    dot: hexa(0xffffff, 0.045),
    text: hex(0xe4e6ea),
    text_soft: hex(0xc3c8d0),
    muted: hex(0x8f97a4),
    faint: hex(0x5b626e),
    accent: hex(0xd8a83b),
    accent_bright: hex(0xf0c25a),
    accent_ink: hex(0x1a1405),
    accent_soft: hexa(0xd8a83b, 0.12),
    accent_line: hexa(0xd8a83b, 0.45),
    hot: hex(0xff8c10),
    hot_soft: hexa(0xff8c10, 0.12),
    ok: hex(0x58b46e),
    ok_soft: hexa(0x58b46e, 0.12),
    danger: hex(0xe8664a),
    danger_soft: hexa(0xe8664a, 0.13),
    info: hex(0x5f9be0),
    info_soft: hexa(0x5f9be0, 0.12),
    led_off: hex(0x2a2e36),
    scrim: hexa(0x060709, 0.62),
    shadow: hexa(0x000000, 0.55),
    knob_top: hex(0x363b45),
    knob_bottom: hex(0x1d2026),
    knob_edge: hex(0x474d58),
    knob_shadow: hexa(0x000000, 0.45),
    switch_top: hex(0x41464f),
    switch_bottom: hex(0x22262c),
    switch_edge: hex(0x4c525d),
    categories: [
        hex(0xe8664a), // Distortion
        hex(0xd9b13b), // Dynamics
        hex(0xa9bb4f), // EQ
        hex(0x5f9be0), // Modulation
        hex(0x58b46e), // Delay
        hex(0x3fb8b2), // Reverb
        hex(0x9d80e3), // Pitch/Synth
        hex(0xd27bc0), // Filter
        hex(0xb96fd8), // Wah
        hex(0xec8a35), // Amp, Amp+Cab
        hex(0xf2ad55), // Preamp
        hex(0xc69462), // Cab
        hex(0xe07892), // IR
        hex(0x93a3b6), // Volume/Pan
        hex(0x7fb2a2), // Send/Return
        hex(0xa69e98), // Looper
        hex(0x8b93a1), // Input and output
    ],
};

pub const LIGHT: Palette = Palette {
    dark: false,
    bg_deep: hex(0xe7e8e5),
    bg: hex(0xf5f5f3),
    panel: hex(0xffffff),
    raised: hex(0xefefec),
    hover: hex(0xe7e7e3),
    pressed: hex(0xdcdcd7),
    tile_off: hex(0xf0f0ee),
    line_soft: hex(0xebebe8),
    line: hex(0xdfdfda),
    line_strong: hex(0xc4c5c0),
    wire: hex(0x9ea3ab),
    dot: hexa(0x000000, 0.06),
    text: hex(0x1a1c1f),
    text_soft: hex(0x3b3f46),
    muted: hex(0x69707a),
    faint: hex(0xa2a7af),
    accent: hex(0xa87612),
    accent_bright: hex(0xf3c868),
    accent_ink: hex(0x1a1405),
    accent_soft: hexa(0xd8a83b, 0.16),
    accent_line: hexa(0xa87612, 0.45),
    hot: hex(0xdd6a00),
    hot_soft: hexa(0xdd6a00, 0.12),
    ok: hex(0x2d8a47),
    ok_soft: hexa(0x2d8a47, 0.12),
    danger: hex(0xc8492d),
    danger_soft: hexa(0xc8492d, 0.10),
    info: hex(0x3570b8),
    info_soft: hexa(0x3570b8, 0.10),
    led_off: hex(0xd5d6d2),
    scrim: hexa(0x1e2024, 0.28),
    shadow: hexa(0x1e2024, 0.22),
    knob_top: hex(0xffffff),
    knob_bottom: hex(0xe6e6e2),
    knob_edge: hex(0xc4c6c1),
    knob_shadow: hexa(0x000000, 0.10),
    switch_top: hex(0xffffff),
    switch_bottom: hex(0xdcdcd8),
    switch_edge: hex(0xb9bbb6),
    categories: [
        hex(0xcf4b30),
        hex(0xa98510),
        hex(0x788c22),
        hex(0x3a76c2),
        hex(0x2e8a49),
        hex(0x1b8c87),
        hex(0x7558cc),
        hex(0xb0509b),
        hex(0x964cc0),
        hex(0xcf6c12),
        hex(0xc68420),
        hex(0x976a3c),
        hex(0xc24b6b),
        hex(0x64758a),
        hex(0x4a8676),
        hex(0x837b75),
        hex(0x7c828a),
    ],
};

thread_local! {
    /// Which palette this thread draws with. Per thread rather than per
    /// process: the interface is drawn on one thread, and tests that build
    /// interfaces side by side must not recolour each other's.
    static DARK_NOW: Cell<bool> = const { Cell::new(true) };
}

/// The palette in use this frame.
#[must_use]
pub fn palette() -> &'static Palette {
    if is_dark() {
        &DARK
    } else {
        &LIGHT
    }
}

/// Whether the dark palette is the one in use.
#[must_use]
pub fn is_dark() -> bool {
    DARK_NOW.with(Cell::get)
}

macro_rules! tokens {
    ($($name:ident),* $(,)?) => {
        $(
            #[must_use]
            #[inline]
            pub fn $name() -> Color32 {
                palette().$name
            }
        )*
    };
}

tokens!(
    bg_deep,
    bg,
    panel,
    raised,
    hover,
    pressed,
    tile_off,
    line_soft,
    line,
    line_strong,
    wire,
    dot,
    text,
    text_soft,
    muted,
    faint,
    accent,
    accent_bright,
    accent_ink,
    accent_soft,
    accent_line,
    hot,
    hot_soft,
    ok,
    ok_soft,
    danger,
    danger_soft,
    info,
    info_soft,
    led_off,
    scrim,
);

/// `colour` at `alpha`, as CSS's `color-mix(colour a%, transparent)`.
#[must_use]
pub fn alpha(colour: Color32, alpha: f32) -> Color32 {
    let [r, g, b, a] = colour.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(
        r,
        g,
        b,
        (f32::from(a) * alpha.clamp(0.0, 1.0)).round() as u8,
    )
}

/// `amount` of `colour` over `base`, as CSS's `color-mix(colour a%, base)`.
#[must_use]
pub fn mix(base: Color32, colour: Color32, amount: f32) -> Color32 {
    base.lerp_to_gamma(colour, amount.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------
// Appearance

/// Which theme the editor uses: the system's, or one chosen in settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Dark or light as the desktop is; dark where it does not say.
    #[default]
    System,
    Dark,
    Light,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Dark, Appearance::Light];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        }
    }

    fn preference(self) -> egui::ThemePreference {
        match self {
            Appearance::System => egui::ThemePreference::System,
            Appearance::Dark => egui::ThemePreference::Dark,
            Appearance::Light => egui::ThemePreference::Light,
        }
    }
}

/// Install fonts, icons and both themes' styles, once, before the first
/// frame, and start in the chosen appearance.
pub fn install(ctx: &egui::Context, appearance: Appearance) {
    fonts(ctx);
    register_icons(ctx);
    let rendering = text_rendering();
    for (theme, palette) in [(egui::Theme::Dark, &DARK), (egui::Theme::Light, &LIGHT)] {
        let mut style = style(palette);
        rendering.apply_to_visuals(&mut style.visuals);
        ctx.set_style_of(theme, style);
    }
    // egui falls back to dark when the desktop does not say.
    ctx.options_mut(|options| options.fallback_theme = egui::Theme::Dark);
    choose(ctx, appearance);
}

/// Follow a new choice of appearance.
pub fn choose(ctx: &egui::Context, appearance: Appearance) {
    ctx.set_theme(appearance.preference());
    follow(ctx);
}

/// Read the theme egui resolved for this frame, so every token agrees with
/// egui's own widgets. Called at the start of each frame; with the system
/// appearance this is how a change on the desktop arrives.
pub fn follow(ctx: &egui::Context) {
    DARK_NOW.with(|dark| dark.set(ctx.theme() == egui::Theme::Dark));
}

// ---------------------------------------------------------------------------
// Type

/// The interface face at a weight.
pub use fastframe_fonts::Weight;

/// Inter at `size` points and `weight`. Every figure is tabular.
#[must_use]
pub fn font(size: f32, weight: Weight) -> egui::FontId {
    weight.font_id(size)
}

#[must_use]
pub fn regular(size: f32) -> egui::FontId {
    font(size, Weight::Regular)
}

#[must_use]
pub fn medium(size: f32) -> egui::FontId {
    font(size, Weight::Medium)
}

#[must_use]
pub fn semibold(size: f32) -> egui::FontId {
    font(size, Weight::SemiBold)
}

#[must_use]
pub fn bold(size: f32) -> egui::FontId {
    font(size, Weight::Bold)
}

/// Inter for the interface, and Inter again for egui's monospace family:
/// its figures are tabular, so a reading keeps its width without a second
/// face. IBM Plex Mono, which existed for exactly that, is gone.
///
/// egui's own fonts and the installed faces for scripts Inter lacks stay
/// behind it as fallbacks; a missing glyph must never become an empty box.
/// Every face is rasterized the way the desktop asks (see [`text_rendering`]).
pub fn fonts(ctx: &egui::Context) {
    let mut fonts = font_setup().definitions();
    text_rendering().apply_to(&mut fonts);
    ctx.set_fonts(fonts);
}

/// The desktop's font rendering settings: its hinting and, on Linux, glyph
/// coverage drawn linearly as FreeType and cairo draw it. Asked once per
/// process, since on Linux it is a D-Bus call.
fn text_rendering() -> fastframe_text::TextRendering {
    static RENDERING: std::sync::OnceLock<fastframe_text::TextRendering> =
        std::sync::OnceLock::new();
    *RENDERING.get_or_init(fastframe_text::detect)
}

fn font_setup() -> fastframe_fonts::FontSetup {
    use fastframe_fonts::{FontSetup, Monospace};
    FontSetup::default()
        .weights(&[Weight::Medium, Weight::SemiBold, Weight::Bold])
        .monospace(Monospace::Inter)
}

// Sizes of the type scale, in points.
/// Tile captions and endpoint labels (700, caps).
pub const CAPTION_TINY: f32 = 9.5;
/// Section captions (600, caps), chips and tags.
pub const CAPTION: f32 = 11.0;
/// Knob, slot and meta labels.
pub const LABEL: f32 = 11.5;
/// Secondary text and tile names.
pub const SECONDARY: f32 = 12.5;
/// Body text, rows and buttons.
pub const BODY: f32 = 13.0;
/// Card and sheet titles.
pub const TITLE: f32 = 15.0;
/// The block's name in the pane.
pub const BLOCK_NAME: f32 = 17.0;
/// The preset's name in the deck.
pub const PRESET_NAME: f32 = 20.0;
/// Page titles.
pub const PAGE_TITLE: f32 = 22.0;
/// The welcome screen, the only display size.
pub const DISPLAY: f32 = 30.0;

// Shape.
/// The spacing grid.
pub const GRID: f32 = 4.0;
pub const RADIUS_CHIP: u8 = 5;
pub const RADIUS_CONTROL: u8 = 8;
pub const RADIUS_MENU: u8 = 10;
pub const RADIUS_TILE: u8 = 12;
pub const RADIUS_CARD: u8 = 14;
pub const RADIUS_DIALOG: u8 = 16;

/// The three sizes the layout is drawn for, by window width: S up to 1100
/// points (compact tiles, smaller knobs, shorter labels), M the reference
/// (1280 × 760), and L from 1800 points (larger tiles and knobs, and every
/// lens on screen at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    S,
    M,
    L,
}

impl Tier {
    #[must_use]
    pub fn of(width: f32) -> Tier {
        if width <= 1100.0 {
            Tier::S
        } else if width >= 1800.0 {
            Tier::L
        } else {
            Tier::M
        }
    }

    /// The window's tier this frame.
    #[must_use]
    pub fn now(ctx: &egui::Context) -> Tier {
        Tier::of(ctx.content_rect().width())
    }

    /// One of three values, by tier.
    #[must_use]
    pub fn pick<T>(self, s: T, m: T, l: T) -> T {
        match self {
            Tier::S => s,
            Tier::M => m,
            Tier::L => l,
        }
    }
}

/// egui's own widgets in the palette's colours, with the design's type scale
/// and spacing, for the parts of the interface egui draws itself: text
/// fields, menus, scroll bars, tooltips and combo boxes.
fn style(palette: &Palette) -> egui::Style {
    use egui::{CornerRadius, FontFamily, FontId, TextStyle};
    let theme = if palette.dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    let mut style = egui::Style {
        visuals: theme.default_visuals(),
        ..Default::default()
    };
    let v = &mut style.visuals;
    v.dark_mode = palette.dark;
    v.override_text_color = Some(palette.text);
    v.weak_text_color = Some(palette.muted);
    v.hyperlink_color = palette.accent;
    v.warn_fg_color = palette.hot;
    v.error_fg_color = palette.danger;
    v.panel_fill = palette.bg;
    v.window_fill = palette.panel;
    v.window_stroke = Stroke::new(1.0, palette.line_strong);
    v.window_corner_radius = CornerRadius::same(RADIUS_DIALOG);
    v.menu_corner_radius = CornerRadius::same(RADIUS_MENU);
    v.extreme_bg_color = if palette.dark {
        palette.bg_deep
    } else {
        palette.panel
    };
    v.text_edit_bg_color = Some(v.extreme_bg_color);
    v.faint_bg_color = palette.raised;
    v.code_bg_color = palette.raised;
    v.selection.bg_fill = alpha(palette.accent, 0.32);
    v.selection.stroke = Stroke::new(1.0, palette.accent_line);
    v.text_cursor.stroke = Stroke::new(1.5, palette.text);
    let shadow = |offset: [i8; 2], blur: u8, spread: u8| egui::epaint::Shadow {
        offset,
        blur,
        spread,
        color: palette.shadow,
    };
    v.popup_shadow = shadow([0, 8], 24, 0);
    v.window_shadow = shadow([0, 16], 48, 0);
    v.window_highlight_topmost = false;

    let corner = CornerRadius::same(RADIUS_CONTROL);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = palette.panel;
    w.noninteractive.weak_bg_fill = palette.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, palette.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, palette.text_soft);
    w.noninteractive.corner_radius = corner;
    for (state, fill, stroke, ink) in [
        (
            &mut w.inactive,
            palette.raised,
            palette.line_strong,
            palette.text,
        ),
        (
            &mut w.hovered,
            palette.hover,
            palette.line_strong,
            palette.text,
        ),
        (
            &mut w.active,
            palette.pressed,
            palette.line_strong,
            palette.text,
        ),
        (
            &mut w.open,
            palette.hover,
            palette.line_strong,
            palette.text,
        ),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.fg_stroke = Stroke::new(1.0, ink);
        state.corner_radius = corner;
        state.expansion = 0.0;
    }

    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(CAPTION, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(BODY, FontFamily::Proportional)),
        (TextStyle::Button, semibold(BODY)),
        (TextStyle::Heading, semibold(BLOCK_NAME)),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();
    // Readings change under the pointer; Inter's tabular figures keep them
    // still in any family, so DragValue may use the body face.
    style.drag_value_text_style = TextStyle::Body;
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 4.0);
    style.spacing.interact_size = Vec2::new(40.0, 24.0);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.slider_width = 180.0;
    style.spacing.combo_width = 120.0;
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style
}

// ---------------------------------------------------------------------------
// Categories

/// A block's category, which is what colours it everywhere: one palette for
/// HX and PRO tones and the website (tonepush.rocks' `CATEGORY_ACCENTS`).
///
/// It replaced HX Edit's colours, which gave amp, preamp and cab one red,
/// dynamics and EQ one yellow, and pitch, filter and wah one purple, and the
/// separate set the PRO editor had. What HX Edit's colours carried, the light
/// under your foot, stays exact: footswitch LEDs are drawn in the colour the
/// pedal lights ([`led`]), never in a category colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Distortion,
    Dynamics,
    Eq,
    Modulation,
    Delay,
    Reverb,
    PitchSynth,
    Filter,
    Wah,
    Amp,
    Preamp,
    Cab,
    Ir,
    VolumePan,
    SendReturn,
    Looper,
    Io,
}

impl Category {
    pub const COUNT: usize = 17;

    /// From the category's name as HX Edit's catalog (and the PRO adapter)
    /// spell it. Unknown names are input and output, the neutral grey.
    #[must_use]
    pub fn named(name: &str) -> Category {
        match name {
            "Distortion" => Category::Distortion,
            "Dynamics" => Category::Dynamics,
            "EQ" => Category::Eq,
            "Modulation" => Category::Modulation,
            "Delay" => Category::Delay,
            "Reverb" => Category::Reverb,
            "Pitch/Synth" => Category::PitchSynth,
            "Filter" => Category::Filter,
            "Wah" => Category::Wah,
            "Amp" | "Amp+Cab" => Category::Amp,
            "Preamp" => Category::Preamp,
            "Cab" => Category::Cab,
            "IR" => Category::Ir,
            "Volume/Pan" => Category::VolumePan,
            "Send/Return" => Category::SendReturn,
            "Looper" => Category::Looper,
            _ => Category::Io,
        }
    }

    /// Its colour in the palette in use.
    #[must_use]
    pub fn colour(self) -> Color32 {
        palette().categories[self as usize]
    }
}

/// A category's colour by name, in the palette in use.
#[must_use]
pub fn category_colour(name: &str) -> Color32 {
    Category::named(name).colour()
}

// ---------------------------------------------------------------------------
// Footswitch LEDs

/// The colour a footswitch lights, by the name HX Edit gives it: what the
/// pedal shows, never a category colour. `None` for Auto Color and anything
/// a later HX Edit adds, where the switch takes the colour of what it carries.
#[must_use]
pub fn led(name: &str) -> Option<Color32> {
    Some(match name {
        "White" => hex(0xf4f4f4),
        "Red" => hex(0xff3b30),
        "Dark Orange" => hex(0xe0601a),
        "Light Orange" => hex(0xff8c10),
        "Yellow" => hex(0xffd52e),
        "Green" => hex(0x39d05a),
        "Turquoise" => hex(0x2fd6c8),
        "Blue" => hex(0x3a7cff),
        "Violet" => hex(0x9a52ff),
        "Pink" => hex(0xff5cc4),
        "Off" => led_off(),
        _ => return None,
    })
}

/// The swatch for a footswitch LED colour name: its light, or the muted ink
/// for Auto Color, which no one swatch can stand for.
#[must_use]
pub fn led_swatch(name: &str) -> Color32 {
    led(name).unwrap_or_else(muted)
}

// ---------------------------------------------------------------------------
// Icons

/// One icon set: Lucide's (ISC) for the interface, drawn white so egui can
/// tint them, and TonePush's own glyphs where Lucide has no word (a pedal, a
/// computer).
macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon {
            $($variant,)*
        }

        const UI_ICONS: &[(Icon, &str, &[u8])] = &[$((
            Icon::$variant,
            concat!("bytes://ui-", $file, ".svg"),
            include_bytes!(concat!("../../assets/icons/ui/", $file, ".svg")).as_slice(),
        )),*];
    };
}

icons! {
    // The three places a tone can live.
    Pedal => "pedal",
    Computer => "computer",
    Cloud => "cloud",
    Save => "save",
    Discard => "circle-x",
    Undo => "undo-2",
    Redo => "redo-2",
    Copy => "copy",
    Paste => "clipboard-paste",
    Remove => "trash-2",
    Gear => "settings",
    Sliders => "sliders-vertical",
    SlidersHorizontal => "sliders-horizontal",
    Keep => "import",
    Star => "star",
    StarOn => "star-on",
    Repeat => "repeat",
    PanelLeft => "panel-left",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    ChevronLeft => "chevron-left",
    ChevronUp => "chevron-up",
    ChevronsUpDown => "chevrons-up-down",
    Ellipsis => "ellipsis",
    Search => "search",
    Plus => "plus",
    Minus => "minus",
    Close => "x",
    Check => "check",
    CircleCheck => "circle-check",
    CircleAlert => "circle-alert",
    TriangleAlert => "triangle-alert",
    Info => "info",
    Lock => "lock",
    ShieldCheck => "shield-check",
    ShieldAlert => "shield-alert",
    Shield => "shield",
    Library => "library",
    Download => "download",
    Upload => "upload",
    HardDrive => "hard-drive",
    Usb => "usb",
    Plug => "plug",
    PlugZap => "plug-zap",
    Cable => "cable",
    Power => "power",
    PowerOff => "power-off",
    RotateCcw => "rotate-ccw",
    RotateCw => "rotate-cw",
    RefreshCw => "refresh-cw",
    History => "history",
    Monitor => "monitor",
    LayoutGrid => "layout-grid",
    List => "list",
    ArrowRight => "arrow-right",
    ArrowUpRight => "arrow-up-right",
    ExternalLink => "external-link",
    Pencil => "pencil",
    Sun => "sun",
    Moon => "moon",
    Timer => "timer",
    Activity => "activity",
    AudioWaveform => "audio-waveform",
    FileAudio => "file-audio",
    FileDown => "file-down",
    Layers => "layers",
    GitCompare => "git-compare",
    Camera => "camera",
    OctagonAlert => "octagon-alert",
    FolderOpen => "folder-open",
    Folder => "folder",
    PackageCheck => "package-check",
    CircleDashed => "circle-dashed",
    CircleDot => "circle-dot",
    LoaderCircle => "loader-circle",
    MousePointerClick => "mouse-pointer-click",
    Tag => "tag",
    Hash => "hash",
    ListChecks => "list-checks",
    TextCursorInput => "text-cursor-input",
    LifeBuoy => "life-buoy",
    Keyboard => "keyboard",
    Replace => "replace",
    SquareDashed => "square-dashed",
}

impl Icon {
    /// Where this icon's artwork is registered.
    #[must_use]
    pub fn uri(self) -> &'static str {
        UI_ICONS
            .iter()
            .find(|(icon, _, _)| *icon == self)
            .map(|(_, uri, _)| *uri)
            .unwrap_or_default()
    }

    /// The icon as a picture to paint.
    #[must_use]
    pub fn art(self) -> Art {
        Art::whole(self.uri().to_owned())
    }
}

/// TonePush's own category drawings (`assets/icons/category`), shared with
/// the website, and the endpoint and junction drawings beside them. They are
/// written in `currentColor` for the website; here they are served white so
/// egui's tint is the colour on screen.
macro_rules! drawings {
    ($($name:literal => $file:literal),* $(,)?) => {
        const DRAWINGS: &[(&str, &str, &[u8])] = &[$((
            $name,
            concat!("bytes://drawing-", $file, ".svg"),
            include_bytes!(concat!("../../assets/icons/", $file, ".svg")).as_slice(),
        )),*];
    };
}

drawings! {
    "Distortion" => "category/distortion",
    "Dynamics" => "category/dynamics",
    "EQ" => "category/eq",
    "Modulation" => "category/modulation",
    "Delay" => "category/delay",
    "Reverb" => "category/reverb",
    "Pitch/Synth" => "category/pitch-synth",
    "Filter" => "category/filter",
    "Wah" => "category/wah",
    "Amp+Cab" => "category/amp-cab",
    "Amp" => "category/amp",
    "Preamp" => "category/preamp",
    "Cab" => "category/cab",
    "IR" => "category/ir",
    "Volume/Pan" => "category/volume-pan",
    "Send/Return" => "category/send-return",
    "Looper" => "category/looper",
    "Input" => "input",
    "Output" => "output",
    "Split" => "split",
    "Merge" => "merge",
    "Connected Devices" => "connected-devices",
}

/// TonePush's own mark: the app icon as it is packaged, so the editor shows
/// whatever icon the release ships.
const MARK_URI: &str = "bytes://tonepush-mark.svg";
const MARK: &[u8] = include_bytes!("../../../../packaging/icons/tonepush.svg");

/// Hand the icons and drawings to egui's loaders, once, so they can be drawn
/// by URI like any other image.
pub fn register_icons(ctx: &egui::Context) {
    for (_, uri, bytes) in UI_ICONS {
        ctx.include_bytes(*uri, *bytes);
    }
    for (_, uri, bytes) in DRAWINGS {
        let svg = String::from_utf8_lossy(bytes).replace("currentColor", "#ffffff");
        ctx.include_bytes(*uri, svg.into_bytes());
    }
    ctx.include_bytes(MARK_URI, MARK);
}

/// Paint TonePush's mark, in its own colours, filling `rect`.
pub fn paint_mark(ui: &egui::Ui, rect: egui::Rect) {
    egui::Image::new(MARK_URI).paint_at(ui, rect);
}

/// The drawing for a category, by the name HX Edit gives it.
#[must_use]
pub fn category_icon(name: &str) -> Option<Art> {
    DRAWINGS
        .iter()
        .find(|(label, _, _)| *label == name)
        .map(|(_, uri, _)| Art::whole((*uri).to_owned()))
}

/// A picture to draw: a whole file, or one frame of a vertical strip.
///
/// The endpoints need the second form. HX Edit draws whichever destination an
/// input or output is routed to, and those live as vertical strips of equal
/// frames in one file rather than as separate images.
#[derive(Clone, Debug, PartialEq)]
pub struct Art {
    pub uri: String,
    /// `(frame, total)` when the file is a strip; `None` for a plain image.
    pub frame: Option<(usize, usize)>,
}

impl Art {
    pub fn whole(uri: String) -> Art {
        Art { uri, frame: None }
    }

    pub fn strip(uri: String, frame: usize, total: usize) -> Art {
        Art {
            uri,
            frame: Some((frame, total)),
        }
    }

    /// Paint it centred in `area`, keeping its proportions, tinted `tint`.
    pub fn paint(&self, ui: &egui::Ui, area: egui::Rect, tint: Color32) {
        match self.frame {
            None => fit(ui, &self.uri, area, tint),
            Some((frame, total)) if total > 0 => {
                // One frame of a vertical strip, kept square: the source cells
                // are square, so the drawn box must be too or the icon skews.
                let side = area.height().min(area.width());
                let square = egui::Rect::from_center_size(area.center(), Vec2::splat(side));
                let top = frame.min(total - 1) as f32 / total as f32;
                egui::Image::new(&self.uri)
                    .tint(tint)
                    .uv(egui::Rect::from_min_max(
                        egui::pos2(0.0, top),
                        egui::pos2(1.0, top + 1.0 / total as f32),
                    ))
                    .paint_at(ui, square);
            }
            _ => {}
        }
    }
}

/// Draw an image centred inside `area`, preserving its proportions.
///
/// `Image::paint_at` stretches to whatever rectangle it is given, which turns
/// wide pedal photographs into squashed ones. Asking the image for its natural
/// size first and scaling by the smaller of the two ratios letterboxes it
/// instead.
fn fit(ui: &egui::Ui, uri: &str, area: egui::Rect, tint: Color32) {
    let image = egui::Image::new(uri).maintain_aspect_ratio(true).tint(tint);
    // Until the texture has loaded its size is unknown; fill the box for that
    // frame and let the next one place it properly.
    let natural = image
        .load_and_calc_size(ui, Vec2::splat(f32::INFINITY))
        .unwrap_or(area.size());
    let scale = (area.width() / natural.x)
        .min(area.height() / natural.y)
        .min(1.0);
    let placed = egui::Rect::from_center_size(area.center(), natural * scale);
    image.paint_at(ui, placed);
}

/// Paint an icon `size` points square, centred on `centre`, in `colour`.
pub fn paint_icon(ui: &egui::Ui, icon: Icon, centre: egui::Pos2, size: f32, colour: Color32) {
    egui::Image::new(icon.uri())
        .tint(colour)
        .paint_at(ui, egui::Rect::from_center_size(centre, Vec2::splat(size)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_use_inter_with_tabular_figures_in_every_family() {
        let fonts = font_setup().system_fallbacks(false).definitions();
        let first = |family: &egui::FontFamily| fonts.families[family].first().cloned();
        assert_eq!(
            first(&egui::FontFamily::Proportional).as_deref(),
            Some(fastframe_fonts::INTER_REGULAR)
        );
        assert_eq!(
            first(&egui::FontFamily::Monospace).as_deref(),
            Some(fastframe_fonts::INTER_REGULAR),
            "readings are Inter, not a second face"
        );
        for weight in [Weight::Medium, Weight::SemiBold, Weight::Bold] {
            assert_eq!(
                first(&weight.family()).as_deref(),
                Some(weight.name()),
                "{weight:?} is registered"
            );
        }
        assert!(
            !fonts.font_data.keys().any(|name| name.contains("plex")),
            "IBM Plex Mono is gone"
        );
    }

    #[test]
    fn text_is_drawn_the_way_the_desktop_draws_it() {
        let ctx = egui::Context::default();
        install(&ctx, Appearance::Dark);
        let options = ctx.style_of(egui::Theme::Dark).visuals.text_options;
        assert_eq!(
            options.color_transfer_function,
            text_rendering().color_transfer_function(true)
        );
        // Linux desktops draw coverage linearly whatever their hinting; egui's
        // dark default (2c - c²) made text heavier than GTK's.
        #[cfg(target_os = "linux")]
        assert_eq!(
            options.color_transfer_function,
            egui::epaint::FontColorTransferFunction::Off
        );
    }

    #[test]
    fn each_theme_reads_its_own_tokens() {
        let ctx = egui::Context::default();
        install(&ctx, Appearance::Light);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            follow(ui.ctx());
            assert!(!is_dark());
            assert_eq!(panel(), Color32::WHITE);
            assert_eq!(text(), hex(0x1a1c1f));
            assert_eq!(Category::Reverb.colour(), hex(0x1b8c87));
        })
        .drop_without_applying_deltas();
        choose(&ctx, Appearance::Dark);
        assert!(is_dark());
        assert_eq!(bg(), hex(0x121418));
        assert_eq!(accent(), hex(0xd8a83b));
        assert_eq!(Category::Reverb.colour(), hex(0x3fb8b2));
    }

    #[test]
    fn egui_draws_its_own_widgets_in_the_same_palette() {
        let dark = style(&DARK);
        let light = style(&LIGHT);
        assert_eq!(dark.visuals.panel_fill, DARK.bg);
        assert_eq!(light.visuals.panel_fill, LIGHT.bg);
        assert_eq!(dark.visuals.widgets.inactive.bg_fill, DARK.raised);
        assert_eq!(light.visuals.window_fill, LIGHT.panel);
        assert!(!light.visuals.dark_mode);
    }

    #[test]
    fn one_category_palette_serves_hx_and_pro_names() {
        assert_eq!(Category::named("Amp+Cab"), Category::Amp);
        assert_eq!(Category::named("Pitch/Synth"), Category::PitchSynth);
        assert_eq!(Category::named("Output"), Category::Io);
        assert_eq!(DARK.categories.len(), Category::COUNT);
        // Reverb is teal, not HX Edit's orange or the old PRO blue.
        assert_eq!(DARK.categories[Category::Reverb as usize], hex(0x3fb8b2));
    }

    #[test]
    fn leds_are_the_pedals_colours_and_auto_has_none() {
        assert_eq!(led("Green"), Some(hex(0x39d05a)));
        assert_eq!(led("Turquoise"), Some(hex(0x2fd6c8)));
        assert_eq!(led("Auto Color"), None);
    }

    #[test]
    fn alpha_and_mix_follow_css() {
        let half = alpha(hex(0xff0000), 0.5);
        assert_eq!(half.to_srgba_unmultiplied(), [255, 0, 0, 128]);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
    }

    #[test]
    fn every_icon_and_drawing_is_registered_under_its_own_uri() {
        let mut uris: Vec<&str> = UI_ICONS.iter().map(|(_, uri, _)| *uri).collect();
        uris.extend(DRAWINGS.iter().map(|(_, uri, _)| *uri));
        let count = uris.len();
        uris.sort_unstable();
        uris.dedup();
        assert_eq!(uris.len(), count, "no two icons share a URI");
        assert!(category_icon("Reverb").is_some());
        assert!(category_icon("Amp+Cab").is_some());
    }
}
