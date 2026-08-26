//! Shared editor composition for every supported processor.
//!
//! Protocol implementations provide names, topology, values and commands;
//! these helpers keep TonePush's spatial language and interaction affordances
//! identical across devices.

use egui::Ui;

use crate::theme;

pub(crate) const TOP_HEIGHT: f32 = 46.0;
pub(crate) const STATUS_HEIGHT: f32 = 28.0;
pub(crate) const PRESET_WIDTH: f32 = 216.0;
pub(crate) const PRESET_RANGE: std::ops::RangeInclusive<f32> = 150.0..=340.0;

pub(crate) fn top_bar(root: &mut Ui, id: &'static str, body: impl FnOnce(&mut Ui)) {
    egui::Panel::top(id)
        .exact_size(TOP_HEIGHT)
        .show(root, |ui| ui.horizontal_centered(body));
}

pub(crate) fn status_bar(root: &mut Ui, id: &'static str, body: impl FnOnce(&mut Ui)) {
    egui::Panel::bottom(id)
        .exact_size(STATUS_HEIGHT)
        .show(root, |ui| ui.horizontal_centered(body));
}

pub(crate) fn preset_panel(root: &mut Ui, id: &'static str, body: impl FnOnce(&mut Ui)) {
    egui::Panel::left(id)
        .default_size(PRESET_WIDTH)
        .size_range(PRESET_RANGE)
        .show(root, body);
}

pub(crate) fn chain_panel(
    root: &mut Ui,
    id: &'static str,
    default_height: f32,
    range: std::ops::RangeInclusive<f32>,
    body: impl FnOnce(&mut Ui),
) {
    egui::Panel::top(id)
        .resizable(true)
        .default_size(default_height)
        .size_range(range)
        .show(root, body);
}

pub(crate) fn editor(root: &mut Ui, body: impl FnOnce(&mut Ui)) {
    egui::CentralPanel::default().show(root, body);
}

pub(crate) fn device_button(
    ui: &mut Ui,
    enabled: bool,
    name: impl Into<egui::WidgetText>,
    hover: &str,
) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(name))
        .on_hover_text(hover)
}

/// A short wire for a fixed chain. HX chains reserve room between blocks for
/// insertion targets; the PRO cannot insert or move processors.
pub(crate) fn fixed_connector(ui: &mut Ui) {
    theme::wire_run(ui, 4.0, 88.0);
}

pub(crate) fn fixed_signal_block(
    ui: &mut Ui,
    name: &str,
    category: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let art = theme::category_icon(category);
    theme::fixed_block_button_tinted(
        ui,
        name,
        category,
        art.as_ref(),
        selected,
        enabled,
        category_accent(category),
    )
}

pub(crate) fn fixed_endpoint(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let art = theme::category_icon(label);
    theme::fixed_block_button_tinted(
        ui,
        label,
        label,
        art.as_ref(),
        selected,
        true,
        category_accent(label),
    )
}

