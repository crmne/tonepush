//! How much of each path's DSP the loaded chain uses, and what still fits.
//!
//! A Helix Floor gives each path the same budget and refuses a block that
//! would go past it with error -306: the blocks it greys out on its own
//! screen. A block costs what HX Edit lists for the variant placed (see
//! [`hx_catalog::Catalog::cost`]), an Amp+Cab costs its amp and its cab, and
//! a bypassed block costs as much as an engaged one. Inputs, outputs, splits
//! and joins cost nothing worth counting: a split and its join measured at
//! most 0.05 together.
//!
//! Costs are added up in hundredths of HX Edit's units, which is the
//! precision they are listed in, so the pedal's own boundary is kept exactly:
//! 68 + 7.00 fits on a Floor and 68 + 7.01 does not.

use hx_catalog::Catalog;
use hx_proto::preset::{Kind, Path};

use crate::{session, App};

/// A load in hundredths of HX Edit's units.
pub(crate) fn hundredths(load: f32) -> u32 {
    (load * 100.0).round().max(0.0) as u32
}

/// What a block costs: its model and the cab riding along. `None` where the
/// catalog lists no cost for either, which nothing is held back for.
pub(crate) fn block_cost(catalog: &Catalog, model: u32, paired: Option<u32>) -> Option<u32> {
    let cab = match paired {
        Some(cab) => hundredths(catalog.cost(cab)?),
        None => 0,
    };
    Some(hundredths(catalog.cost(model)?) + cab)
}

/// The slots of a path's blocks: before the split, on its lanes, after the
/// join.
fn blocks_of(path: &Path) -> impl Iterator<Item = usize> + '_ {
    path.head
        .iter()
        .chain(path.lanes.iter().flat_map(|lane| lane.blocks.iter()))
        .chain(path.tail.iter())
        .copied()
}

/// What a path's blocks cost together, or `None` where one of them has no
/// listed cost.
pub(crate) fn path_load(catalog: &Catalog, path: &Path, chain: &[session::Block]) -> Option<u32> {
    blocks_of(path)
        .filter_map(|slot| chain.iter().find(|b| b.position == slot as i64))
        .filter(|block| matches!(block.kind, Kind::Block | Kind::Looper))
        .map(|block| block_cost(catalog, block.model, block.paired))
        .sum()
}

/// A share of the budget, as a whole percentage. Rounding keeps the order of
/// two shares, so a block shown needing no more than what is left always
/// fits.
pub(crate) fn percent(load: u32, budget: u32) -> String {
    shown(load, budget, 0)
}

/// What a block needs and what is left, told apart even when the whole
/// percentages would read the same.
pub(crate) fn percents(need: u32, left: u32, budget: u32) -> (String, String) {
    (0..3)
        .map(|places| (shown(need, budget, places), shown(left, budget, places)))
        .find(|(need, left)| need != left)
        .unwrap_or_else(|| (shown(need, budget, 2), shown(left, budget, 2)))
}

fn shown(load: u32, budget: u32, places: usize) -> String {
    let share = f64::from(load) * 100.0 / f64::from(budget.max(1));
    if places == 0 && load > 0 && share < 0.5 {
        return "<1%".to_owned();
    }
    format!("{share:.places$}%")
}

/// How full one path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Room {
    /// Which path, from 0.
    pub path: usize,
    /// What the path holds, and what its blocks use, in hundredths.
    pub budget: u32,
    pub used: u32,
}

impl Room {
    pub(crate) fn left(&self) -> u32 {
        self.budget.saturating_sub(self.used)
    }

    pub(crate) fn fits(&self, cost: u32) -> bool {
        cost <= self.left()
    }
}

impl App {
    /// What each path of this device holds, where that has been measured.
    fn dsp_budget(&self) -> Option<u32> {
        if self.pro_active() {
            return None;
        }
        hx_proto::PROFILES
            .iter()
            .find(|profile| profile.name == self.device.trim())?
            .dsp_budget
    }

    /// The path a slot belongs to: each path's slots start at its input.
    fn path_of_slot(&self, slot: usize) -> Option<usize> {
        self.layout
            .paths
            .iter()
            .rposition(|path| path.input.is_some_and(|input| input <= slot))
    }

    /// How full a path is, where the device's budget and every block's cost
    /// are known.
    pub(crate) fn path_room(&self, path: usize) -> Option<Room> {
        let budget = self.dsp_budget()?;
        let catalog = self.catalog.as_ref()?;
        let used = path_load(catalog, self.layout.paths.get(path)?, &self.chain)?;
        Some(Room { path, budget, used })
    }

