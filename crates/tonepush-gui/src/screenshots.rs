//! The design screenshots: the whole window drawn offscreen, on the GPU, with
//! invented content.
//!
//! Every scene is synthetic. Preset, tone, setlist, capture and artist names
//! are made up; HX model and knob names come from HX Edit's catalog where it
//! is installed. No pedal is opened: the app is built with channels that lead
//! nowhere and the StompStation PRO panel does not look for hardware in test
//! builds.
//!
//! Ignored by default. To render, point the data directories at scratch
//! copies so nothing personal can appear (HX Edit's data without its
//! `icons_models` and `icons_category` folders keeps Line 6's artwork out),
//! send any request for the site nowhere, and run it on its own:
//!
//! ```sh
//! TONEPUSH_SCREENSHOTS=out \
//! TONEPUSH_LIBRARY=scratch/library TONEPUSH_BACKUPS=scratch/backups \
//! TONEPUSH_CONFIG=scratch/config.json HX_RESOURCES_DEST=scratch/hx-resources \
//! TONEPUSH_SITE=http://127.0.0.1:9 \
//! gpu-lock cargo test -p tonepush-gui --lib screenshots -- --ignored --test-threads=1
//! ```
//!
//! `TONEPUSH_SCREENSHOT_SCENES` (comma separated names),
//! `TONEPUSH_SCREENSHOT_SIZES` (`1280x760,2560x1440`) and
//! `TONEPUSH_SCREENSHOT_THEMES` (`dark,light`) narrow the run. The renderer
//! insists on a discrete GPU and says which one it used.

use std::sync::mpsc;
use std::sync::Arc;

use egui_kittest::Harness;

use crate::{session, shell, theme, App, Connection};

/// The sizes the design was drawn at: the smallest supported window, the
/// reference, and a large display at a scale of one.
const SIZES: [(u32, u32); 3] = [(1024, 640), (1280, 760), (2560, 1440)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// An HX Stomp with 01B Plexi Crunch loaded and its amp selected.
    HxEdit,
    /// A StompStation PRO with 03B Velvet Drive loaded.
    ProEdit,
    /// Nothing plugged in yet: TonePush looking for either family.
    NoDevice,
    /// The HX Stomp with a setlist about to be written, asking first.
    SetlistConfirm,
    /// The library's tones, one of them on its way to a slot.
    HxLibrary,
    /// The HX Stomp's own page, on its backups.
    HxPedal,
    /// The StompStation PRO's own page, on its backups.
    ProPedal,
    /// The HX Stomp with TonePush's settings open.
    Settings,
    /// The HX Stomp's page, on its impulse responses.
    HxPedalIrs,
    /// The HX Stomp's page, on its global EQ.
    HxPedalEq,
    /// The HX Stomp's page, on its settings.
    HxPedalSettings,
    /// The model browser on the drive's block, another drive being tried.
    HxBrowser,
    /// The Footswitches lens, on the switch that carries two things.
    HxFootswitches,
    /// The Snapshots lens.
    HxSnapshots,
    /// The library's setlists, one compared with the pedal bank by bank.
    HxSetlists,
    /// The StompStation PRO's NAM amps, the AC30 chosen.
    ProNam,
    /// The StompStation PRO's impulse responses, a stereo pair chosen.
    ProIrs,
    /// The StompStation PRO before a backup of it matches: saving waits.
    ProUnprotected,
    /// The StompStation PRO's own settings.
    ProSettings,
    /// A firmware update, backing the pedal up.
    ProFirmwareBackup,
    /// A firmware update, waiting for the pedal in Update Mode.
    ProFirmwareUpdateMode,
    /// A firmware update, asking before it writes.
    ProFirmwareConfirm,
    /// A firmware update, writing.
    ProFirmwareWriting,
    /// A firmware update, waiting for the restart.
    ProFirmwareRestart,
    /// A firmware update that stopped.
    ProFirmwareFailed,
    /// Nothing found on USB, with the way to look again.
    ConnectNotFound,
    /// A StompStation PRO, a tempo it does not take just typed.
    ProTempoRefused,
    /// A StompStation PRO on firmware 2.0.10 with the default chain.
    ProRouter,
    /// Firmware 2.0.10, the delay and the reverb in parallel.
    ProRouterParallel,
    /// Firmware 2.0.10, three blocks in parallel and free positions.
    ProRouterThreeWay,
    /// Firmware 2.0.10, a free position chosen to put a block in.
    ProRouterPicker,
    /// Firmware 2.2.6, not verified for saving: edits are live only.
    ProReadOnly,
    /// TonePush's public tones for the HX Stomp, in the library's Cloud.
    HxCloud,
    /// The HX Stomp's page, the device card's menu of its pages open.
    HxPages,
    /// A StompStation PRO on 2.0.10, on its NAM amps, the menu open.
    ProPages,
    /// Dream Pop auditioned from the library over 01B Plexi Crunch.
    HxAudition,
    /// The same, Keep's other choices open.
    HxAuditionKeep,
    /// Velvet Drive, a StompStation PRO tone, clicked with an HX Stomp.
    HxCannotPlay,
    /// Glass Wall auditioned on the StompStation PRO over 03B Velvet Drive.
    ProAudition,
    /// Dream Pop auditioned, two knobs turned since: the edits stay with it.
    HxAuditionEdited,
    /// Slapback Twang put in 05B, which holds Chime Clean: the question.
    HxPutAsk,
    /// Three tones put from 14C: the question for several.
    HxPutSeveral,
    /// Shimmer Lead auditioned on firmware 2.2.6, Keep's choices open.
    ProReadOnlyKeep,
    /// Glass Cathedral auditioned from TonePush over 01B Plexi Crunch.
    HxCloudAudition,
    /// The setlist's 05A, Shimmer Pad, auditioned from its page.
    HxSetlistAudition,
    /// No pedal: Dream Pop clicked, shown where the editor would be.
    NoDeviceGlimpse,
    /// Slapback Twang dragged over 05B Chime Clean.
    HxDragToneToSlot,
    /// The preset in 02C dragged onto the Tones tab.
    HxDragPresetToTones,
    /// A TonePush tone dragged over 07C.
    HxDragCloudToSlot,
    /// Three tones dragged over 05B.
    HxDragSeveral,
    /// A setlist dragged over the presets.
    HxDragSetlist,
    /// Dream Pop right-clicked: chosen without playing, its menu open.
    HxMenuTone,
    /// Glass Wall, a StompStation PRO tone, right-clicked with an HX Stomp.
    HxMenuOther,
    /// Three tones chosen, their menu open.
    HxMenuSeveral,
    /// Album release show's menu.
    HxMenuSetlist,
    /// One slot of Album release show, its menu open.
    HxMenuSlot,
    /// Glass Cathedral's menu, in the Cloud.
    HxMenuCloud,
    /// The loaded preset's menu, opened with Shift F10.
    HxMenuPreset,
    /// The StompStation PRO's loaded preset's menu.
    ProMenuPreset,
    /// Slapback Twang, on TonePush as v2, about to be published as v3.
    HxPublish,
    /// Three tones about to be published at once.
    HxPublishSeveral,
    /// Cloud, Mine: what this library published, Slapback Twang's menu open.
    HxMine,
    /// Cloud, Mine with the server's account: setlists and tones from any
    /// computer, Album release show chosen.
    HxMineAccount,
    /// Deleting Slapback Twang from TonePush: the question.
    HxMineDelete,
    /// Your tone's menu with the server's items: Hide, Delete.
    HxMineAccountMenu,
    /// Publishing for you alone, where the server can hide a tone.
    HxPublishAccount,
    /// Album release show about to go on TonePush, two of its tones not.
    HxPublishSetlist,
}

impl Scene {
    const ALL: [Scene; 67] = [
        Scene::HxEdit,
        Scene::ProEdit,
        Scene::NoDevice,
        Scene::SetlistConfirm,
        Scene::HxLibrary,
        Scene::HxPedal,
        Scene::ProPedal,
        Scene::Settings,
        Scene::HxPedalIrs,
        Scene::HxPedalEq,
        Scene::HxPedalSettings,
        Scene::HxBrowser,
        Scene::HxFootswitches,
        Scene::HxSnapshots,
        Scene::HxSetlists,
        Scene::ProNam,
        Scene::ProIrs,
        Scene::ProUnprotected,
        Scene::ProSettings,
        Scene::ProFirmwareBackup,
        Scene::ProFirmwareUpdateMode,
        Scene::ProFirmwareConfirm,
        Scene::ProFirmwareWriting,
        Scene::ProFirmwareRestart,
        Scene::ProFirmwareFailed,
        Scene::ConnectNotFound,
        Scene::ProTempoRefused,
        Scene::ProRouter,
        Scene::ProRouterParallel,
        Scene::ProRouterThreeWay,
        Scene::ProRouterPicker,
        Scene::ProReadOnly,
        Scene::HxCloud,
        Scene::HxPages,
        Scene::ProPages,
        Scene::HxAudition,
        Scene::HxAuditionKeep,
        Scene::HxCannotPlay,
        Scene::ProAudition,
        Scene::HxAuditionEdited,
        Scene::HxPutAsk,
        Scene::HxPutSeveral,
        Scene::ProReadOnlyKeep,
        Scene::HxCloudAudition,
        Scene::HxSetlistAudition,
        Scene::NoDeviceGlimpse,
        Scene::HxDragToneToSlot,
        Scene::HxDragPresetToTones,
        Scene::HxDragCloudToSlot,
        Scene::HxDragSeveral,
        Scene::HxDragSetlist,
        Scene::HxMenuTone,
        Scene::HxMenuOther,
        Scene::HxMenuSeveral,
        Scene::HxMenuSetlist,
        Scene::HxMenuSlot,
        Scene::HxMenuCloud,
        Scene::HxMenuPreset,
        Scene::ProMenuPreset,
        Scene::HxPublish,
        Scene::HxPublishSeveral,
        Scene::HxMine,
        Scene::HxMineAccount,
        Scene::HxMineDelete,
        Scene::HxMineAccountMenu,
        Scene::HxPublishAccount,
        Scene::HxPublishSetlist,
    ];

