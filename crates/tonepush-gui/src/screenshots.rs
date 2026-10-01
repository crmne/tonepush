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
    /// Nothing plugged in.
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
}

impl Scene {
    const ALL: [Scene; 25] = [
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
        }
    }

    fn stage(self, app: &mut App) {
        // Nothing goes to TonePush's site from a screenshot.
        app.cloud_search_due = None;
        app.cloud_check = None;
        library(app);
        match self {
            Scene::HxEdit | Scene::Settings => hx_stomp(app),
            Scene::ProEdit => pro(app),
            Scene::SetlistConfirm | Scene::HxSetlists => {
                hx_stomp(app);
                app.page = shell::Page::Library;
                app.lib_showing = crate::LibraryView::Setlists;
                app.select_setlist_entry(0);
                if self == Scene::SetlistConfirm {
                    app.confirm_push = Some(0);
                }
            }
            Scene::HxLibrary => {
                hx_stomp(app);
                app.page = shell::Page::Library;
                let sending = app
                    .lib_entries
                    .iter()
                    .position(|entry| entry.name == "Slapback Twang")
                    .expect("the library holds Slapback Twang");
                app.select_lib_entry(sending);
                app.sending = Some(crate::Sending {
                    hash: app.lib_entries[sending].hash.clone(),
                    name: app.lib_entries[sending].name.clone(),
                });
            }
            Scene::HxPedal => {
                hx_stomp(app);
                backups_history(app);
                app.page = shell::Page::Pedal;
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
                app.connection = Connection::Offline;
                app.status =
                    "No supported pedal found. Check USB and close any other pedal editor."
                        .to_owned();
            }
        }
    }
}

/// A chain written a letter per block, as the fixture's tones write it: W
/// wah, D drive, Y dynamics, E EQ, M modulation, L delay, R reverb, P pitch,
/// F filter, A amp, C cab, I impulse response; lower case when off, and a
/// parallel stretch in brackets.
fn fixture_chain(code: &str) -> Vec<shell::Mini> {
    let block = |letter: char| {
        let category = match letter.to_ascii_uppercase() {
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
                        },
                    );
                // Fonts and styles installed on the first frame apply from
                // the next; a few more let images load and layouts settle.
                harness.run_steps(8);
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

/// A library of invented tones and setlists. Each tone is a few bytes stored
/// in the scratch library the run points at, so it has a hash and a pedal
/// family like a real one; the rows the table draws are held in memory.
fn library(app: &mut App) {
    // Name | pedal | song | artist | character | rating | day kept in
    // September | version | the one-line reading of its chain | the chain,
    // a letter per block (lower case when off, a stack in brackets).
    const TONES: [&str; 17] = [
        "Plexi Crunch|hx|Original||drive|4|12|2|Full rig|wDA[CC]lR",
        "Glass Clean|hx|Harbour Lights|The Night Signals|clean|4|10|1|Full rig|YACMLR",
        "Brown Lead|hx|Static Bloom|June Arcade|hi-gain|4|11|3|Full rig|DDACLR",
        "Ambient Swell|hx|Original||clean|3|10|1|Effects only|YMLLR",
        "Velvet Drive|pro|Low Tide|Marlow Kent|drive|5|28|4|Full rig|YDAIEMLR",
        "Edge of Breakup|hx|Original||drive|4|14|1|Full rig|YDACR",
        "Doom Fuzz|hx|Iron Valley|Slow Comet|fuzz|3|15|1|Full rig|DDACr",
        "Worship Pad|hx|Original||clean|5|16|2|Effects only|YMLRR",
        "Glass Wall|pro|Original||clean|4|29|1|Full rig|YMDAIELR",
        "Slapback Twang|hx|Dust Road|The Night Signals|clean|0|17|1|Full rig|YACLR",
        "Funk Rhythm|hx|Original||clean|3|18|1|Full rig|YFACR",
        "Tape Echo Clean|hx|Paper Boats|June Arcade|clean|4|19|2|Amp and cab|DACLR",
        "Shimmer Lead|pro|Low Tide|Marlow Kent|hi-gain|4|29|2|Full rig|YDAIMLR",
        "Surf Spring|hx|Original||clean|3|20|1|Amp and cab|ACRR",
        "Octave Fuzz|hx|Original||fuzz|2|21|1|Amp and cab|PDAC",
        "Dream Pop|hx|Glasshouse|Slow Comet|clean|4|23|1|Full rig|YMACLR",
        "Garage Grit|hx|Original||drive|3|24|1|Amp and cab|DAC",
    ];
    let field = |tone: &'static str, index: usize| tone.split('|').nth(index).unwrap_or_default();
    let hash = |name: &str| {
        let pedal = TONES
            .iter()
            .find(|tone| field(tone, 0) == name)
            .map_or("hx", |tone| field(tone, 1));
        let kind = if pedal == "pro" {
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
                    tags: if field(tone, 1) == "pro" {
                        vec!["stereo".into(), "nam".into()]
                    } else {
                        vec!["stage".into()]
                    },
                    added_at: kept.clone(),
                    modified_at: kept.clone(),
                    ..Default::default()
                },
                added_at: kept.clone(),
                modified_at: kept,
                downloads: None,
                rating: (rating > 0).then_some(f32::from(rating)),
                version,
                versions: version,
                chain: fixture_chain(field(tone, 9)),
                pro: field(tone, 1) == "pro",
            }
        })
        .collect();
    app.library_lookup.indexed = app.lib_entries.iter().map(|e| e.hash.clone()).collect();
    app.library_lookup.reindex(&app.lib_entries);

    // The setlists are written against the pedal's own presets, so they are
    // made with the HX Stomp's fixture; see `setlists`.
    app.lib_setlists = Vec::new();
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
            let meta = app
                .lib_entries
                .iter()
                .find(|entry| entry.hash == slot.hash)
                .map(|entry| entry.meta.clone())
                .unwrap_or_default();
            app.library_lookup
                .setlist_tones
                .insert(slot.hash.clone(), crate::SetlistTone { held: true, meta });
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
    // library holds unchanged, and one it holds in another version.
    for (index, name) in app.presets.clone().iter().enumerate() {
        if let Some(entry) = app.lib_entries.iter().find(|entry| &entry.name == name) {
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
        }
    })
    .collect();
}