    /// The room a block going into `slot` has: everything its path holds
    /// except the block there now, which it replaces.
    pub(crate) fn room_at(&self, slot: usize) -> Option<Room> {
        let mut room = self.path_room(self.path_of_slot(slot)?)?;
        let catalog = self.catalog.as_ref()?;
        if let Some(there) = self
            .chain
            .iter()
            .find(|b| b.position == slot as i64 && matches!(b.kind, Kind::Block | Kind::Looper))
        {
            room.used = room
                .used
                .saturating_sub(block_cost(catalog, there.model, there.paired)?);
        }
        Some(room)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hx_proto::preset::Layout;

    fn room(used: f32) -> Room {
        Room {
            path: 1,
            budget: 7500,
            used: hundredths(used),
        }
    }

    /// Measured on a Helix Floor: the limit itself fits, a hundredth more
    /// does not.
    #[test]
    fn a_block_fits_up_to_the_last_hundredth() {
        assert!(room(68.0).fits(hundredths(7.0)));
        assert!(!room(68.0).fits(hundredths(7.01)));
        assert!(room(74.0).fits(hundredths(0.35)));
        assert!(!room(74.0).fits(hundredths(1.16)));
    }

    /// A path holding 53.05 took an Amp+Cab of 14.56 and 7.2, and refused
    /// one of 15.3 and 7.2, as a Floor did.
    #[test]
    fn an_amp_and_its_cab_count_together() {
        let pair = |amp: f32, cab: f32| hundredths(amp) + hundredths(cab);
        assert!(room(53.05).fits(pair(14.56, 7.2)));
        assert!(!room(53.05).fits(pair(15.3, 7.2)));
    }

    #[test]
    fn shares_read_as_whole_percentages_until_they_would_tie() {
        assert_eq!(percent(hundredths(22.5), 7500), "30%");
        assert_eq!(percent(hundredths(0.35), 7500), "<1%");
        assert_eq!(percent(0, 7500), "0%");
        // 22.5 needed, 22.4 left: both read 30% whole.
        assert_eq!(
            percents(hundredths(22.5), hundredths(22.4), 7500),
            ("30.0%".to_owned(), "29.9%".to_owned())
        );
    }

    /// A Helix Floor whose path 1 holds a stereo Gain and a stereo Vintage
    /// Digital, and whose path 2 holds an IR, in the real catalog's prices.
    fn floor() -> Option<App> {
        let (to_device, _cmds) = std::sync::mpsc::channel();
        let (_events, from_device) = std::sync::mpsc::channel();
        let mut app = App::new(&egui::Context::default(), to_device, from_device);
        let catalog = app.catalog.as_ref()?;
        let number = |symbol: &str| {
            catalog
                .symbols()
                .iter()
                .find(|s| s.symbol == symbol)
                .map(|s| s.number)
                .unwrap()
        };
        let block = |position: i64, kind: Kind, model: u32| session::Block {
            position,
            routing: None,
            kind,
            model,
            enabled: false,
            values: Vec::new(),
            paired: None,
            paired_values: Vec::new(),
        };
        app.chain = vec![
            block(0, Kind::Input, 0),
            block(1, Kind::Block, number("HD2_VolPanGainStereo")),
            block(2, Kind::Block, number("HD2_DelayVintageDigitalStereo")),
            block(9, Kind::Output, 0),
            block(20, Kind::Input, 0),
            block(21, Kind::Block, number("HD2_ImpulseResponse1024Mono")),
            block(29, Kind::Output, 0),
        ];
        app.layout = Layout {
            paths: vec![
                Path {
                    input: Some(0),
                    output: Some(9),
                    head: vec![1, 2],
                    ..Path::default()
                },
                Path {
                    input: Some(20),
                    output: Some(29),
                    head: vec![21],
                    ..Path::default()
                },
            ],
        };
        app.device = "Helix Floor".into();
        Some(app)
    }

    #[test]
    fn each_path_counts_its_own_blocks_bypassed_or_not() {
        let Some(app) = floor() else { return };
        assert_eq!(app.path_room(0).unwrap().used, 51 + 714);
        assert_eq!(app.path_room(1).unwrap().used, 250);
        // A gap on a path has that path's whole room.
        assert_eq!(app.room_at(5).unwrap().used, 765);
        assert_eq!(app.room_at(25).unwrap().path, 1);
    }

    #[test]
    fn a_swapped_block_frees_what_it_cost() {
        let Some(app) = floor() else { return };
        let room = app.room_at(2).unwrap();
        assert_eq!(room.used, 51);
        assert_eq!(room.left(), 7449);
    }

    #[test]
    fn a_device_never_measured_holds_nothing_back() {
        let Some(mut app) = floor() else { return };
        app.device = "HX Stomp".into();
        assert_eq!(app.path_room(0), None);
        assert_eq!(app.room_at(5), None);
    }
}