    fn name(self) -> &'static str {
        match self {
            Scene::HxEdit => "hx-edit",
            Scene::ProEdit => "pro-edit",
            Scene::NoDevice => "no-device",
            Scene::SetlistConfirm => "setlist-confirm",
            Scene::HxLibrary => "hx-library",
            Scene::HxPedal => "hx-pedal",
            Scene::ProPedal => "pro-pedal",
            Scene::Settings => "settings",
            Scene::HxPedalIrs => "hx-pedal-irs",
            Scene::HxPedalEq => "hx-pedal-eq",
            Scene::HxPedalSettings => "hx-pedal-settings",
            Scene::HxBrowser => "hx-browser",
            Scene::HxFootswitches => "hx-footswitches",
            Scene::HxSnapshots => "hx-snapshots",
            Scene::HxSetlists => "hx-setlists",
            Scene::ProNam => "pro-nam",
            Scene::ProIrs => "pro-irs",
            Scene::ProUnprotected => "pro-unprotected",
            Scene::ProSettings => "pro-settings",
            Scene::ProFirmwareBackup => "pro-firmware-backup",
            Scene::ProFirmwareUpdateMode => "pro-firmware-update-mode",
            Scene::ProFirmwareConfirm => "pro-firmware-confirm",
            Scene::ProFirmwareWriting => "pro-firmware-writing",
            Scene::ProFirmwareRestart => "pro-firmware-restart",
            Scene::ProFirmwareFailed => "pro-firmware-failed",
            Scene::ConnectNotFound => "connect-not-found",
            Scene::ProTempoRefused => "pro-tempo-refused",
            Scene::ProRouter => "pro-router",
            Scene::ProRouterParallel => "pro-router-parallel",
            Scene::ProRouterThreeWay => "pro-router-three-way",
            Scene::ProRouterPicker => "pro-router-picker",
            Scene::ProReadOnly => "pro-read-only",
            Scene::HxCloud => "hx-cloud",
            Scene::HxPages => "hx-pages",
            Scene::ProPages => "pro-pages",
            Scene::HxAudition => "hx-audition",
            Scene::HxAuditionKeep => "hx-audition-keep",
            Scene::HxCannotPlay => "hx-cannot-play",
            Scene::ProAudition => "pro-audition",
            Scene::HxAuditionEdited => "hx-audition-edited",
            Scene::HxPutAsk => "hx-put-ask",
            Scene::HxPutSeveral => "hx-put-several",
            Scene::ProReadOnlyKeep => "pro-read-only-keep",
            Scene::HxCloudAudition => "hx-cloud-audition",
            Scene::HxSetlistAudition => "hx-setlist-audition",
            Scene::NoDeviceGlimpse => "no-device-glimpse",
            Scene::HxDragToneToSlot => "hx-drag-tone-to-slot",
            Scene::HxDragPresetToTones => "hx-drag-preset-to-tones",
            Scene::HxDragCloudToSlot => "hx-drag-cloud-to-slot",
            Scene::HxDragSeveral => "hx-drag-several",
            Scene::HxDragSetlist => "hx-drag-setlist",
            Scene::HxMenuTone => "hx-menu-tone",
            Scene::HxMenuOther => "hx-menu-other-pedal",
            Scene::HxMenuSeveral => "hx-menu-several",
            Scene::HxMenuSetlist => "hx-menu-setlist",
            Scene::HxMenuSlot => "hx-menu-setlist-slot",
            Scene::HxMenuCloud => "hx-menu-cloud",
            Scene::HxMenuPreset => "hx-menu-preset",
            Scene::ProMenuPreset => "pro-menu-preset",
            Scene::HxPublish => "hx-publish",
            Scene::HxPublishSeveral => "hx-publish-several",
            Scene::HxMine => "hx-mine",
            Scene::HxMineAccount => "hx-mine-account",
            Scene::HxMineDelete => "hx-mine-delete",
            Scene::HxMineAccountMenu => "hx-mine-account-menu",
            Scene::HxPublishAccount => "hx-publish-account",
            Scene::HxPublishSetlist => "hx-publish-setlist",
        }
    }

    /// What is dragged in a drag scene, held as the payload every frame.
    fn dragged(self, app: &App) -> Option<crate::dnd::Dragged> {
        use crate::dnd::Dragged;
        match self {
            Scene::HxDragToneToSlot | Scene::HxDragSeveral => {
                let index = tone_named(app, "Slapback Twang");
                Some(app.dragged_tones(index))
            }
            Scene::HxDragPresetToTones => Some(Dragged::Preset(5)),
            Scene::HxDragCloudToSlot => Some(Dragged::Cloud(0)),
            Scene::HxDragSetlist => Some(Dragged::Setlist(0)),
            _ => None,
        }
    }

    /// The row menu a menu scene opens, once the row it belongs to is drawn.
    fn row_menu(self, app: &App) -> Option<crate::menus::MenuFor> {
        use crate::menus::MenuFor;
        match self {
            Scene::HxMenuTone => Some(MenuFor::Tones(vec![tone_named(app, "Dream Pop")])),
            Scene::HxMenuOther => Some(MenuFor::Tones(vec![tone_named(app, "Glass Wall")])),
            Scene::HxMenuSeveral => {
                Some(MenuFor::Tones(app.put_rows(tone_named(app, "Dream Pop"))))
            }
            Scene::HxMenuSetlist => Some(MenuFor::Setlist(0)),
            Scene::HxMenuSlot => Some(MenuFor::SetlistSlot {
                setlist: 0,
                slot: 4,
            }),
            Scene::HxMenuCloud => Some(MenuFor::Cloud(0)),
            Scene::HxMine | Scene::HxMineAccountMenu => app.mine.selected.map(MenuFor::Mine),
            _ => None,
        }
    }

    /// Where the pointer is in a drag scene, at 1280 by 760.
    fn pointer(self) -> Option<egui::Pos2> {
        match self {
            Scene::HxDragToneToSlot | Scene::HxDragSeveral => Some(egui::pos2(150.0, 484.0)),
            Scene::HxDragPresetToTones => Some(egui::pos2(272.0, 548.0)),
            Scene::HxDragCloudToSlot => Some(egui::pos2(150.0, 678.0)),
            Scene::HxDragSetlist => Some(egui::pos2(120.0, 360.0)),
            _ => None,
        }
    }

    fn stage(self, app: &mut App) {
        // Nothing goes to TonePush's site from a screenshot.
        app.cloud_search_due = None;
        app.cloud_check = None;
        // Signed in by name only, an invented one: there is no token, so
        // nothing could reach an account even if the site were there.
        app.config.account = Some("Noa Calder".to_owned());
        library(app);
        match self {
            Scene::HxEdit => {
                // The design's main screen: the library on every pedal's
                // tones, with nothing chosen, so the details are the loaded
                // preset's, scrolled to its row.
                hx_stomp(app);
                app.library_device_filter = None;
                app.lib_reveal = true;
            }
            Scene::Settings => hx_stomp(app),
            Scene::ProEdit => pro(app),
            Scene::SetlistConfirm | Scene::HxSetlists => {
                hx_stomp(app);
                app.lib_showing = crate::LibraryView::Setlists;
                app.select_setlist_entry(0);
                tall_pane(app);
                if self == Scene::SetlistConfirm {
                    app.confirm_push = Some(0);
                }
            }
            Scene::HxLibrary => {
                hx_stomp(app);
                // Every pedal's tones, so the markers of each show.
                app.library_device_filter = None;
                let sending = app
                    .lib_entries
                    .iter()
                    .position(|entry| entry.name == "Slapback Twang")
                    .expect("the library holds Slapback Twang");
                app.select_lib_entry(sending);
                app.sending = Some(crate::put::Sending::one(
                    app.lib_entries[sending].hash.clone(),
                    app.lib_entries[sending].name.clone(),
                ));
            }
            Scene::HxCloud => {
                hx_stomp(app);
                cloud_feed(app);
                app.lib_showing = crate::LibraryView::Cloud;
                app.cloud_selected = Some(0);
            }
            Scene::HxPedal | Scene::HxPages => {
                hx_stomp(app);
                backups_history(app);
                app.page = shell::Page::Pedal;
            }
            Scene::HxAudition | Scene::HxAuditionKeep | Scene::HxAuditionEdited => {
                hx_stomp(app);
                // Every pedal's tones, as the sheet draws them.
                app.library_device_filter = None;
                auditioning(app, "Dream Pop");
                dream_pop_chain(app);
                if self == Scene::HxAuditionEdited {
                    // Two knobs turned on the tone while it plays.
                    app.undo_depth = 2;
                    app.dirty = true;
                }
            }
            Scene::HxCannotPlay => {
                hx_stomp(app);
                app.library_device_filter = None;
                let index = app
                    .lib_entries
                    .iter()
                    .position(|entry| entry.name == "Velvet Drive")
                    .expect("the library holds Velvet Drive");
                app.choose_tone(index);
                app.audition_library(index);
                app.lib_reveal = true;
            }
            Scene::ProAudition => {
                pro(app);
                auditioning(app, "Glass Wall");
            }
            Scene::HxPutAsk => {
                hx_stomp(app);
                app.library_device_filter = None;
                let index = tone_named(app, "Slapback Twang");
                app.choose_tone(index);
                app.lib_reveal = true;
                let tones = [(
                    app.lib_entries[index].hash.clone(),
                    app.lib_entries[index].name.clone(),
                )];
                app.put_question = Some(crate::put::Asking {
                    writes: app.plan_put(13, &tones),
                    beside: Some(13),
                    back_up_first: false,
                    clears: None,
                });
            }
            Scene::HxPutSeveral => {
                hx_stomp(app);
                app.library_device_filter = None;
                let chosen: Vec<usize> = ["Brown Lead", "Doom Fuzz", "Dream Pop"]
                    .iter()
                    .map(|name| tone_named(app, name))
                    .collect();
                app.choose_tone(chosen[0]);
                app.lib_chosen = chosen
                    .iter()
                    .map(|&index| app.lib_entries[index].hash.clone())
                    .collect();
                let tones: Vec<(String, String)> = chosen
                    .iter()
                    .map(|&index| {
                        (
                            app.lib_entries[index].hash.clone(),
                            app.lib_entries[index].name.clone(),
                        )
                    })
                    .collect();
                app.put_question = Some(crate::put::Asking {
                    writes: app.plan_put(41, &tones),
                    beside: None,
                    back_up_first: false,
                    clears: None,
                });
            }
            Scene::ProReadOnlyKeep => {
                use crate::pro::demo::DemoChain;
                pro(app);
                app.pro.show_demo_router(DemoChain::Parallel);
                app.pro.demo_read_only();
                app.library_device_filter = None;
                auditioning(app, "Shimmer Lead");
            }
            Scene::HxCloudAudition => {
                hx_stomp(app);
                cloud_feed(app);
                app.lib_showing = crate::LibraryView::Cloud;
                app.cloud_selected = Some(0);
                app.hearing.arrows = crate::audition::Arrows::Cloud;
                let (id, name) = {
                    let tone = &app.cloud_entries[0].discovered.tone.summary;
                    (tone.id, tone.name.clone())
                };
                app.begin_hearing(id, crate::audition::Source::TonePush(id), name);
                answered(app, id);
                chain_of(
                    app,
                    &[
                        ("Deluxe Comp", "Dynamics"),
                        ("US Princess", "Amp"),
                        ("1x12 Blue Bell", "Cab"),
                        ("PlastiChorus", "Modulation"),
                        ("Simple Delay", "Delay"),
                        ("Glitz", "Reverb"),
                    ],
                    &["Verse", "Chorus", "Swell"],
                    84.0,
                );
            }
            Scene::HxSetlistAudition => {
                hx_stomp(app);
                app.lib_showing = crate::LibraryView::Setlists;
                app.select_setlist_entry(0);
                // As tall as the sheet's, the bar counted.
                for (size, height) in [("small", 254.0), ("medium", 300.0), ("large", 714.0)] {
                    app.config.library_pane.insert(
                        size.to_owned(),
                        crate::config::PaneSize {
                            height,
                            folded: false,
                        },
                    );
                }
                let (name, tone) = {
                    let (_, setlist) = &app.lib_setlists[0];
                    (setlist.name.clone(), setlist.slots[12].clone())
                };
                app.audition_setlist_slot(&name, 12, &tone);
                let key = app
                    .hearing
                    .heard
                    .as_ref()
                    .map(|heard| heard.key)
                    .expect("the slot was asked to play");
                answered(app, key);
                chain_of(
                    app,
                    &[
                        ("Deluxe Comp", "Dynamics"),
                        ("Simple Pitch", "Pitch/Synth"),
                        ("Essex A30", "Amp"),
                        ("Adriatic Swell", "Delay"),
                        ("Searchlights", "Reverb"),
                    ],
                    &["Pad", "Swell", "Dry"],
                    72.0,
                );
            }
            Scene::NoDeviceGlimpse => {
                app.connection = Connection::Connecting;
                app.status.clear();
                app.library_device_filter = None;
                // Dream Pop's chain, as reading its document would give it.
                chain_of(
                    app,
                    &[
                        ("Optical Trem", "Modulation"),
                        ("US Deluxe Nrm", "Amp"),
                        ("1x12 US Deluxe", "Cab"),
                        ("Adriatic Delay", "Delay"),
                        ("Ganymede", "Reverb"),
                    ],
                    &["Clean", "Wide", "Lead"],
                    92.0,
                );
                let index = tone_named(app, "Dream Pop");
                app.choose_tone(index);
                app.lib_reveal = true;
                let preview = crate::Preview {
                    name: "Dream Pop".to_owned(),
                    line: "Full rig".to_owned(),
                    chain: std::mem::take(&mut app.chain),
                    layout: std::mem::take(&mut app.layout),
                    skipped: Vec::new(),
                    load: crate::LoadKind::Document(Vec::new()),
                    dest: 0,
                    source: ("hxpreset".to_owned(), Vec::new()),
                };
                app.hearing.glimpse = Some(crate::audition::Glimpse {
                    preview,
                    marker: app.lib_entries[index].marker.clone(),
                    from: "From your library".to_owned(),
                });
                app.hearing.strip = Some(crate::audition::Strip {
                    icon: crate::theme::Icon::Usb,
                    words: vec![
                        ("No pedal is connected, so ".to_owned(), false),
                        ("Dream Pop".to_owned(), true),
                        (
                            " is shown, not played. Plug in your HX Stomp and the next click \
                             plays."
                                .to_owned(),
                            false,
                        ),
                    ],
                    narrow: None,
                    open: None,
                });
            }
            Scene::HxDragToneToSlot | Scene::HxDragSeveral => {
                hx_stomp(app);
                app.library_device_filter = None;
                let index = tone_named(app, "Slapback Twang");
                app.choose_tone(index);
                app.lib_reveal = true;
                if self == Scene::HxDragSeveral {
                    app.lib_chosen = ["Slapback Twang", "Surf Spring", "Tape Echo Clean"]
                        .iter()
                        .map(|name| app.lib_entries[tone_named(app, name)].hash.clone())
                        .collect();
                }
            }
            Scene::HxDragPresetToTones => {
                hx_stomp(app);
                app.library_device_filter = None;
            }
            Scene::HxDragCloudToSlot => {
                hx_stomp(app);
                cloud_feed(app);
                app.lib_showing = crate::LibraryView::Cloud;
                app.cloud_selected = Some(0);
            }
            Scene::HxMenuTone | Scene::HxMenuOther | Scene::HxMenuSeveral => {
                hx_stomp(app);
                app.library_device_filter = None;
                let name = if self == Scene::HxMenuOther {
                    "Glass Wall"
                } else {
                    "Dream Pop"
                };
                let index = tone_named(app, name);
                app.choose_tone(index);
                app.lib_reveal = true;
                if self == Scene::HxMenuSeveral {
                    app.lib_chosen = ["Dream Pop", "Slapback Twang", "Surf Spring"]
                        .iter()
                        .map(|name| app.lib_entries[tone_named(app, name)].hash.clone())
                        .collect();
                }
            }
            Scene::HxMenuSetlist | Scene::HxMenuSlot => {
                hx_stomp(app);
                app.lib_showing = crate::LibraryView::Setlists;
                app.select_setlist_entry(0);
                app.hearing.arrows = crate::audition::Arrows::Setlists;
                tall_pane(app);
            }
            Scene::HxMenuCloud => {
                hx_stomp(app);
                cloud_feed(app);
                app.lib_showing = crate::LibraryView::Cloud;
                app.cloud_selected = Some(0);
                app.hearing.arrows = crate::audition::Arrows::Cloud;
            }
            Scene::HxMenuPreset => {
                hx_stomp(app);
                app.clipboard = Some(("Chime Clean".to_owned(), vec![0; 4]));
            }
            Scene::ProMenuPreset => pro(app),
            Scene::HxPublish | Scene::HxPublishSeveral => {
                hx_stomp(app);
                app.library_device_filter = None;
                mine_fixture(app);
                app.lib_showing = crate::LibraryView::Tones;
                let index = tone_named(app, "Slapback Twang");
                app.choose_tone(index);
                app.lib_reveal = true;
                let hashes = if self == Scene::HxPublish {
                    vec![app.lib_entries[index].hash.clone()]
                } else {
                    ["Slapback Twang", "Surf Spring", "Tape Echo Clean"]
                        .iter()
                        .map(|name| app.lib_entries[tone_named(app, name)].hash.clone())
                        .collect()
                };
                app.ask_to_publish(hashes);
            }
            Scene::HxMine => {
                hx_stomp(app);
                app.library_device_filter = None;
                mine_fixture(app);
                tall_pane(app);
            }
            Scene::HxMineAccount | Scene::HxMineDelete | Scene::HxMineAccountMenu => {
                hx_stomp(app);
                app.library_device_filter = None;
                mine_fixture(app);
                account_fixture(app);
                tall_pane(app);
                if self == Scene::HxMineAccount {
                    app.mine.selected_setlist = app
                        .account
                        .setlists
                        .as_ref()
                        .and_then(|setlists| setlists.first())
                        .map(|setlist| setlist.id);
                }
                if self == Scene::HxMineDelete {
                    app.account.confirm_delete = app.mine.selected.map(crate::account::Yours::Tone);
                }
            }
            Scene::HxPublishAccount => {
                hx_stomp(app);
                app.library_device_filter = None;
                mine_fixture(app);
                account_fixture(app);
                app.lib_showing = crate::LibraryView::Tones;
                let index = tone_named(app, "Surf Spring");
                app.choose_tone(index);
                app.lib_reveal = true;
                app.ask_to_publish(vec![app.lib_entries[index].hash.clone()]);
                app.publish_visibility = crate::cloud::Visibility::OnlyYou;
            }
            Scene::HxPublishSetlist => {
                hx_stomp(app);
                mine_fixture(app);
                account_fixture(app);
                app.lib_showing = crate::LibraryView::Setlists;
                app.select_setlist_entry(0);
                tall_pane(app);
                app.publish_ask = Some(crate::publish::Asked::Setlist(0));
            }
            Scene::HxDragSetlist => {
                hx_stomp(app);
                app.lib_showing = crate::LibraryView::Setlists;
                app.select_setlist_entry(0);
                tall_pane(app);
            }
            Scene::ProPages => {
                use crate::pro::demo::DemoChain;
                pro(app);
                app.pro.show_demo_router(DemoChain::Default);
                app.page = shell::Page::Pedal;
                app.pro.demo_library(false);
            }
            Scene::HxPedalIrs | Scene::HxPedalEq | Scene::HxPedalSettings => {
                hx_stomp(app);
                app.page = shell::Page::Pedal;
                app.pedal_tab = match self {
                    Scene::HxPedalIrs => crate::pages::PedalTab::Irs,
                    Scene::HxPedalEq => crate::pages::PedalTab::Eq,
                    _ => crate::pages::PedalTab::Settings,
                };
            }
            Scene::ProPedal => {
                pro(app);
                app.page = shell::Page::Pedal;
                app.pro.demo_page();
            }
            Scene::ProNam | Scene::ProIrs => {
                pro(app);
                app.page = shell::Page::Pedal;
                app.pro.demo_library(self == Scene::ProIrs);
            }
            Scene::ProTempoRefused => {
                pro(app);
                app.pro.demo_tempo_refused();
            }
            Scene::ProRouter
            | Scene::ProRouterParallel
            | Scene::ProRouterThreeWay
            | Scene::ProRouterPicker => {
                use crate::pro::demo::DemoChain;
                pro(app);
                app.pro.show_demo_router(match self {
                    Scene::ProRouterParallel => DemoChain::Parallel,
                    Scene::ProRouterThreeWay => DemoChain::ThreeWay,
                    Scene::ProRouterPicker => DemoChain::Picking,
                    _ => DemoChain::Default,
                });
            }
            Scene::ProReadOnly => {
                use crate::pro::demo::DemoChain;
                pro(app);
                app.pro.show_demo_router(DemoChain::Parallel);
                app.pro.demo_read_only();
            }
            Scene::ProUnprotected => {
                pro(app);
                app.pro.demo_unprotected();
            }
            Scene::ProSettings => {
                pro(app);
                app.page = shell::Page::Pedal;
                app.pro.demo_settings();
            }
            Scene::ProFirmwareBackup
            | Scene::ProFirmwareUpdateMode
            | Scene::ProFirmwareConfirm
            | Scene::ProFirmwareWriting
            | Scene::ProFirmwareRestart
            | Scene::ProFirmwareFailed => {
                pro(app);
                app.page = shell::Page::Pedal;
                let step = [
                    Scene::ProFirmwareBackup,
                    Scene::ProFirmwareUpdateMode,
                    Scene::ProFirmwareConfirm,
                    Scene::ProFirmwareWriting,
                    Scene::ProFirmwareRestart,
                    Scene::ProFirmwareFailed,
                ]
                .iter()
                .position(|scene| *scene == self)
                .unwrap_or_default();
                app.pro.demo_firmware(step);
            }
            Scene::HxBrowser => {
                hx_stomp(app);
                trying(app);
            }
            Scene::HxFootswitches => {
                hx_stomp(app);
                app.lens = crate::pane::Lens::Footswitches;
                app.focused_source = Some(hx_proto::rpc::Source::Footswitch(2));
            }
            Scene::HxSnapshots => {
                hx_stomp(app);
                app.lens = crate::pane::Lens::Snapshots;
            }
            Scene::NoDevice => {
                app.connection = Connection::Connecting;
                app.status.clear();
            }
            Scene::ConnectNotFound => {
                app.connection = Connection::Offline;
                app.status =
                    "No supported pedal found. Check USB and close any other pedal editor."
                        .to_owned();
                app.pro.demo_not_found();
                // Without HX Edit's data, as on a first run.
                app.catalog = None;
            }
        }
        if app.pro_active() {
            kept_from_pro(app);
        }
    }
}

