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

## Stage 3: the HX Edit page

The board: the chain on the dotted surface, tiles that fill it between their
limits (72 to 104 points wide on a small window, 92 to 124 at the reference,
136 to 176 on a large one) with the category's drawing and wash, the name on
two lines, the caption, and tags hung on the top edge for what drives the
block. Wires are one line for mono and two for stereo; forks and merges are
dots on the line, with A/B, XO or DYN under a split that is not a Y; the
endpoints are jacks with their routing. The header counts the blocks and says
what the page is doing ("Trying models in Minotaur's place", "2 blocks on
FS2"). A gap offers a "+", the parallel branch is offered dashed under the
line, and a notch in the board's edge points at the selected block.

The pane: the selected block's head (its drawing, its name as the model
switch, what it is, Change model, copy, paste and remove, and the Block ·
Footswitches · Snapshots switch) and its face: the on/off switch drawn as the
footswitch it is, in a column of its own, and the controls in balanced rows of
the largest knobs that fit (68 down to 44 points, 76 on a large window).
"Controls on this block" replaces the ASSIGNMENTS table. The floor along the
bottom is the pedal: its footswitches with their LED rings and what each
carries, its expression pedals, and how many CCs reach the preset. A chip
opens its switch in the Footswitches lens.

The lenses: the model browser (Recent and HX Edit's categories with their
counts, the shelves, cards with each model's controls drawn at their
defaults, search, a list view, and the audition bar), Footswitches (every
switch, pedal and MIDI as a row with what it carries, and the chosen one's
name, light, press and the two ends of each control it carries, as that
parameter's own knobs) and Snapshots (which blocks each snapshot turns on,
what follows the snapshots, and the tempo each keeps). On a large window the
block, the footswitch board, the controls, the snapshot matrix and what the
library knows of the preset are on screen together.

Scenes: `hx-edit`, `hx-browser` (Teemah! being tried in Minotaur's place),
`hx-footswitches` (FS2) and `hx-snapshots`; `settings` and `no-device` show
the new Edit page too. Their `before/` is `before/hx-edit`, where the shelf,
the knobs and the ASSIGNMENTS table share the page.

- Trying a model is the audition the design describes. The pedal's worker
  keeps the preset as it was before the first try: Put back (Esc, or closing
  the browser) restores it byte for byte with its undo history, and Keep
  (Enter) makes all the tries one undo step. Anything else done meanwhile
  keeps what is playing, as the shelf always did. Adding a block is one
  click, and the new block is selected when the preset comes back.
- The rail has no Favorites. The pedal's favourite blocks are names it keeps,
  and TonePush has no way to put one in a slot; they stay on the Pedal page.
  Recent is kept in TonePush's settings file.
- Values set per snapshot show for the snapshot on the pedal only: the others
  need the snapshot controller values decoded, which the design leaves for
  later. The controls card shows that value as a chip with the snapshot's
  name; switching snapshots shows the next.
- There is no "Add: click a block or a knob" in the Footswitches editor and no
  Assign control mode: a control is given by right-clicking a knob or the
  on/off, or clicking its name, and the lens says so. The lens also leaves
  out the MIDI channel and the HX Stomp's footswitch mode, which TonePush does
  not read.
- The header says "7 blocks" rather than "7 of 8 blocks": TonePush does not
  know how many more a preset can take.
- The board's height is the chain's, as the design draws it; the draggable
  divider under the old chain is gone.
- Tiles keep to drawings, as the design proposes; where HX Edit's pictures are
  installed, the block's head shows the model's picture in its well. An
  Amp+Cab's face shows the cab's controls under a rule with the cab's name,
  which no mockup draws.
- On a small window the browser's shelves move from its head to above the
  models, and the deck says one thing at a time.
