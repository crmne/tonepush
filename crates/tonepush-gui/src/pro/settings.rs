//! The PRO's own settings as a page: a card for each part of the pedal, each
//! setting a row with its control at the right, as the HX's Settings tab
//! draws them. They are the pedal's memory, not a preset's, so they wait for
//! a checked backup of it.

use egui::{Ui, Vec2};

use super::*;
use crate::theme::Icon;

/// The parts of the pedal's settings, in the order the page shows them.
const SECTIONS: [(&str, &str); 4] = [
    ("tuner", "Tuner"),
    ("input", "Input"),
    ("misc", "General"),
    ("ctrl", "Controller"),
];

impl Panel {
    /// The Settings tab.
    pub(super) fn settings_page(&mut self, ui: &mut Ui, snapshot: &Snapshot) {
        let writable = self.rollback.is_some() && self.read_only.is_none();
        if !writable {
            theme::label(
                ui,
                "These change the pedal's own memory, so they wait for a checked backup of it.",
                theme::regular(theme::SECONDARY),
                theme::muted(),
            );
            ui.add_space(10.0);
        }
        theme::search_field(
            ui,
            "pro-settings-search",
            &mut self.search,
            "Filter settings",
            220.0,
        );
        ui.add_space(12.0);
        let needle = self.search.trim().to_ascii_lowercase();
        let mut asked: Option<(NodePath, NodeDescription, Value, Value)> = None;
        let mut shown = 0;
        for (section, heading) in SECTIONS {
            let nodes: Vec<(NodePath, NodeDescription, Value)> = snapshot
                .settings
                .iter()
                .filter(|(path, description)| {
                    settings_group(path.as_str()) == Some(section)
                        && node_matches_search(path.as_str(), description, &needle)
                })
                .filter_map(|(path, description)| {
                    let current = self.drafts.get(path.as_str()).cloned()?;
                    editable_node(path.as_str(), description, &current)
                        .then(|| (path.clone(), description.clone(), current))
                })
                .collect();
            if nodes.is_empty() {
                continue;
            }
            if shown > 0 {
                ui.add_space(16.0);
            }
            shown += 1;
            crate::pages::section_head(ui, heading, "", |_| {});
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                let last = nodes.len() - 1;
                for (row, (path, description, current)) in nodes.into_iter().enumerate() {
                    let label = node_label(&path, &description);
                    crate::pages::setting_row(ui, &label, row < last, |ui| {
                        ui.add_enabled_ui(writable && self.online && !self.busy, |ui| {
                            if let Some(value) =
                                setting_control(ui, snapshot, &path, &description, &current)
                            {
                                asked = Some((
                                    path.clone(),
                                    description.clone(),
                                    current.clone(),
                                    value,
                                ));
                            }
                        });
                    });
                }
            });
        }
        if shown == 0 {
            theme::label(
                ui,
                "No settings match.",
                theme::regular(theme::SECONDARY),
                theme::muted(),
            );
        }
        if let Some((path, description, before, value)) = asked {
            if description.validate_value(&value).is_ok() {
                self.drafts.insert(path.to_string(), value.clone());
                self.recompute_dirty();
                let _ = self.tx.send(Cmd::SetNode {
                    path,
                    description: Box::new(description),
                    before,
                    value,
                    persistent: true,
                });
            }
        }
    }
}

/// One setting's control, laid out right to left: returns a new value when
/// it was changed.
fn setting_control(
    ui: &mut Ui,
    snapshot: &Snapshot,
    path: &NodePath,
    description: &NodeDescription,
    current: &Value,
) -> Option<Value> {
    let mut changed = None;
    match description.kind.as_ref() {
        Some(NodeKind::Float) => {
            let mut number = current.as_f64().unwrap_or_default();
            let unit = description
                .unit
                .as_deref()
                .map(|unit| format!(" {unit}"))
                .unwrap_or_default();
            let step = description.step.unwrap_or(0.0).max(0.0);
            let mut typed = egui::DragValue::new(&mut number).suffix(unit);
            if let (Some(min), Some(max)) = (description.min, description.max) {
                typed = typed.range(min..=max).speed((max - min) / 400.0);
            }
            if step > 0.0 {
                // As many decimals as the step has, as the readings show.
                typed = typed.fixed_decimals((-step.log10()).ceil().clamp(0.0, 2.0) as usize);
            }
            if ui.add(typed).changed() {
                changed = json_number(number);
            }
            if let (Some(min), Some(max)) = (description.min, description.max) {
                ui.add_space(12.0);
                let mut slid = number as f32;
                if theme::slider(ui, &mut slid, min as f32..=max as f32, step as f32, 200.0)
                    .changed()
                {
                    changed = json_number(f64::from(slid));
                }
            }
        }
        Some(NodeKind::Enum | NodeKind::Array | NodeKind::PropertyList) => {
            if let Some((off, on)) = toggle_choices(description) {
                let mut lit = *current == on;
                if theme::toggle(ui, &mut lit, theme::accent()).changed() {
                    changed = Some(if lit { on } else { off });
                }
            } else {
                let choices = selector_choices(snapshot, description);
                let button = crate::pages::choice_button(ui, &value_text(current));
                egui::Popup::menu(&button).gap(4.0).show(|ui| {
                    theme::menu_width(ui, 220.0);
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for choice in choices {
                                let chosen = choice == *current;
                                if theme::menu_item(
                                    ui,
                                    chosen.then_some(Icon::Check),
                                    &value_text(&choice),
                                    None,
                                )
                                .clicked()
                                    && !chosen
                                {
                                    changed = Some(choice);
                                }
                            }
                        });
                });
            }
        }
        Some(NodeKind::Item) if current.is_boolean() => {
            let mut lit = current.as_bool().unwrap_or_default();
            if theme::toggle(ui, &mut lit, theme::accent()).changed() {
                changed = Some(Value::Bool(lit));
            }
        }
        Some(NodeKind::Item) if current.is_string() => {
            let id = ui.id().with(("pro-setting-text", path.as_str()));
            let mut text = ui
                .data(|data| data.get_temp::<String>(id))
                .unwrap_or_else(|| current.as_str().unwrap_or_default().to_owned());
            let field = ui.add(egui::TextEdit::singleline(&mut text).desired_width(210.0));
            if field.lost_focus() {
                ui.data_mut(|data| data.remove::<String>(id));
                if text != current.as_str().unwrap_or_default() {
                    changed = Some(Value::String(text));
                }
            } else if field.has_focus() {
                ui.data_mut(|data| data.insert_temp(id, text));
            }
        }
        _ => {
            theme::label(
                ui,
                &value_text(current),
                theme::regular(theme::BODY),
                theme::muted(),
            );
        }
    }
    changed
}