/// The row of a fixture tone, by its name.
fn tone_named(app: &App, name: &str) -> usize {
    app.lib_entries
        .iter()
        .position(|entry| entry.name == name)
        .expect("the library holds the tone")
}

/// A library tone auditioned with a click, the pedal answering that it
/// plays: the loaded preset set aside with its unsaved changes and its
/// history, as the worker would say.
fn auditioning(app: &mut App, name: &str) {
    let index = app
        .lib_entries
        .iter()
        .position(|entry| entry.name == name)
        .expect("the library holds the tone");
    app.choose_tone(index);
    app.lib_reveal = true;
    app.audition_library(index);
    let key = app
        .hearing
        .heard
        .as_ref()
        .map(|heard| heard.key)
        .expect("the tone was asked to play");
    app.audition_event(Some(key));
    app.put_aside_dirty = true;
    app.dirty = false;
    app.undo_depth = 0;
    app.redo_depth = 0;
    app.pro.demo_history_set_aside();
    app.preset_name = name.to_owned();
}

/// The pedal answering that the tone asked for plays, setting the loaded
/// preset aside with its unsaved changes and its history.
fn answered(app: &mut App, key: i64) {
    app.audition_event(Some(key));
    app.put_aside_dirty = true;
    app.dirty = false;
    app.undo_depth = 0;
    app.redo_depth = 0;
    app.pro.demo_history_set_aside();
}

