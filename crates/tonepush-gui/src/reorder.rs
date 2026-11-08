//! Moving a preset to another place in its setlist, the presets between
//! closing up behind it, as dropping one between two others does in HX
//! Edit.
//!
//! The pedal does the moving: the setlist's directory goes back to it in the
//! new order, one message however many presets shift, and it rearranges its
//! presets to match (see `hx_usb::Session::move_preset`). So a move needs no
//! documents from the backup and writes no slots, and only a device whose
//! profile can do it is offered the drop.

use crate::dnd::Outcome;
use crate::session::Cmd;
use crate::theme::Icon;
use crate::App;

/// Where a preset dropped in the gap before `before` ends up.
pub(crate) fn destination(from: i64, before: i64) -> i64 {
    if before > from {
        before - 1
    } else {
        before
    }
}

impl App {
    /// Whether the connected pedal moves presets itself.
    pub(crate) fn moves_presets(&self) -> bool {
        !self.pro_active()
            && hx_proto::PROFILES
                .iter()
                .find(|profile| profile.name == self.device.trim())
                .is_some_and(|profile| profile.moves_presets)
    }

    /// What dropping the preset in `from` in the gap before `before` does.
    pub(crate) fn reorder_outcome(&self, from: i64, before: i64) -> Option<Outcome> {
        let to = destination(from, before);
        if to == from || before < 0 || before > self.slot_count() || !self.moves_presets() {
            return None;
        }
        if let Some(why) = self.put_refusal() {
            return Outcome::refuses(why);
        }
        let between = (to - from).abs();
        let shift = if from < to { "up" } else { "down" };
        Outcome::does(
            Icon::ArrowDownToLine,
            format!(
                "Move to {}: {between} {} shift {shift}",
                self.active_slot_label(to),
                if between == 1 { "preset" } else { "presets" },
            ),
        )
    }

    /// Move the preset in `from` to the gap before `before`. Nothing asks:
    /// the pedal does it in a moment, and moving it back undoes it.
    pub(crate) fn reorder(&mut self, from: i64, before: i64) {
        let to = destination(from, before);
        if to == from || self.reorder_outcome(from, before).is_none_or(|o| o.refused) {
            return;
        }
        let name = self.presets.get(from as usize).cloned().unwrap_or_default();
        self.note(format!(
            "moving {} {} to {}",
            self.active_slot_label(from),
            name.trim(),
            self.active_slot_label(to)
        ));
        self.send(Cmd::MovePreset { from, to });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_preset_lands_in_the_gap_it_was_dropped_in() {
        // Down the list: the gap before 10 is slot 9 once 3 has left.
        assert_eq!(destination(3, 10), 9);
        // Up the list: the gap before 1 is slot 1.
        assert_eq!(destination(10, 1), 1);
        // Either gap beside it leaves it where it is.
        assert_eq!(destination(5, 5), 5);
        assert_eq!(destination(5, 6), 5);
    }

    fn app(device: &str) -> (App, mpsc::Receiver<Cmd>) {
        let (to_device, cmds) = mpsc::channel();
        let (_events, from_device) = mpsc::channel();
        let mut app = App::new(&egui::Context::default(), to_device, from_device);
        let _ = cmds.try_iter().count();
        app.device = device.into();
        app.connection = crate::Connection::Online;
        app.presets = (0..8).map(|slot| format!("P{slot}")).collect();
        (app, cmds)
    }

    /// A drop between two presets is one command to the pedal, sent at once.
    #[test]
    fn a_drop_between_presets_moves_it_in_one_command() {
        let (mut app, cmds) = app("Helix Floor");
        app.reorder(1, 6);
        let sent: Vec<Cmd> = cmds.try_iter().collect();
        assert!(
            matches!(sent.as_slice(), [Cmd::MovePreset { from: 1, to: 5 }]),
            "one move, to the gap's slot"
        );
    }

    /// A pedal whose profile does not move presets is not offered the gap.
    #[test]
    fn only_a_pedal_that_moves_presets_is_offered_the_gap() {
        let (app, _) = app("HX Stomp");
        assert!(app.reorder_outcome(1, 6).is_none());
        let (app, _) = app_floor();
        assert!(app.reorder_outcome(1, 6).is_some_and(|o| !o.refused));
    }

    fn app_floor() -> (App, mpsc::Receiver<Cmd>) {
        app("Helix Floor")
    }
}
