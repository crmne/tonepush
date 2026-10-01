//! Shared editor composition for every supported processor.
//!
//! Protocol implementations provide names, topology, values and commands;
//! these helpers keep TonePush's spatial language and interaction affordances
//! identical across devices. The frame around them (sidebar, decks, pages)
//! is `shell`.

use egui::Ui;

use crate::theme;

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
        theme::category_colour(category),
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
        theme::category_colour(label),
    )
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