/// A chain of blocks by model name and category, one path, the amp chosen,
/// with its snapshots and tempo.
fn chain_of(app: &mut App, blocks: &[(&str, &str)], snapshots: &[&str], tempo: f32) {
    use hx_proto::preset::{Kind, Layout, Path};
    let Some(catalog) = app.catalog.as_ref() else {
        return;
    };
    let number = |name: &str, category: &str| {
        catalog
            .models()
            .find(|model| {
                model.name == name
                    && catalog
                        .category_of(&model.id)
                        .and_then(|id| catalog.category(id))
                        .is_some_and(|found| found.name == category)
            })
            .or_else(|| catalog.models().find(|model| model.name == name))
            .and_then(|model| crate::number_of(catalog, &model.id))
            .unwrap_or(0)
    };
    let defaults = |model: u32| -> Vec<f32> {
        catalog
            .model_number(model)
            .map(|model| {
                catalog
                    .ordered_params(model)
                    .iter()
                    .map(|p| p.default)
                    .collect()
            })
            .unwrap_or_default()
    };
    let block = |position: i64, kind: Kind, model: u32| session::Block {
        position,
        routing: matches!(kind, Kind::Input | Kind::Output).then_some(0),
        kind,
        model,
        enabled: true,
        values: defaults(model),
        paired: None,
        paired_values: Vec::new(),
    };
    let routing = |symbol: &str, param: &str| {
        catalog
            .model(symbol)
            .and_then(|model| model.params.iter().find(|p| p.id == param))
            .and_then(|param| catalog.choices(param))
            .and_then(|choices| {
                choices
                    .iter()
                    .position(|choice| choice.starts_with("Multi"))
            })
            .map(|index| index as i64)
    };
    let endpoint = |position: i64, kind: Kind, symbol: &str, param: &str| session::Block {
        routing: routing(symbol, param).or(Some(0)),
        ..block(position, kind, 0)
    };
    let input = app.chain.iter().find(|b| b.position == 0).cloned();
    let output = app.chain.iter().find(|b| b.position == 9).cloned();
    let mut chain =
        vec![input
            .unwrap_or_else(|| endpoint(0, Kind::Input, "HelixStomp_AppDSPFlowInput", "@input"))];
    for (offset, (name, category)) in blocks.iter().enumerate() {
        chain.push(block(
            offset as i64 + 1,
            Kind::Block,
            number(name, category),
        ));
    }
    chain.push(output.unwrap_or_else(|| {
        endpoint(
            9,
            Kind::Output,
            "HelixStomp_AppDSPFlowOutputMain",
            "@output",
        )
    }));
    let amp = blocks
        .iter()
        .position(|(_, category)| *category == "Amp")
        .map_or(1, |at| at + 1);
    app.chain = chain;
    app.layout = Layout {
        paths: vec![Path {
            input: Some(0),
            output: Some(9),
            split: None,
            join: None,
            head: (1..=blocks.len()).collect(),
            lanes: Vec::new(),
            tail: Vec::new(),
        }],
    };
    app.selected = amp;
    app.assignments.clear();
    app.switches.clear();
    app.tempo = Some(tempo);
    app.snapshots = snapshots.iter().map(|name| (*name).to_owned()).collect();
    app.current_snapshot = 0;
    app.snapshot_details.clear();
}

/// Dream Pop on the pedal while it is auditioned, as sheet 02 draws it: a
/// tremolo, the Deluxe and its cab, a delay and a reverb, three snapshots at
/// 92 BPM.
fn dream_pop_chain(app: &mut App) {
    use hx_proto::preset::{Kind, Layout, Path};
    let Some(catalog) = app.catalog.as_ref() else {
        return;
    };
    let number = |name: &str, category: &str| {
        catalog
            .models()
            .find(|model| {
                model.name == name
                    && catalog
                        .category_of(&model.id)
                        .and_then(|id| catalog.category(id))
                        .is_some_and(|found| found.name == category)
            })
            .or_else(|| catalog.models().find(|model| model.name == name))
            .and_then(|model| crate::number_of(catalog, &model.id))
            .unwrap_or(0)
    };
    let defaults = |model: u32| -> Vec<f32> {
        catalog
            .model_number(model)
            .map(|model| {
                catalog
                    .ordered_params(model)
                    .iter()
                    .map(|p| p.default)
                    .collect()
            })
            .unwrap_or_default()
    };
    let block = |position: i64, kind: Kind, model: u32| session::Block {
        position,
        routing: matches!(kind, Kind::Input | Kind::Output).then_some(0),
        kind,
        model,
        enabled: true,
        values: defaults(model),
        paired: None,
        paired_values: Vec::new(),
    };
    let trem = number("Optical Trem", "Modulation");
    let amp = number("US Deluxe Nrm", "Amp");
    let cab = number("1x12 US Deluxe", "Cab");
    let delay = number("Adriatic Delay", "Delay");
    let reverb = number("Ganymede", "Reverb");
    let input = app.chain.iter().find(|b| b.position == 0).cloned();
    let output = app.chain.iter().find(|b| b.position == 9).cloned();
    app.chain = vec![
        input.unwrap_or_else(|| block(0, Kind::Input, 0)),
        block(1, Kind::Block, trem),
        block(2, Kind::Block, amp),
        block(3, Kind::Block, cab),
        block(4, Kind::Block, delay),
        block(5, Kind::Block, reverb),
        output.unwrap_or_else(|| block(9, Kind::Output, 0)),
    ];
    app.layout = Layout {
        paths: vec![Path {
            input: Some(0),
            output: Some(9),
            split: None,
            join: None,
            head: vec![1, 2, 3, 4, 5],
            lanes: Vec::new(),
            tail: Vec::new(),
        }],
    };
    app.selected = 2;
    app.assignments.clear();
    app.tempo = Some(92.0);
    app.snapshots = vec!["Clean".into(), "Wide".into(), "Lead".into()];
    app.current_snapshot = 0;
    app.snapshot_details.clear();
    let carried = |block: i64, name: &str, colour: i64| hx_usb::Carried {
        block,
        name: name.to_owned(),
        colour: Some(colour),
        enabled: true,
    };
    app.switches = vec![
        hx_usb::Switch {
            switch: 1,
            momentary: false,
            label: None,
            colour: None,
            carries: vec![carried(1, "Optical Trem", 0x2080ff)],
        },
        hx_usb::Switch {
            switch: 2,
            momentary: false,
            label: None,
            colour: None,
            carries: vec![carried(4, "Adriatic Delay", 0x00cc00)],
        },
        hx_usb::Switch {
            switch: 3,
            momentary: false,
            label: None,
            colour: None,
            carries: vec![carried(5, "Ganymede", 0x00c0c0)],
        },
    ];
}

/// The tone loaded on the StompStation PRO was kept from it, so it carries
/// the firmware the scene's pedal runs: 1.5.12, or 2.0.10 with the chain.
fn kept_from_pro(app: &mut App) {
    let firmware = app.pro.firmware().to_owned();
    if let Some(entry) = app
        .lib_entries
        .iter_mut()
        .find(|entry| entry.name == "Velvet Drive")
    {
        entry.marker = crate::devices::Marker::of_device("StompStation PRO", &firmware);
        entry.firmware.clone_from(&firmware);
        entry.meta.firmware = firmware;
    }
}

/// A chain written a letter per block, as the fixture's tones write it: W
/// wah, D drive, Y dynamics, E EQ, M modulation, L delay, R reverb, P pitch,
/// F filter, A amp, C cab, I impulse response; lower case when off, and a
/// parallel stretch in brackets.
fn fixture_chain(code: &str) -> Vec<shell::Mini> {
    let block = |letter: char| {
        let category = match letter.to_ascii_uppercase() {
            'V' => "Volume/Pan",
            'W' => "Wah",
            'D' => "Distortion",
            'Y' => "Dynamics",
            'E' => "EQ",
            'M' => "Modulation",
            'L' => "Delay",
            'R' => "Reverb",
            'P' => "Pitch/Synth",
            'F' => "Filter",
            'A' => "Amp",
            'C' => "Cab",
            'I' => "IR",
            _ => "",
        };
        (
            theme::category_colour(category),
            letter.is_ascii_uppercase(),
        )
    };
    let mut chain = Vec::new();
    let mut stack: Option<Vec<(egui::Color32, bool)>> = None;
    for letter in code.chars() {
        match letter {
            '[' => stack = Some(Vec::new()),
            ']' => {
                if let Some(lanes) = stack.take() {
                    chain.push(shell::Mini::Stack(lanes));
                }
            }
            letter => match stack.as_mut() {
                Some(lanes) => lanes.push(block(letter)),
                None => {
                    let (colour, on) = block(letter);
                    chain.push(shell::Mini::Block { colour, on });
                }
            },
        }
    }
    chain
}

/// The model browser open on Minotaur's block, with Teemah! playing in its
/// place, and a few models already in Recent.
fn trying(app: &mut App) {
    app.selected = 2;
    app.open_browser_swap();
    let Some(catalog) = app.catalog.as_ref() else {
        return;
    };
    let named = |name: &str| {
        catalog
            .models()
            .find(|model| model.name == name)
            .map(|model| model.id.clone())
    };
    app.config.recent_models = [
        "Teemah!",
        "Plateaux",
        "Transistor Tape",
        "US Double Nrm",
        "Heir Apparent",
        "Minotaur",
    ]
    .iter()
    .filter_map(|name| named(name))
    .collect();
    let Some(id) = named("Teemah!") else {
        return;
    };
    let Some(model) = catalog.model(&id) else {
        return;
    };
    let number = crate::number_of(catalog, &id);
    let values: Vec<f32> = catalog
        .ordered_params(model)
        .iter()
        .map(|param| param.default)
        .collect();
    if let (Some(number), Some(block)) = (number, app.chain.iter_mut().find(|b| b.position == 2)) {
        block.model = number;
        block.values = values;
    }
    if let Some(browser) = app.browser.as_mut() {
        browser.playing = Some((id, "Teemah!".to_owned()));
    }
}

/// Today at 14:02, when every invented backup was taken.
fn two_minutes_past_two() -> std::time::SystemTime {
    jiff::Zoned::now()
        .with()
        .hour(14)
        .minute(2)
        .second(0)
        .build()
        .map(|time| std::time::SystemTime::from(time.timestamp()))
        .expect("today at 14:02 is a time")
}

/// The library pane dragged up toward the board, as the design draws the
/// setlists: the block pane keeps its head.
fn tall_pane(app: &mut App) {
    for (size, height) in [("small", 300.0), ("medium", 345.0), ("large", 760.0)] {
        app.config.library_pane.insert(
            size.to_owned(),
            crate::config::PaneSize {
                height,
                folded: false,
            },
        );
    }
}

/// The StompStation PRO, with the library holding three of its presets.
fn pro(app: &mut App) {
    app.pro.show_demo();
    app.library_connected_device = "StompStation PRO".to_owned();
    app.library_device_filter = Some("StompStation PRO".to_owned());
    let marks = [(1, "Glass Wall"), (7, "Velvet Drive"), (8, "Shimmer Lead")]
        .into_iter()
        .filter_map(|(slot, name)| {
            app.lib_entries
                .iter()
                .find(|entry| entry.name == name)
                .map(|entry| (slot, entry.hash.clone()))
        })
        .collect();
    app.pro.demo_library_marks(marks);
}

/// The app for one scene, built on the harness's own context the first time
/// it draws, so fonts, icons and styles are installed where they are used.
struct Demo {
    scene: Scene,
    appearance: theme::Appearance,
    app: Option<App>,
    /// Frames drawn, for a menu that opens once its row has been drawn.
    frames: usize,
}

