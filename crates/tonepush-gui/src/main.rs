//! Desktop entry point.

fn main() -> eframe::Result<()> {
    // First of all, because everything below reads one of the directories it
    // moves: a machine that knew this program under its old name has its
    // library, its setlists and its extracted resources filed under that name,
    // and looking only under the new one would show an empty library and call
    // it the truth.
    for dir in hx_catalog::home::adopt_former_name() {
        eprintln!("brought {} across from the old name", dir.display());
    }
    // A library written before tones were stored by content moves across now,
    // once, silently: a few file renames and a rewritten index. It happens here
    // rather than in `App::new` because this is the one place that only ever
    // runs for a real person on their real library. Doing it in the app's
    // constructor put it in reach of every test that builds an App, and a test
    // that reaches out of its scratch directory and rearranges the machine's
    // actual library is not a test.
    let moved = tonepush_gui::library::migrate();
    if moved > 0 {
        eprintln!("moved {moved} tones into the library's object store");
    }
    // And whatever is in the store is called what its tone is called. Separate
    // from the migration above because it is not a one-off: a library written
    // by an earlier TonePush has objects named after their hashes, and a
    // rename that failed half way should simply finish next time.
    let renamed = tonepush_gui::library::tidy_names();
    if renamed > 0 {
        eprintln!("gave {renamed} tones their own names on disk");
    }

    let (tx, rx, repaint) = tonepush_gui::spawn_repainting();
    // Closing the window must let the device go cleanly. A process that just
    // disappears leaves the device mid-conversation, and it then refuses new
    // sessions until its power is pulled.
    let on_exit = tx.clone();
    eframe::run_native(
        "TonePush",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 640.0]),
            ..Default::default()
        },
        Box::new(move |cc| {
            repaint.bind(&cc.egui_ctx);
            // Lets `ui.image("file://…")` load the model artwork HX Edit ships.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(tonepush_gui::App::new(&cc.egui_ctx, tx, rx)))
        }),
    )?;

    // eframe has returned, so the window is gone; give the worker a moment to
    // put the device down before the process exits.
    let _ = on_exit.send(tonepush_gui::Cmd::Disconnect);
    std::thread::sleep(std::time::Duration::from_millis(800));
    Ok(())
}
