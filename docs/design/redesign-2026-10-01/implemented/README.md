# The redesign as built

Screenshots of the editor itself, for comparing the implementation with the
mockups beside this folder. `before/` is TonePush as it was when the work
started (0.7.0's layout); the files here are the editor after the latest
stage, named `<scene>-<width>x<height>-<theme>.png`. Each stage's commit holds
the screenshots as that stage left them.

## How they are made

They are the real interface, drawn offscreen by `src/screenshots.rs` in
`tonepush-gui`: an ignored test that builds the app with invented content,
renders whole frames on the GPU through egui's wgpu renderer at a scale of one,
and writes PNGs. No pedal is opened (the app's device channels lead nowhere and
the StompStation PRO panel does not connect in test builds), nothing is sent to
TonePush's site, and the data directories point at scratch copies, so nothing
personal can appear. Line 6's model artwork is left out by giving the run a
copy of HX Edit's data without its picture folders; model and knob names are
HX Edit's.

```sh
TONEPUSH_SCREENSHOTS=out \
TONEPUSH_LIBRARY=scratch/library TONEPUSH_BACKUPS=scratch/backups \
TONEPUSH_CONFIG=scratch/config.json HX_RESOURCES_DEST=scratch/hx-resources \
TONEPUSH_SITE=http://127.0.0.1:9 \
gpu-lock cargo test -p tonepush-gui --lib screenshots -- --ignored --test-threads=1
```

`TONEPUSH_SCREENSHOT_SCENES`, `TONEPUSH_SCREENSHOT_SIZES` and
`TONEPUSH_SCREENSHOT_THEMES` narrow a run. The renderer refuses a software
rasterizer and prints the adapter it used; these were drawn on an NVIDIA
GeForce RTX 3090 through Vulkan.

## Before

0.7.0 had no light theme, so `before/` is dark only, each at 1024 × 640,
1280 × 760 and 2560 × 1440: the HX Stomp editor (`hx-edit`), the StompStation
PRO editor (`pro-edit`), the window with no pedal (`no-device`), and the
floating windows stage 2 retires: the HX's device window
(`hx-device-window`), global EQ (`hx-eq-window`) and preferences
(`hx-preferences-window`), and the PRO's device window
(`pro-device-window`).

## Stage 1: tokens and type

Every colour is a token of the dark or light palette, the category palette
is the one shared with tonepush.rocks, and Inter is the only face. The 0.7.0
layout was still there, so stage 1 showed the new colours, type and controls
on the old frame. Every confirmation is the one dialog component, named by its
outcome and counting what it writes (`setlist-confirm`).

- Putting a setlist on the pedal still writes every slot, as TonePush does
  today, and the dialog says so. Writing only the slots that differ is one of
  the design's open questions and is not built.

## Stage 2: the frame

The sidebar, full height: the device card (the pedal, how it is connected,
and a menu to let it go or look for one), the Edit · Library · Pedal switch,
the presets in the pedal's own banks with the library's marks, and the foot
with the pedal's protection and TonePush's settings (the appearance, System,
Dark or Light, and the version with the update offer). The deck over the Edit
page: slot, name, the state in words, snapshots, tempo with Tap, undo, redo
and Save. The Library and Pedal pages keep the loaded preset in a one-line deck
with its chain in colours. Ctrl+B hides the sidebar.

The status bar is gone. The HX's device, EQ and preferences windows are the
Pedal page's tabs (Backups, Impulse responses, Favorite blocks, Global EQ,
Settings, Activity), and the PRO's device window is its Pedal page (Backups,
the IR and NAM libraries, Settings). The activity log is the Activity tab.

Scenes: `hx-edit`, `pro-edit`, `no-device`, `setlist-confirm` (now on the
Library page), `hx-library` (a tone on its way to a slot: the sidebar is the
destination), `hx-pedal`, `hx-pedal-irs`, `hx-pedal-eq`, `hx-pedal-settings`,
`pro-pedal` and `settings`.

- The chain, the block pane and the model shelf on the Edit page, and the
  library's tables and inspector, keep their 0.7.0 shapes inside the new
  frame; later stages redraw them. So does the Pedal page's settings list.
- With no pedal the Edit page still shows the empty chain and editor; the
  connect page comes in stage 7.
- Discard stays in the deck. The mockup's deck has no way to throw changes
  away and the editor has always had one, so a discard button sits before
  undo while there are changes to discard.
- The state line says "Changes not saved" without a count: the undo history
  counts bursts of edits, not changes, so a number would be made up.
- Keeping a preset and marking it a favourite moved from buttons on every row
  into the preset's menu; the rows show the state as marks, as the design
  draws them. The menu leaves out the mockup's F2, Ctrl C and Ctrl V hints,
  because those keys do nothing in TonePush.
- StompStation PRO slots read 01A to 20C, as the pedal's home screen does.
  This is one of the open questions; it is only a label, and the command line
  keeps its 1-based numbers.
- A PRO on firmware TonePush has not been verified against says so in the
  deck and the foot ("Read only on firmware …"); before, only the status bar's
  message did.
- The window opens at 1280 × 760, the design's reference, and no smaller than
  1024 × 640, the smallest size it was drawn for.