impl Demo {
    fn frame(&mut self, ui: &mut egui::Ui) {
        let Some(app) = self.app.as_mut() else {
            // The fonts installed here are bound from the next frame, which
            // is when drawing starts, exactly as eframe builds the app before
            // its first frame.
            egui_extras::install_image_loaders(ui.ctx());
            let (to_device, _nowhere) = mpsc::channel();
            let (_silent, from_device) = mpsc::channel();
            let mut app = App::new(ui.ctx(), to_device, from_device);
            theme::choose(ui.ctx(), self.appearance);
            app.config.appearance = self.appearance;
            self.scene.stage(&mut app);
            self.app = Some(app);
            return;
        };
        // The harness frames its ui with a margin; eframe hands the app the
        // whole window, so draw into a ui that covers all of it.
        let window = ui.ctx().content_rect();
        let mut root = ui.new_child(egui::UiBuilder::new().max_rect(window));
        root.set_clip_rect(window);
        if self.scene == Scene::Settings {
            egui::Popup::open_id(ui.ctx(), shell::settings_popup());
        }
        if matches!(self.scene, Scene::HxPages | Scene::ProPages) {
            egui::Popup::open_id(ui.ctx(), shell::device_card_menu());
        }
        if let Some(dragged) = self.scene.dragged(app) {
            egui::DragAndDrop::set_payload(ui.ctx(), dragged);
        }
        if matches!(self.scene, Scene::HxAuditionKeep | Scene::ProReadOnlyKeep) {
            egui::Popup::open_id(ui.ctx(), crate::audition::keep_menu());
        }
        self.frames += 1;
        if let Some(what) = self.scene.row_menu(app) {
            if self.frames > 3 && app.row_menu.is_none() {
                // Under the row, where Shift F10 opens it; a slot of a
                // setlist has no keyboard of its own, so where it is drawn.
                let at = match self.scene {
                    Scene::HxMenuSlot => egui::pos2(900.0, 708.0),
                    // Where the sheet's pointer is, over the table's right.
                    Scene::HxMine | Scene::HxMineAccountMenu => egui::pos2(533.0, 470.0),
                    _ => app.menu_anchor.unwrap_or(egui::pos2(400.0, 600.0)),
                };
                app.open_row_menu(ui.ctx(), what, at);
            }
        }
        if self.scene == Scene::HxMenuPreset {
            app.preset_menu_by_key = true;
        }
        if self.scene == Scene::ProMenuPreset {
            app.pro.menu_by_key = true;
        }
        app.draw(&mut root);
    }
}

/// A renderer on the machine's discrete GPU, refusing a software rasterizer.
fn gpu_renderer() -> egui_kittest::wgpu::WgpuTestRenderer {
    use egui_wgpu::wgpu;
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = wgpu::Backends::VULKAN;
    setup.native_adapter_selector = Some(Arc::new(|adapters, _surface| {
        adapters
            .iter()
            .find(|adapter| adapter.get_info().device_type == wgpu::DeviceType::DiscreteGpu)
            .cloned()
            .ok_or_else(|| "no discrete GPU to render the screenshots on".to_owned())
    }));
    let state = egui_kittest::wgpu::create_render_state(
        egui_wgpu::WgpuSetup::CreateNew(setup),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    let info = state.adapter.get_info();
    assert_ne!(
        info.device_type,
        wgpu::DeviceType::Cpu,
        "{} is a software rasterizer",
        info.name
    );
    eprintln!("rendering on {} ({:?})", info.name, info.backend);
    egui_kittest::wgpu::WgpuTestRenderer::from_render_state(state)
}

/// Save a frame as an opaque PNG at the encoder's best lossless compression:
/// these are committed, so every kilobyte is kept for good.
fn write_png(path: &std::path::Path, image: image::RgbaImage) {
    use image::ImageEncoder;
    let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("the PNG is created"));
    image::codecs::png::PngEncoder::new_with_quality(
        file,
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )
    .expect("the PNG is written");
}

fn wanted<T: Copy>(variable: &str, all: &[T], parse: impl Fn(&str) -> Option<T>) -> Vec<T> {
    match std::env::var(variable) {
        Ok(list) if !list.trim().is_empty() => list
            .split(',')
            .filter_map(|item| parse(item.trim()))
            .collect(),
        _ => all.to_vec(),
    }
}

#[test]
#[ignore = "renders PNGs on the GPU; see the module documentation"]
fn screenshots() {
    let Some(out) = std::env::var_os("TONEPUSH_SCREENSHOTS").map(std::path::PathBuf::from) else {
        panic!("set TONEPUSH_SCREENSHOTS to the directory the PNGs go in");
    };
    for variable in ["TONEPUSH_LIBRARY", "TONEPUSH_BACKUPS", "TONEPUSH_CONFIG"] {
        assert!(
            std::env::var_os(variable).is_some(),
            "set {variable} to a scratch location, so no personal data is drawn"
        );
    }
    std::fs::create_dir_all(&out).expect("the output directory");
    let scenes = wanted("TONEPUSH_SCREENSHOT_SCENES", &Scene::ALL, |name| {
        Scene::ALL.into_iter().find(|scene| scene.name() == name)
    });
    let sizes = wanted("TONEPUSH_SCREENSHOT_SIZES", &SIZES, |size| {
        let (width, height) = size.split_once('x')?;
        Some((width.parse().ok()?, height.parse().ok()?))
    });
    let themes = wanted(
        "TONEPUSH_SCREENSHOT_THEMES",
        &[theme::Appearance::Dark, theme::Appearance::Light],
        |name| match name {
            "dark" => Some(theme::Appearance::Dark),
            "light" => Some(theme::Appearance::Light),
            _ => None,
        },
    );
    for scene in scenes {
        for &(width, height) in &sizes {
            for &appearance in &themes {
                let mut harness = Harness::builder()
                    .with_size(egui::vec2(width as f32, height as f32))
                    .with_pixels_per_point(1.0)
                    .renderer(gpu_renderer())
                    .build_ui_state(
                        |ui, demo: &mut Demo| demo.frame(ui),
                        Demo {
                            scene,
                            appearance,
                            app: None,
                            frames: 0,
                        },
                    );
                // Fonts and styles installed on the first frame apply from
                // the next; a few more let images load and layouts settle.
                harness.run_steps(2);
                if let Some(pointer) = scene.pointer() {
                    harness.hover_at(pointer);
                }
                harness.run_steps(6);
                let image = harness.render().expect("the frame renders");
                let path = out.join(format!(
                    "{}-{width}x{height}-{}.png",
                    scene.name(),
                    appearance.label().to_ascii_lowercase()
                ));
                write_png(&path, image);
                eprintln!("wrote {}", path.display());
            }
        }
    }
}

/// The pedal a fixture tone was kept from, by the short name the fixture
/// gives it, and the firmware it ran.
fn fixture_pedal(key: &str) -> (&'static str, &'static str) {
    match key {
        "xl" => ("HX Stomp XL", "3.80"),
        "fx" => ("HX Effects", "3.80"),
        "lt" => ("Helix LT", "3.80"),
        "pro2" => ("StompStation PRO", "2.0.10"),
        "pro15" => ("StompStation PRO", "1.5.12"),
        _ => ("HX Stomp", "3.80"),
    }
}

/// A library of invented tones and setlists, the design's own (its
/// `data-lib.js`). Each tone is a few bytes stored in the scratch library
/// the run points at, so it has a hash and a pedal family like a real one;
/// the rows the table draws are held in memory.
fn library(app: &mut App) {
    // Name | pedal it was kept from | song | artist | character | rating |
    // day kept in September | version | the one-line reading of its chain |
    // the chain, a letter per block (lower case when off, a stack in
    // brackets).
    const TONES: [&str; 19] = [
        "Ambient Swell|stomp|Original||clean|3|10|1|Effects only|VMLLR",
        "Big Room Lead|xl|Neon Avenue|June Arcade|hi-gain|4|22|2|Full rig|DACLR",
        "Brown Lead|stomp|Static Bloom|June Arcade|hi-gain|4|11|3|Full rig|DACLR",
        "Doom Fuzz|stomp|Iron Valley|Slow Comet|fuzz|3|15|1|Full rig|DDACr",
        "Dream Pop|stomp|Glasshouse|Slow Comet|clean|4|23|1|Full rig|MACLR",
        "Edge of Breakup|stomp|Original||drive|4|14|1|Full rig|YDACR",
        "Funk Rhythm|stomp|Original||clean|3|18|1|Full rig|YFACR",
        "Garage Grit|stomp|Original||drive|3|24|1|Amp and cab|DAC",
        "Glass Clean|stomp|Harbour Lights|The Night Signals|clean|4|10|1|Full rig|YACMLR",
        "Glass Wall|pro15|Original||clean|4|29|1|Full rig|YAIEMLR",
        "Octave Fuzz|stomp|Original||fuzz|2|21|1|Amp and cab|PDAC",
        "Pedalboard Wash|fx|Original||clean|3|26|1|Effects only|YMLLR",
        "Plexi Crunch|stomp|Original||drive|4|12|2|Full rig|wDA[CC]lR",
        "Shimmer Lead|pro2|Low Tide|Marlow Kent|hi-gain|4|29|2|Full rig|YDAILR",
        "Slapback Twang|stomp|Dust Road|The Night Signals|clean|0|17|3|Full rig|YACLR",
        "Surf Spring|stomp|Original||clean|3|20|1|Amp and cab|ACMR",
        "Tape Echo Clean|stomp|Paper Boats|June Arcade|clean|4|19|2|Amp and cab|ACLR",
        "Velvet Drive|pro2|Low Tide|Marlow Kent|drive|5|28|4|Full rig|YYwDAIE[Mm]LR",
        "Worship Pad|stomp|Original||clean|5|16|2|Effects only|VPLRR",
    ];
    let field = |tone: &'static str, index: usize| tone.split('|').nth(index).unwrap_or_default();
    let hash = |name: &str| {
        let pedal = TONES
            .iter()
            .find(|tone| field(tone, 0) == name)
            .map_or("stomp", |tone| field(tone, 1));
        let kind = if pedal.starts_with("pro") {
            "vxpreset"
        } else {
            "hxpreset"
        };
        let bytes = format!("TonePush screenshot tone: {name}");
        crate::library::store(name, bytes.as_bytes(), kind)
            .expect("the scratch library takes a synthetic tone")
    };
    app.lib_entries = TONES
        .iter()
        .map(|tone| {
            let number = |index: usize| field(tone, index).parse::<u32>().unwrap_or(0);
            let (name, rating, version) = (field(tone, 0), number(5) as u8, number(7));
            let kept = format!("2026-09-{:02}T18:02:00Z", number(6));
            let (pedal, firmware) = fixture_pedal(field(tone, 1));
            let pro = field(tone, 1).starts_with("pro");
            crate::LibEntry {
                hash: hash(name),
                series: hash(name),
                name: name.to_owned(),
                line: field(tone, 8).to_owned(),
                meta: crate::library::Meta {
                    name: name.to_owned(),
                    song: field(tone, 2).to_owned(),
                    artist: field(tone, 3).to_owned(),
                    character: field(tone, 4).to_owned(),
                    rating,
                    part: "Rhythm".to_owned(),
                    tags: if pro {
                        vec!["stereo".into(), "nam".into()]
                    } else {
                        vec!["stage".into()]
                    },
                    added_at: kept.clone(),
                    modified_at: kept.clone(),
                    pedal: pedal.to_owned(),
                    firmware: firmware.to_owned(),
                    ..Default::default()
                },
                added_at: kept.clone(),
                modified_at: kept,
                downloads: None,
                rating: (rating > 0).then_some(f32::from(rating)),
                version,
                versions: version,
                chain: fixture_chain(field(tone, 9)),
                pro,
                marker: crate::devices::Marker::of_device(pedal, firmware),
                firmware: firmware.to_owned(),
            }
        })
        .collect();
    app.library_lookup.indexed = app.lib_entries.iter().map(|e| e.hash.clone()).collect();
    app.library_lookup.reindex(&app.lib_entries);

    // Every HX tone gets its publishable companion, as the library writes
    // one, and TonePush answers that the design's published tones are there.
    const PUBLISHED: [&str; 7] = [
        "Big Room Lead",
        "Brown Lead",
        "Dream Pop",
        "Glass Clean",
        "Plexi Crunch",
        "Velvet Drive",
        "Worship Pad",
    ];
    let mut published = std::collections::BTreeSet::new();
    for entry in &app.lib_entries {
        if !entry.pro {
            let companion = format!("{{\"data\":{{\"meta\":{{\"name\":\"{}\"}}}}}}", entry.name);
            crate::library::attach_portable(&entry.hash, &companion)
                .expect("the scratch library takes a publishable copy");
        }
        if PUBLISHED.contains(&entry.name.as_str()) {
            if let Some(portable) = crate::library::portable_hash(&entry.hash) {
                published.insert(portable);
            }
        }
    }
    app.cloud_files = Some(published);

    // The setlists are written against the pedal's own presets, so they are
    // made with the HX Stomp's fixture; see `setlists`.
    app.lib_setlists = Vec::new();
}

