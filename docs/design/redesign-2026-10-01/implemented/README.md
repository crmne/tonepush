# The redesign as built

Screenshots of the editor itself, for comparing the implementation with the
mockups beside this folder. `before/` is TonePush as it was when the work
started (0.7.0's layout); the files here are the editor after each stage, named
`<scene>-<width>x<height>-<theme>.png`.

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

`TONEPUSH_SCREENSHOT_SCENES` and `TONEPUSH_SCREENSHOT_SIZES` narrow a run.
The renderer refuses a software
rasterizer and prints the adapter it used; these were drawn on an NVIDIA
GeForce RTX 3090 through Vulkan.

## Before

0.7.0 had no light theme, so `before/` is dark only: the HX Stomp editor
(`hx-edit`), the StompStation PRO editor (`pro-edit`) and the window with no
pedal (`no-device`), each at 1024 × 640, 1280 × 760 and 2560 × 1440.

## Stage 1: tokens and type

Every colour is a token of the dark or light palette, the category palette
is the one shared with tonepush.rocks, and Inter is the only face. The 0.7.0
layout is still there, so these show the new colours, type and controls on
the old frame. Every confirmation is the one dialog component, named by its
outcome and counting what it writes (`setlist-confirm`).

- The light theme follows the desktop; the setting that overrides it arrives
  with the sidebar's settings in stage 2.
- Putting a setlist on the pedal still writes every slot, as TonePush does
  today, and the dialog says so. Writing only the slots that differ is one of
  the design's open questions and is not built.