/// The loaded preset title, including its stable dirty indicator and inline
/// rename field. `Some(name)` is returned only when Enter commits a rename.
pub(crate) fn preset_title(
    ui: &mut Ui,
    slot: &str,
    name: &str,
    dirty: bool,
    rename_enabled: bool,
    renaming: &mut Option<String>,
) -> Option<String> {
    let (dot, _) = ui.allocate_exact_size(egui::Vec2::new(10.0, 14.0), egui::Sense::hover());
    if dirty {
        ui.painter().circle_filled(dot.center(), 4.0, theme::ACCENT);
    }
    ui.label(
        egui::RichText::new(format!("{slot}  "))
            .size(16.0)
            .color(theme::DIM),
    );

    if let Some(draft) = renaming {
        let field = ui.add(
            egui::TextEdit::singleline(draft)
                .desired_width(220.0)
                .font(theme::semibold(16.0)),
        );
        if !field.has_focus() && !field.lost_focus() {
            field.request_focus();
        }
        if field.lost_focus() {
            let commit = ui.input(|input| input.key_pressed(egui::Key::Enter));
            let result = commit.then(|| draft.clone());
            *renaming = None;
            return result;
        }
        return None;
    }

    let shown = ui.add_enabled(
        rename_enabled,
        egui::Label::new(
            egui::RichText::new(name)
                .font(theme::semibold(16.0))
                .color(ui.visuals().strong_text_color()),
        )
        .selectable(false)
        .sense(egui::Sense::click()),
    );
    if shown
        .on_hover_text(if !rename_enabled {
            "rename becomes available when persistent writes are guarded"
        } else if dirty {
            "unsaved changes - click the name to rename it"
        } else {
            "click the name to rename it"
        })
        .clicked()
    {
        *renaming = Some(name.to_owned());
    }
    None
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresetActions {
    pub(crate) undo: bool,
    pub(crate) redo: bool,
    pub(crate) save: bool,
}

/// The preset-wide actions are deliberately rendered in one place. A device
/// adapter supplies capability/state booleans; it does not get a second
/// visual vocabulary for Undo, Redo, and Save.
pub(crate) fn preset_tools(
    ui: &mut Ui,
    live: bool,
    undo_depth: usize,
    redo_depth: usize,
    save_enabled: bool,
    save_disabled: &str,
) -> PresetActions {
    let hint = |m, k| ui.ctx().format_shortcut(&egui::KeyboardShortcut::new(m, k));
    let save_hint = hint(egui::Modifiers::COMMAND, egui::Key::S);
    let undo_hint = hint(egui::Modifiers::COMMAND, egui::Key::Z);
    let redo_hint = hint(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );

    let undo = theme::icon_button(ui, theme::Icon::Undo, live && undo_depth > 0)
        .on_hover_text(format!("Undo - step back through changes ({undo_hint})"))
        .clicked();
    let redo = theme::icon_button(ui, theme::Icon::Redo, live && redo_depth > 0)
        .on_hover_text(format!("Redo - put back what undo took away ({redo_hint})"))
        .clicked();
    let save = theme::icon_button(ui, theme::Icon::Save, save_enabled)
        .on_hover_text(format!(
            "Save - write these changes into the preset ({save_hint})"
        ))
        .on_disabled_hover_text(save_disabled)
        .clicked();
    PresetActions { undo, redo, save }
}

/// The common BPM display/editor. Device adapters only decide where the
/// resulting number is sent.
pub(crate) fn tempo_control(
    ui: &mut Ui,
    tempo: f32,
    draft: &mut Option<String>,
    taps: &mut Vec<std::time::Instant>,
) -> Option<f32> {
    let mut changed = None;
    if ui
        .button("Tap")
        .on_hover_text("tap in time to set the tempo")
        .clicked()
    {
        let now = std::time::Instant::now();
        if taps
            .last()
            .is_some_and(|previous| now.duration_since(*previous).as_secs_f32() > 2.0)
        {
            taps.clear();
        }
        taps.push(now);
        if taps.len() > 5 {
            taps.remove(0);
        }
        if taps.len() >= 2 {
            let span = taps
                .last()
                .expect("two taps checked")
                .duration_since(taps[0])
                .as_secs_f32();
            let bpm = 60.0 * (taps.len() - 1) as f32 / span;
            if (20.0..=999.0).contains(&bpm) {
                changed = Some(bpm);
            }
        }
    }

    match draft {
        Some(text) => {
            let edit = ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(52.0)
                    .font(egui::TextStyle::Monospace),
            );
            if !edit.has_focus() && !edit.lost_focus() {
                edit.request_focus();
            }
            if edit.lost_focus() {
                if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    changed = text.trim().parse().ok();
                }
                *draft = None;
            }
        }
        None => {
            let label = ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("{tempo:.1} BPM"))
                        .monospace()
                        .color(theme::ACCENT),
                )
                .sense(egui::Sense::click()),
            );
            if label.on_hover_text("click to change tempo").clicked() {
                *draft = Some(format!("{tempo:.1}"));
            }
        }
    }
    changed
}

pub(crate) const CONTROL_CELL: egui::Vec2 = egui::vec2(84.0, 116.0);

/// The centred control grid used under a selected pedal on every processor.
pub(crate) fn control_grid(ui: &Ui, count: usize) -> (usize, f32) {
    let pitch = CONTROL_CELL.x + ui.spacing().item_spacing.x;
    let columns = ((ui.available_width() / pitch).floor() as usize)
        .clamp(1, 8)
        .min(count.max(1));
    let indent = ((ui.available_width() - columns as f32 * pitch) / 2.0).max(0.0);
    (columns, indent)
}

fn category_accent(category: &str) -> egui::Color32 {
    match category {
        "Distortion" => egui::Color32::from_rgb(0xd8, 0x9a, 0x35),
        "Dynamics" | "EQ" => egui::Color32::from_rgb(0xd3, 0xc3, 0x43),
        "Amp" | "Amp+Cab" | "Preamp" => egui::Color32::from_rgb(0xd0, 0x55, 0x4d),
        "Cab" | "IR" => egui::Color32::from_rgb(0x9b, 0x72, 0xc7),
        "Modulation" => egui::Color32::from_rgb(0x52, 0xa9, 0xb8),
        "Delay" => egui::Color32::from_rgb(0x54, 0xaa, 0x68),
        "Reverb" => egui::Color32::from_rgb(0x70, 0x83, 0xd0),
        "Pitch/Synth" | "Filter" | "Wah" => egui::Color32::from_rgb(0xb0, 0x67, 0xb7),
        "Volume/Pan" | "Input" | "Output" => theme::DIM,
        _ => theme::ACCENT,
    }
}

pub(crate) fn parameter_row(
    ui: &mut Ui,
    enabled: bool,
    label: &str,
    path: &str,
    control: impl FnOnce(&mut Ui),
) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.label(label).on_hover_text(path);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), control);
        });
        ui.separator();
    });
}