/// What this library published, as sheet 18 lists it: eight tones with
/// their downloads, the day each went up and the version TonePush gives,
/// Slapback Twang behind the library by a version. TonePush's answers are
/// held in memory; the account's token is an invented string, and the site
/// is not reachable from a screenshot, so nothing reaches any account.
fn mine_fixture(app: &mut App) {
    // Name | downloads | day published in September | version | versions.
    const MINE: [&str; 8] = [
        "Glass Clean|2047|10|1|1",
        "Velvet Drive|1630|28|4|4",
        "Plexi Crunch|1284|12|2|2",
        "Brown Lead|980|11|3|3",
        "Worship Pad|721|16|2|2",
        "Dream Pop|455|23|1|1",
        "Big Room Lead|388|22|2|2",
        "Slapback Twang|312|17|2|2",
    ];
    app.config.token = Some("invented-screenshot-token".to_owned());
    let field = |row: &'static str, index: usize| row.split('|').nth(index).unwrap_or_default();
    for (n, row) in MINE.iter().enumerate() {
        let name = field(row, 0);
        let number = |index: usize| field(row, index).parse::<u64>().unwrap_or(0);
        let (downloads, day, version, versions) = (number(1), number(2), number(3), number(4));
        let entry = app.lib_entries[tone_named(app, name)].clone();
        let id = 9100 + n as i64;
        let song_id = 9200 + n as i64;
        let portable = crate::library::portable_hash(&entry.hash).unwrap_or_default();
        // TonePush gives the library's file, except for Slapback Twang,
        // whose v3 is not up yet.
        let file = if name == "Slapback Twang" {
            "d".repeat(64)
        } else {
            portable
        };
        let published = format!("2026-09-{day:02}T12:00:00Z");
        let mut json = crate::cloud::tests::tone_json(id, song_id, &entry.meta.song);
        json["name"] = name.into();
        json["creator"] = "Noa Calder".into();
        json["file_sha256"] = file.clone().into();
        json["installs_count"] = downloads.into();
        json["downloads_count"] = downloads.into();
        json["version_number"] = version.into();
        json["versions_count"] = versions.into();
        json["created_at"] = published.clone().into();
        json["updated_at"] = published.clone().into();
        json["device"]["name"] = entry.meta.pedal.clone().into();
        json["firmware_version"] = entry.firmware.clone().into();
        if entry.meta.song == "Original" || entry.meta.song.is_empty() {
            json["song"]["kind"] = "original".into();
            json["song"]["title"] = name.into();
            json["song"]["artist"] = serde_json::Value::Null;
        } else {
            json["song"]["kind"] = "song".into();
            json["song"]["title"] = entry.meta.song.clone().into();
            json["song"]["artist"] = entry.meta.artist.clone().into();
        }
        json["versions"] = (1..=versions)
            .map(|number| {
                serde_json::json!({
                    "number": number,
                    "current": number == version,
                    "file_sha256": if number == version {
                        file.clone()
                    } else {
                        format!("{number}").repeat(64)
                    },
                    "created_at": format!(
                        "2026-09-{:02}T12:00:00Z",
                        day.saturating_sub(8 * (versions - number)).max(1)
                    ),
                    "download": {"artifact": format!("/tones/{id}/versions/{number}/artifact")},
                })
            })
            .collect::<Vec<_>>()
            .into();
        let details: crate::cloud::ToneDetails =
            serde_json::from_value(json).expect("an invented TonePush answer");
        let record = crate::library::Published {
            tone_id: id,
            song_id,
            hash: entry.hash.clone(),
            name: name.to_owned(),
            at: published,
        };
        // Kept as the library keeps it, so a refresh of the library finds it.
        crate::library::record_published(&entry.series, record.clone())
            .expect("the scratch library takes a record");
        app.published.insert(entry.series.clone(), record);
        app.mine.details.insert(id, Ok(details));
        if name == "Slapback Twang" {
            app.mine.selected = Some(id);
        }
    }
    app.mine.mark_asked();
    app.lib_showing = crate::LibraryView::Cloud;
    app.pane.cloud_scope = crate::library_pane::CloudScope::Mine;
    app.hearing.arrows = crate::audition::Arrows::Mine;
}

/// The account as a current TonePush answers for it (sheet 19): the tones of
/// `mine_fixture` and one published from another computer, all public, and
/// two setlists, one kept for the account alone. Invented, and held in
/// memory: nothing is asked of any server.
fn account_fixture(app: &mut App) {
    let mut tones: Vec<crate::cloud::ToneDetails> = app
        .mine
        .details
        .values()
        .filter_map(|answer| answer.as_ref().ok().cloned())
        .map(|mut tone| {
            tone.summary.visibility = Some("everyone".to_owned());
            tone
        })
        .collect();
    let mut arena = crate::cloud::tests::tone_json(9300, 9301, "Arena Lead");
    arena["name"] = "Arena Lead".into();
    arena["creator"] = "Noa Calder".into();
    arena["device"]["name"] = "Helix LT".into();
    arena["installs_count"] = 1402.into();
    arena["downloads_count"] = 1402.into();
    arena["version_number"] = 1.into();
    arena["versions_count"] = 1.into();
    arena["visibility"] = "everyone".into();
    arena["created_at"] = "2026-08-03T12:00:00Z".into();
    arena["song"]["kind"] = "original".into();
    arena["song"]["title"] = "Arena Lead".into();
    arena["versions"] = serde_json::json!([{
        "number": 1, "current": true, "file_sha256": "a".repeat(64),
        "created_at": "2026-08-03T12:00:00Z",
        "download": {"artifact": "/tones/9300/versions/1/artifact"}
    }]);
    tones.push(serde_json::from_value(arena).expect("an invented TonePush answer"));
    let setlist =
        |id: i64, name: &str, venue: &str, device: &str, slots: u32, seen: &str, day: &str| {
            serde_json::from_value::<crate::cloud::SetlistSummary>(serde_json::json!({
                "id": id,
                "name": name,
                "venue": venue,
                "performed_on": null,
                "visibility": seen,
                "state": "published",
                "device": {"id": 1, "name": device, "slug": "device", "family": null},
                "slot_count": slots,
                "created_at": day,
                "updated_at": day,
            }))
            .expect("an invented TonePush setlist")
        };
    app.account.tones = Some(tones);
    app.account.setlists = Some(vec![
        setlist(
            9400,
            "Album release show",
            "Lido Rooftop, Berlin",
            "HX Stomp",
            42,
            "everyone",
            "2026-10-02T12:00:00Z",
        ),
        setlist(
            9401,
            "Summer tour",
            "Various",
            "StompStation PRO",
            21,
            "only_you",
            "2026-08-02T12:00:00Z",
        ),
    ]);
    app.account.me = serde_json::from_value(serde_json::json!({
        "id": 7,
        "name": "Noa Calder",
        "slug": "noa-calder",
        "profile_url": "https://tonepush.rocks/creators/noa-calder",
    }))
    .ok();
    app.account.mark_listed(crate::cloud::Capabilities {
        account: true,
        setlists: true,
    });
}

/// TonePush's public feed for the HX Stomp, as the design's mockups list it:
/// invented tones by invented creators, each with its Song, its downloads and
/// its versions. Held in memory as the site would have answered; nothing is
/// fetched.
fn cloud_feed(app: &mut App) {
    // Name | song | artist | by | character | downloads | day updated in
    // September | version | versions | the chain, a letter per block.
    const TONES: [&str; 9] = [
        "Glass Cathedral|Original||Mira Holt|clean|3410|30|2|2|YACMLR",
        "Midnight City|Neon Rain|Halcyon Drive|Tomas Reyes|drive|2184|27|1|1|YDACLR",
        "Stadium Rhythm|Gold Coast|The Velvet Static|Ines Park|hi-gain|1902|25|3|3|YDACCR",
        "Velvet Fuzz|Original||Dario Venn|fuzz|1288|24|1|1|DACMR",
        "Warm Jazz Box|Blue Hour|Paper Moons|Ruth Okafor|clean|812|21|1|1|YACR",
        "Chime Machine|Original||Mira Holt|clean|655|19|2|2|YACLLR",
        "Dust Devil|Red Mesa|Halcyon Drive|K. Albrecht|fuzz|590|18|1|1|PDACL",
        "Hollow Body Lead|Original||Ana Salgado|drive|418|15|1|1|YDACLR",
        "Tidal Swell|Original||Tomas Reyes|clean|377|12|1|1|VMLRR",
    ];
    let field = |tone: &'static str, index: usize| tone.split('|').nth(index).unwrap_or_default();
    let category = |letter: char| match letter.to_ascii_uppercase() {
        'D' => 1,
        'Y' => 2,
        'E' => 3,
        'M' => 4,
        'L' => 5,
        'R' => 6,
        'P' => 7,
        'F' => 8,
        'W' => 9,
        'A' => 11,
        'C' => 13,
        'I' => 14,
        _ => 15,
    };
    app.cloud_entries = TONES
        .iter()
        .enumerate()
        .map(|(index, tone)| {
            let id = 9000 + index as i64;
            let number = |at: usize| field(tone, at).parse::<u64>().unwrap_or(0);
            let song = field(tone, 1);
            let artist = field(tone, 2);
            let original = song == "Original";
            let chain: Vec<serde_json::Value> = field(tone, 9)
                .chars()
                .map(|letter| {
                    serde_json::json!({
                        "name": letter.to_string(),
                        "category": category(letter),
                        "enabled": letter.is_ascii_uppercase(),
                    })
                })
                .collect();
            let hash = format!("{:064x}", 0xc10d_0000_u64 + index as u64);
            let updated = format!("2026-09-{:02}T10:00:00Z", number(6));
            let json = serde_json::json!({
                "id": id,
                "song_id": id + 500,
                "name": field(tone, 0),
                "device": {
                    "id": 1, "name": "HX Stomp", "slug": "hx-stomp", "family": "Helix",
                    "manufacturer": "Line 6", "capabilities": {}, "artifact_extensions": ["hlx"]
                },
                "creator": field(tone, 3),
                "description": null,
                "state": "published",
                "availability": "free",
                "source_kind": "native",
                "firmware_version": "3.80",
                "minimum_firmware_version": null,
                "parser_version": "0.8.0",
                "installs_count": number(5),
                "saves_count": 0,
                "remix_count": 0,
                "version_number": number(7),
                "versions_count": number(8),
                "created_at": "2026-09-01T10:00:00Z",
                "updated_at": updated,
                "parent_id": null,
                "signal_chain": chain,
                "file_sha256": hash,
                "parsed_metadata": { "character": field(tone, 4) },
                "download": { "artifact": format!("/tones/{id}/artifact") },
                "song": {
                    "id": id + 500,
                    "title": song,
                    "kind": if original { "original" } else { "song" },
                    "artist": if original { None } else { Some(artist) },
                    "part": "Rhythm",
                    "description": null,
                    "tags": ["stage"],
                    "genres": [],
                    "tuning": null,
                    "guitar_type": null,
                    "pickup_type": null,
                    "pickup_electronics": null,
                    "tone_count": 1,
                    "devices": ["HX Stomp"],
                    "file_sha256s": [hash]
                }
            });
            let tone: crate::cloud::ToneDetails =
                serde_json::from_value(json).expect("the invented tone decodes as the site's");
            let discovered = crate::cloud::DiscoveredTone {
                song: tone.song.clone().expect("the invented tone has its Song"),
                tone,
            };
            crate::CloudEntry {
                row: App::cloud_row(&discovered),
                discovered,
            }
        })
        .collect();
    app.cloud_total = Some(1204);
    // The feed on screen is the one this search asked for, so opening the
    // Cloud does not start another.
    app.cloud_loaded_query = Some(String::new());
    app.cloud_loaded_order = Some(app.cloud_order);
    app.cloud_loaded_device = Some(app.cloud_device_scope());
}

/// The setlists, written against the HX Stomp's presets: Album release show
/// differs from the pedal in six slots (four hold other presets, two it would
/// empty), Rehearsal matches it, Studio session holds only its first 24,
/// Summer tour is for a StompStation PRO, and Acoustic night only its first 12.
fn setlists(app: &mut App) {
    let store = |name: &str, salt: &str| {
        crate::library::store(
            name,
            format!("TonePush screenshot preset: {name}{salt}").as_bytes(),
            "hxpreset",
        )
        .expect("the scratch library takes a synthetic preset")
    };
    // What the pedal holds in every slot, as its backup would say: the
    // library's copy where it keeps that preset, its own otherwise.
    for (index, name) in app.presets.clone().iter().enumerate() {
        if !shell::hx_slot_is_empty(name) && !app.mirror.contains_key(&(index as i64)) {
            app.mirror.insert(index as i64, store(name, ""));
        }
    }
    let on_pedal = |app: &App, index: usize| crate::library::Slot {
        hash: app.mirror.get(&(index as i64)).cloned().unwrap_or_default(),
        name: app.presets[index].clone(),
        file: String::new(),
    };
    let filled = app
        .presets
        .iter()
        .take_while(|name| !shell::hx_slot_is_empty(name))
        .count();
    let total = app.presets.len();
    let mut album: Vec<crate::library::Slot> = (0..filled).map(|i| on_pedal(app, i)).collect();
    // Older versions of three presets, a fourth the setlist plays instead,
    // and two slots it leaves empty.
    for index in [4, 12, 26] {
        album[index].hash = store(&album[index].name, " as it was in September");
    }
    album[31] = crate::library::Slot {
        hash: store("Night Verb", ""),
        name: "Night Verb".to_owned(),
        file: String::new(),
    };
    album[39] = crate::library::Slot::default();
    album[40] = crate::library::Slot::default();
    album.resize(total, crate::library::Slot::default());
    let rehearsal: Vec<crate::library::Slot> = (0..total)
        .map(|i| {
            if i < filled {
                on_pedal(app, i)
            } else {
                crate::library::Slot::default()
            }
        })
        .collect();
    let first = |count: usize| -> Vec<crate::library::Slot> {
        (0..total)
            .map(|i| {
                if i < count {
                    on_pedal(app, i)
                } else {
                    crate::library::Slot::default()
                }
            })
            .collect()
    };
    let mut studio = first(24);
    studio[7].hash = store(&studio[7].name, " from the studio");
    let acoustic = first(12);
    let pro_slot = |name: &str| crate::library::Slot {
        hash: app
            .lib_entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.hash.clone())
            .unwrap_or_default(),
        name: name.to_owned(),
        file: String::new(),
    };
    let summer = vec![
        pro_slot("Velvet Drive"),
        pro_slot("Glass Wall"),
        pro_slot("Shimmer Lead"),
    ];
    let setlist = |name: &str,
                   venue: &str,
                   date: &str,
                   version: u32,
                   captured: &str,
                   slots: Vec<crate::library::Slot>| crate::library::Setlist {
        series: format!("series-{name}"),
        version,
        added_at: "2026-07-01T12:00:00Z".to_owned(),
        modified_at: captured.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        venue: venue.to_owned(),
        date: date.to_owned(),
        slots,
    };
    app.lib_setlists = vec![
        (
            "album-release-show.json".into(),
            setlist(
                "Album release show",
                "Lido Rooftop, Berlin",
                "4 Oct 2026",
                2,
                "2026-10-04T18:02:00Z",
                album,
            ),
        ),
        (
            "rehearsal.json".into(),
            setlist(
                "Rehearsal",
                "Room 3",
                "28 Sep 2026",
                1,
                "2026-09-28T19:30:00Z",
                rehearsal,
            ),
        ),
        (
            "studio-session.json".into(),
            setlist(
                "Studio session",
                "Kranhaus Studio",
                "12 Sep 2026",
                1,
                "2026-09-12T11:00:00Z",
                studio,
            ),
        ),
        (
            "summer-tour.json".into(),
            setlist(
                "Summer tour",
                "Various",
                "2 Aug 2026",
                3,
                "2026-08-02T10:00:00Z",
                summer,
            ),
        ),
        (
            "acoustic-night.json".into(),
            setlist(
                "Acoustic night",
                "Café Wendel",
                "18 Jul 2026",
                1,
                "2026-07-18T20:00:00Z",
                acoustic,
            ),
        ),
    ];
    // Every tone a setlist plays is held, so no slot reads as missing.
    for (_, setlist) in &app.lib_setlists {
        for slot in &setlist.slots {
            if slot.is_empty() {
                continue;
            }
            let entry = app.lib_entries.iter().find(|entry| entry.hash == slot.hash);
            let meta = entry.map(|entry| entry.meta.clone()).unwrap_or_default();
            // A preset the library keeps only in a setlist was kept from the
            // HX Stomp, as the setlists were.
            let marker = entry.map_or_else(
                || Some(crate::devices::Marker::hx("Stomp")),
                |entry| entry.marker.clone(),
            );
            let firmware = entry.map_or_else(|| "3.80".to_owned(), |entry| entry.firmware.clone());
            app.library_lookup.setlist_tones.insert(
                slot.hash.clone(),
                crate::SetlistTone {
                    held: true,
                    meta,
                    marker,
                    firmware,
                },
            );
        }
    }
}

/// The HX Stomp's backups as the design shows them: the automatic copy,
/// kept current after 01B was saved; the copy set aside as the pedal
/// connected; the one set aside before Album release show was written, which
/// differs from the pedal in six slots and is chosen; a copy saved to a
/// file; and older ones set aside as the pedal connected. All in memory:
/// nothing is read from disk or written to it.
fn backups_history(app: &mut App) {
    use crate::backups::{BackupCopy, Held, Origin, Why};

    let manifest = app
        .automatic_backup
        .clone()
        .expect("the HX Stomp fixture is backed up");
    let automatic = session::automatic_dir().expect("the scratch backups have a home");
    let history = automatic
        .parent()
        .expect("the automatic backup has a folder")
        .join("history");
    let at = |days: i64, hour: i8, minute: i8| -> u64 {
        let time = jiff::Zoned::now()
            .with()
            .hour(hour)
            .minute(minute)
            .second(0)
            .subsec_nanosecond(0)
            .build()
            .expect("a time of day")
            .checked_sub(jiff::Span::new().days(days))
            .expect("a day in the past");
        u64::try_from(time.timestamp().as_second()).expect("after 1970")
    };
    let now = Held {
        names: vec![app.presets.clone()],
        hashes: app
            .mirror
            .iter()
            .map(|(slot, hash)| ((0, *slot as usize), hash.clone()))
            .collect(),
    };
    let album = app
        .lib_setlists
        .iter()
        .find(|(_, setlist)| setlist.name == "Album release show")
        .map(|(_, setlist)| setlist.clone())
        .expect("the fixture's setlists include Album release show");
    let before = Held {
        names: vec![(0..app.presets.len())
            .map(|slot| {
                album
                    .slots
                    .get(slot)
                    .map(|held| held.name.clone())
                    .unwrap_or_default()
            })
            .collect()],
        hashes: album
            .slots
            .iter()
            .enumerate()
            .filter(|(_, held)| !held.is_empty())
            .map(|(slot, held)| ((0, slot), held.hash.clone()))
            .collect(),
    };
    let copy = |path: std::path::PathBuf, origin: Origin, when: u64| BackupCopy {
        path,
        origin,
        when,
        manifest: hx_usb::backup::Manifest {
            captured: when,
            ..manifest.clone()
        },
        complete: true,
    };
    let set_aside = |why: Why, when: u64| {
        copy(
            history.join(format!("automatic {}.hxbundle", session::stamp_of(when))),
            Origin::SetAside(Some(why)),
            when,
        )
    };
    let chosen = set_aside(
        Why::Setlist {
            name: "Album release show".to_owned(),
        },
        at(3, 18, 2),
    );
    let copies = vec![
        copy(
            automatic.clone(),
            Origin::Current { saved: Some(1) },
            at(0, 14, 2),
        ),
        set_aside(Why::Connected, at(0, 13, 58)),
        chosen.clone(),
        set_aside(Why::Connected, at(4, 21, 11)),
        copy(
            automatic.with_file_name("hx-stomp-before-tour.hxbundle"),
            Origin::File,
            at(9, 10, 2),
        ),
        set_aside(Why::Connected, at(16, 19, 40)),
        set_aside(Why::Connected, at(23, 11, 5)),
    ];
    for copy in &copies {
        let held = if copy.path == chosen.path {
            before.clone()
        } else {
            now.clone()
        };
        app.backup_held.insert(copy.path.clone(), held);
    }
    app.backup_chosen = Some(chosen.path);
    app.backup_shelf = Some(copies);
}

/// 01B Plexi Crunch on an HX Stomp: a wah under EXP 1, a drive on FS1, the
/// amp selected, a Y split into two cabs, a tape delay the Solo snapshot turns
/// on, and a reverb on FS3. Models are found by name in HX Edit's catalog.
fn hx_stomp(app: &mut App) {
    use hx_proto::preset::{Assignment, Kind, Lane, Layout, Path, Target};
    use hx_proto::rpc::Source;

    let names = [
        "Glass Clean",
        "Plexi Crunch",
        "Brown Lead",
        "Edge of Breakup",
        "Ambient Swell",
        "Slapback Twang",
        "Doom Fuzz",
        "Worship Pad",
        "Funk Rhythm",
        "Velvet Lead",
        "Tape Echo Clean",
        "Octave Fuzz",
        "Shimmer Pad",
        "Chime Clean",
        "Smooth Overdrive",
        "Surf Spring",
        "Tremolo Clean",
        "Desert Rock",
        "Jazz Box",
        "Bass DI",
        "Acoustic Sim",
        "Wall of Fuzz",
        "Clean Comp",
        "Big Room Lead",
        "Vibe Rhythm",
        "Dotted Eighths",
        "Crunch Stack",
        "Soft Swell",
        "Garage Grit",
        "Lead Boost",
        "Tight Rhythm",
        "Sparkle Verb",
        "Rotary Clean",
        "Twang Machine",
        "Cave Drone",
        "Blues Edge",
        "Dream Pop",
        "Grunge Crunch",
        "Octave Lead",
        "Reverse Swell",
        "Practice Clean",
        "Studio Rhythm",
    ];
    app.connection = Connection::Online;
    app.status.clear();
    app.device = "HX Stomp".to_owned();
    // The library is already scoped to the pedal, as it is once one connects.
    app.library_connected_device = "HX Stomp".to_owned();
    app.library_device_filter = Some("HX Stomp".to_owned());
    app.firmware = "3.80".to_owned();
    app.preset_count = 126;
    app.presets = (0..126)
        .map(|index| {
            names
                .get(index)
                .map_or("New Preset", |name| name)
                .to_owned()
        })
        .collect();
    for slot in [0, 1, 9, 12] {
        if !app.config.is_favorite(0, slot) {
            app.config
                .favorites
                .push(crate::config::Favorite { setlist: 0, slot });
        }
    }
    app.preset_index = 1;
    app.preset_name = "Plexi Crunch".to_owned();
    app.tempo = Some(104.0);
    app.snapshots = vec!["Verse".into(), "Chorus".into(), "Solo".into()];
    app.current_snapshot = 1;
    app.dirty = true;
    app.undo_depth = 3;
    // What the pedal holds, as the automatic backup would say: the slots the
    // library holds unchanged, and one it holds in another version. A tone
    // the library kept from another pedal is not this pedal's preset of the
    // same name.
    for (index, name) in app.presets.clone().iter().enumerate() {
        let kept_here =
            |entry: &&crate::LibEntry| &entry.name == name && entry.meta.pedal == "HX Stomp";
        if let Some(entry) = app.lib_entries.iter().find(kept_here) {
            let hash = if name == "Tape Echo Clean" {
                crate::library::store(
                    name,
                    b"TonePush screenshot preset: Tape Echo Clean as edited on the pedal",
                    "hxpreset",
                )
                .expect("the scratch library takes a synthetic preset")
            } else {
                entry.hash.clone()
            };
            app.mirror.insert(index as i64, hash);
        }
    }
    setlists(app);
    let captured = two_minutes_past_two()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    app.automatic_backup = Some(hx_usb::backup::Manifest {
        version: 1,
        device: "HX Stomp".to_owned(),
        firmware: "3.80".to_owned(),
        captured,
        setlists: vec!["SETLIST 1".to_owned()],
        presets: app.presets.clone(),
        more_setlists: Vec::new(),
        irs: [
            ("0", "Silver Bell 2x12"),
            ("1", "Field Coil 1x12"),
            ("2", "Blue Room 4x12"),
            ("3", "Lido Rooftop Hall"),
        ]
        .into_iter()
        .map(|(slot, name)| (slot.to_owned(), name.to_owned()))
        .collect(),
        globals: 154,
    });
    app.irs = vec![
        (0, "Silver Bell 2x12".to_owned()),
        (1, "Field Coil 1x12".to_owned()),
        (2, "Blue Room 4x12".to_owned()),
        (3, "Lido Rooftop Hall".to_owned()),
    ];
    app.favourites = vec![
        (0, "Minotaur".to_owned()),
        (1, "Plateaux".to_owned()),
        (2, "Transistor Tape".to_owned()),
    ];
    // Every setting at a plain value, and the global EQ shaped.
    use hx_proto::settings::{Kind as Setting, SETTINGS};
    app.settings = SETTINGS
        .iter()
        .map(|setting| {
            let value = match &setting.kind {
                Setting::Switch(..) => 1.0,
                Setting::Choice(_) => 0.0,
                Setting::Number { min, max, .. } => (min + max) / 2.0,
            };
            (setting.id, value)
        })
        .collect();
    for (id, value) in [
        (crate::id::EQ_ON, 1.0),
        (crate::id::LOW_CUT, 80.0),
        (crate::id::LOW_FREQ, 120.0),
        (crate::id::LOW_Q, 0.7),
        (crate::id::LOW_GAIN, 2.5),
        (crate::id::MID_FREQ, 800.0),
        (crate::id::MID_Q, 1.0),
        (crate::id::MID_GAIN, -2.0),
        (crate::id::HIGH_FREQ, 4000.0),
        (crate::id::HIGH_Q, 0.7),
        (crate::id::HIGH_GAIN, 1.5),
        (crate::id::HIGH_CUT, 12000.0),
    ] {
        app.settings.insert(id, value);
    }
    app.log = vec![
        "connected to HX Stomp, firmware 3.80".to_owned(),
        "backed up 126 presets, 154 settings and 4 impulse responses".to_owned(),
        "loaded 01B Plexi Crunch".to_owned(),
    ];

    let Some(catalog) = app.catalog.as_ref() else {
        // Without HX Edit's data there are no model names to show.
        return;
    };
    // By name within a category: the catalog has an amp and a preamp both
    // called US Double Nrm.
    let in_category = |name: &str, category: &str| {
        catalog
            .models()
            .find(|model| {
                model.name == name
                    && catalog
                        .category_of(&model.id)
                        .and_then(|id| catalog.category(id))
                        .is_some_and(|found| found.name == category)
            })
            .and_then(|model| crate::number_of(catalog, &model.id))
            .unwrap_or(0)
    };
    let number = |name: &str| {
        let category = match name {
            "Teardrop 310" => "Wah",
            "Minotaur" => "Distortion",
            "US Double Nrm" => "Amp",
            "2x12 Silver Bell" | "1x12 Field Coil" => "Cab",
            "Transistor Tape" => "Delay",
            "Plateaux" => "Reverb",
            _ => "",
        };
        let found = in_category(name, category);
        if found != 0 {
            return found;
        }
        catalog
            .models()
            .find(|model| model.name == name)
            .and_then(|model| crate::number_of(catalog, &model.id))
            .unwrap_or(0)
    };
    let defaults = |model: u32| -> Vec<f32> {
        catalog
            .model_number(model)
            .map(|model| {
                catalog
                    .ordered_params(model)
                    .iter()
                    .map(|p| p.default)
                    .collect()
            })
            .unwrap_or_default()
    };
    // The amp's knobs as the design shows them, as fractions of each range.
    let amp = number("US Double Nrm");
    let amp_values: Vec<f32> = catalog
        .model_number(amp)
        .map(|model| {
            let fractions = [
                0.45, 0.55, 0.6, 0.65, 0.4, 0.75, 1.0, 0.5, 0.5, 0.5, 0.6, 0.5,
            ];
            catalog
                .ordered_params(model)
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    fractions
                        .get(index)
                        .map_or(param.default, |f| param.min + f * (param.max - param.min))
                })
                .collect()
        })
        .unwrap_or_default();
    let block =
        |position: i64, kind: Kind, model: u32, enabled: bool, values: Vec<f32>| session::Block {
            position,
            routing: matches!(kind, Kind::Input | Kind::Output).then_some(0),
            kind,
            model,
            enabled,
            values,
            paired: None,
            paired_values: Vec::new(),
        };
    let wah = number("Teardrop 310");
    let drive = number("Minotaur");
    let cab = number("2x12 Silver Bell");
    let second_cab = number("1x12 Field Coil");
    let delay = number("Transistor Tape");
    let reverb = number("Plateaux");
    let split = number("Split Y");
    let join = number("Mixer");
    app.chain = vec![
        block(0, Kind::Input, 0, true, Vec::new()),
        block(1, Kind::Block, wah, false, defaults(wah)),
        block(2, Kind::Block, drive, true, defaults(drive)),
        block(3, Kind::Block, amp, true, amp_values),
        block(4, Kind::Block, cab, true, defaults(cab)),
        block(5, Kind::Block, delay, false, defaults(delay)),
        block(6, Kind::Block, reverb, true, defaults(reverb)),
        block(9, Kind::Output, 0, true, Vec::new()),
        block(10, Kind::Split, split, true, defaults(split)),
        block(11, Kind::Block, second_cab, true, defaults(second_cab)),
        block(19, Kind::Join, join, true, defaults(join)),
    ];
    app.layout = Layout {
        paths: vec![Path {
            input: Some(0),
            output: Some(9),
            split: Some(10),
            join: Some(19),
            head: vec![1, 2, 3],
            lanes: vec![
                Lane {
                    branch: 0,
                    blocks: vec![4],
                    span: 4..5,
                },
                Lane {
                    branch: 1,
                    blocks: vec![11],
                    span: 11..19,
                },
            ],
            tail: vec![5, 6],
        }],
    };
    app.selected = 3;
    // The input on Multi and the output on Main L/R, as the design shows.
    for (position, symbol, wanted) in [
        (0, "HelixStomp_AppDSPFlowInput", "Multi"),
        (9, "HelixStomp_AppDSPFlowOutputMain", "Main"),
    ] {
        let routing = catalog
            .model(symbol)
            .and_then(|model| {
                model
                    .params
                    .iter()
                    .find(|p| p.id == "@input" || p.id == "@output")
            })
            .and_then(|param| catalog.choices(param))
            .and_then(|choices| {
                choices
                    .iter()
                    .position(|choice| choice.starts_with(wanted))
                    .or_else(|| {
                        choices
                            .iter()
                            .position(|choice| choice.starts_with("Multi"))
                    })
            });
        if let (Some(routing), Some(block)) = (
            routing,
            app.chain.iter_mut().find(|b| b.position == position),
        ) {
            block.routing = Some(routing as i64);
        }
    }
    let param = |model: u32, name: &str| -> i64 {
        catalog
            .model_number(model)
            .and_then(|model| {
                catalog
                    .ordered_params(model)
                    .iter()
                    .position(|param| param.name == name)
            })
            .unwrap_or(0) as i64
    };
    app.assignments = vec![
        Assignment {
            block: 1,
            source: Source::Expression(1),
            target: Target::Bypass,
            min: 0.0,
            max: 1.0,
            cc: None,
        },
        Assignment {
            block: 1,
            source: Source::Expression(1),
            target: Target::Param(param(wah, "Position")),
            min: 0.0,
            max: 1.0,
            cc: None,
        },
        Assignment {
            block: 3,
            source: Source::Snapshots,
            target: Target::Param(param(amp, "Drive")),
            min: 0.0,
            max: 1.0,
            cc: None,
        },
        Assignment {
            block: 3,
            source: Source::Footswitch(2),
            target: Target::Param(param(amp, "Ch Vol")),
            min: 0.75,
            max: 0.86,
            cc: None,
        },
        Assignment {
            block: 5,
            source: Source::Snapshots,
            target: Target::Param(param(delay, "Mix")),
            min: 0.0,
            max: 1.0,
            cc: None,
        },
        Assignment {
            block: 6,
            source: Source::Snapshots,
            target: Target::Param(param(reverb, "Mix")),
            min: 0.0,
            max: 1.0,
            cc: None,
        },
        Assignment {
            block: 6,
            source: Source::MidiCc,
            target: Target::Param(param(reverb, "Decay")),
            min: 0.0,
            max: 0.6,
            cc: Some(4),
        },
    ];
    let led = |name: &str| {
        catalog
            .menu(hx_catalog::FOOTSWITCH_LED)
            .and_then(|colours| colours.iter().position(|colour| colour == name))
            .map(|index| index as i64)
    };
    let carried = |block: i64, name: &str, colour: i64, enabled: bool| hx_usb::Carried {
        block,
        name: name.to_owned(),
        colour: Some(colour),
        enabled,
    };
    app.switches = vec![
        hx_usb::Switch {
            switch: 1,
            momentary: false,
            label: None,
            colour: None,
            carries: vec![carried(2, "Minotaur", 0xff8c10, true)],
        },
        hx_usb::Switch {
            switch: 2,
            momentary: false,
            label: Some("LEAD".to_owned()),
            colour: led("Green"),
            carries: vec![
                carried(5, "Transistor Tape", 0x00cc00, false),
                carried(3, "US Double Nrm", 0xdd1111, false),
            ],
        },
        hx_usb::Switch {
            switch: 3,
            momentary: false,
            label: None,
            colour: led("Turquoise"),
            carries: vec![carried(6, "Plateaux", 0xff5c00, true)],
        },
    ];
    // Which blocks each snapshot turns on, by slot: the drive comes in for
    // Chorus and the delay for Solo.
    let slots = [1, 2, 3, 4, 11, 5, 6];
    app.snapshot_details = [
        ("Verse", [false, false, true, true, true, false, true]),
        ("Chorus", [false, true, true, true, true, false, true]),
        ("Solo", [false, true, true, true, true, true, true]),
    ]
    .into_iter()
    .map(|(name, states)| {
        let mut enabled = vec![None; 20];
        for (slot, on) in slots.iter().zip(states) {
            enabled[*slot] = Some(on);
        }
        hx_proto::preset::Snapshot {
            name: name.to_owned(),
            tempo: None,
            valid: true,
            named: true,
            enabled,
            colour: None,
        }
    })
    .collect();
}
