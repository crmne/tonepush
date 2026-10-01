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

## Stage 4: the Library and Pedal pages

Library, Tones: one row per tone with where it is (the pedal's mark, the
computer's, TonePush's), its chain as a strip of category colours, song and
artist, character, rating and the day it was kept; PRO tones say so. Which
columns show is a menu beside the search (and the header's right-click
menu), and the tags are a filter menu beside it. The inspector holds the
tone's name, its chain as wells, where it is with Send to a slot, its song and
tone details, rating, tags, notes, versions, Publish and Export.

Library, Setlists: a card per setlist saying first how it stands against the
pedal ("Matches the pedal", "6 slots differ", "For StompStation PRO"), and the
chosen one's page: its name, venue and date edited in place, its version,
Capture the pedal as the next version, Put on the pedal, and its menu. The
banner says what differs in a sentence and narrows the banks to those slots;
the banks show replaced slots in amber with the compare mark, and the slots
the setlist would empty dashed and named. A slot sends its own preset back on
a double-click or from its menu.

Putting a setlist on the pedal (06) counts what the write changes, read from
the pedal's backup: the presets replaced and the slots emptied or filled by
slot, and those that already match.

Pedal, Backups (08): how the pedal is protected, then every copy of it on this
computer, newest first: the automatic backup kept current ("Refreshed after
saving 01B"), the copies set aside before it was refreshed ("Pedal
connected", "Before a setlist: Before Album release show was written",
"Before presets were sent", "Before a restore"), and the backups saved to a
file. The chosen copy is compared with the pedal now, preset by preset
("Ambient Swell edited since", "Sparkle Verb was Night Verb"), and can be
put back whole or shown in its folder. The Settings tab is redrawn in the same
cards, with the two states of a switch side by side, choices in a menu and the
tempo on a slider beside a field to type it.

Scenes: `hx-library`, `hx-setlists` (new: Album release show against the
pedal), `setlist-confirm`, `hx-pedal` (the backups, the copy before Album
release show chosen), `hx-pedal-settings`, and `pro-pedal`, whose banner now
spans its column as the design's banners do. Their `before/` is
`before/hx-edit` and the stage 2 windows.

- Putting a setlist on the pedal still writes every slot, as TonePush does
  today: writing only the slots that differ is one of the design's open
  questions. The dialog says so, counts the slots that match as "written again
  as they are", and its button says "Write 126 slots". Its footer asks to keep
  the pedal connected rather than estimating a time TonePush does not know.
- "Keep the pedal as it is now as a setlist first" works: the pedal is read
  into the library as the next version of the setlist named after it (its
  description says which setlist it came before), without asking for a name,
  and nothing is written until that copy is saved. A capture that could not
  keep every preset stops the write.
- Restoring single presets, and "Restore these 6 presets…", are not built:
  that is the other open question. The comparison lists every preset that
  differs, and Restore the whole pedal… writes the copy back after a
  confirmation that counts what it writes. Restoring from a file now asks the
  same question first (it used to start at once), and a backup from another
  kind of pedal is refused before anything is asked.
- The Checked column says "Complete" (every preset, impulse response and the
  settings the copy names are in it), not "Verified": an HX backup has no
  checksums to verify against, unlike the PRO's.
- Why a copy was set aside is written inside it from now on; copies taken
  before this say "An earlier copy". The worker names the setlist when a whole
  setlist is written, and the slots when presets are sent on their own.
- Backups saved to a file appear in the history from now on: TonePush keeps
  the last twenty in its settings file, and leaves out any that have moved.
- The mockup lists Summer tour (for a StompStation PRO) while the library is
  scoped to the HX Stomp; TonePush keeps another family's setlists out of a
  pedal's scope, as it does today, and shows them with "For StompStation PRO"
  under All pedals.
- The tags rail and the Favorites filter are a filter menu beside the search,
  and the columns are a menu; at the smallest size the table scrolls sideways
  rather than dropping columns, and the columns are chosen from the header.
- No mockup draws the Settings tab, the impulse responses, favourite blocks,
  the global EQ or the activity log; they use the stage 2 components.

## Stage 5: the StompStation PRO

The Edit page: the PRO's chain on the HX's board, its blocks as the same
tiles between the input and output jacks, each saying what it holds (the
compressor's mode, the pitch, modulation and reverb algorithms, the NAM model
in Drive and Amp, the impulse response in IR). The chain is read from the
pedal's schema rather than written down, so a firmware that lists other
blocks gets them as ordinary tiles. The pane under it is an HX block's: the
head with the block's drawing in its well and what it plays, then the face,
the on/off drawn as the footswitch it is, a NAM capture or an impulse
response as a wide cell whose Change lists every one the pedal holds, and the
other controls as knobs (a reading can be typed), switches and menus in
balanced rows of the largest knobs that fit. On a large window the pedal's
protection and what the library knows of the preset sit beside the block.

Before a backup of the pedal matches it (21), Back up to unlock saving takes
Save's place under a strip that says why, with Use an existing backup beside
it; it reads the whole pedal into a checked backup where TonePush keeps its
own, and arms it.

The Pedal page: NAM amps, NAM drives and Impulse responses (10, 11) are each a
table of the library's slots, with what each file says about itself (the gear
a capture models, its WaveNet size; an impulse response's length) and how many
presets play it, and an inspector for the slot chosen: the file's facts, the
presets that use it, Export, Rename and Remove, and a menu to replace it with a
file or move it a slot. Stereo pairs are one row, bracketed across their two
slots, with both sides' shapes. Settings is drawn as the HX's is.

Scenes: `pro-edit`, `pro-unprotected` (new), `pro-nam` (new), `pro-irs` (new),
`pro-settings` (new) and `pro-pedal`. Their `before/` is `before/pro-edit`
and `before/pro-device-window`.

- The design draws firmware 2.x: sixteen positions with free slots, fixed
  blocks with locks, parallel lanes and the mono and stereo wires. TonePush
  writes to firmware 1.5.12, whose chain is twelve blocks each always in its
  place, and does not read 2.x's chain yet, so the board draws the twelve in
  the pedal's order with no free positions, no locks and no legend; the header
  says "12 blocks · in the pedal's fixed order". The tiles' short names
  ("Comp", "Mod", "Pre Mod") keep every word whole in a tile.
- There is no floor and no Quick controls or Controllers lens: the F1 to F4
  quick controls and the controllers are 2.x's, and 1.5.12's controller
  modules stay hidden as before. Copy settings is not built.
- A 1.5.12 amp plays one capture, so its model cell is one, without Pan.
- The right-hand model shelf beside the editor is gone: Change on the model
  cell chooses a model, and managing the slots moved to the Pedal page. So is
  the block's Filter parameters field: a PRO block has a handful of controls,
  all on its face. The Settings tab keeps its filter.
- What a file says about itself and which presets play it come from the
  checked backup that guards the pedal, which holds every file and preset;
  nothing extra is read from the pedal. A file imported since that backup
  shows without facts until the next one, and the counts are as of that
  backup: the worker still asks the pedal itself before renaming or removing
  anything, as it always has.
- Back up to unlock saving names the backup the way TonePush names the ones it
  takes on its own, and gives no time estimate. The large pane's protection
  card offers Back up now, which takes a fresh backup and guards the pedal
  with it.
- Dropping a WAV on the window while a PRO is the pedal now imports it into
  the first free impulse-response slot (two, for a stereo WAV), and a `.nam`
  into the NAM library open on the Pedal page.
- The Firmware tab comes with stage 6.

## Stage 6: StompStation PRO firmware

Pedal, Firmware (12 to 17): the five steps always in view (Back up, Update
Mode, Write, Restart, Check), the way `tonepush pro firmware-update` installs
Sonulab's firmware and with its checks. Choosing the `.zip` or `.upd` checks
it with `voidx_client::firmware::Image`; the pedal is backed up into a fresh
checked backup and Continue waits for it (12). The pedal is let go and started
in Update Mode by hand, with the drawing of its back and UPD lit (13);
TonePush finds it by itself, reads only its identity, and asks before it
writes, naming the version, the backup and what that restores onto (14). The
write shows what the pedal has confirmed, batch by batch, and how long is left
(15). Once the pedal has the whole file, TonePush counts down the five minutes
it is left on, sees it switched off, counts the ten seconds, and waits for it
(16); when it starts again it checks the version it reports and backs it up
again. A write that stops says where and how to finish (17), with the file and
the backup it still holds and the details for support.

Scenes: `pro-firmware-backup`, `pro-firmware-update-mode`,
`pro-firmware-confirm`, `pro-firmware-writing`, `pro-firmware-restart` and
`pro-firmware-failed` (all new); the PRO's Pedal page gains the Firmware tab
in `pro-pedal`, `pro-nam`, `pro-irs` and `pro-settings`. There was no firmware
update in 0.7.0, so there is no `before/`.

- In Update Mode the pedal names itself a Raspberry Pi serial port. When no
  StompStation PRO is listed, TonePush takes the one port that may be the
  pedal in Update Mode, reads its identity, and uses it only if it says
  StompStation PRO in Update Mode, as the command line does; a device that
  says anything else is sent nothing more. A pedal in Update Mode is asked
  nothing but the update: the worker refuses every other command, and does
  not read its libraries or settings.
- The design's steps say to switch the pedal off and to hold UPD while it
  starts. On hardware, UPD must be held only once the Sonulab logo shows (held
  while the power comes on, it starts the Raspberry Pi's USB boot mode, with a
  blank screen). The screens follow the order tested on the pedal: unplug its
  power, keep the USB cable in, wait ten seconds, plug it back in, and hold UPD
  once the logo shows. They say the boot-mode trap too.
- The command line leaves the pedal on for five minutes after the transfer,
  while it writes the image, before it is restarted; the design goes straight
  to "switch it off". The Restart step counts those five minutes first, and
  says so if the pedal is switched off early. TonePush never restarts the
  pedal.
- The design's "about 3 minutes" and "Checking what was written" are not
  claimed: the transfer takes about a minute on the hardware, and the pedal
  confirms bytes received, not a check of what it wrote. Its footer says the
  transfer runs to the end rather than that it cannot be stopped safely: a
  transfer that stops early leaves the pedal on the firmware it had.
- A pedal already in Update Mode when TonePush starts is offered the update
  on the newest checked backup of a StompStation PRO from the last day, since
  Update Mode cannot be backed up; without one, the page says to start the
  pedal normally first.
- "Sonulab support" opens Sonulab's StompStation PRO page, the one address
  the guide already names.

## Stage 7: connect, and the first run

With no pedal, the Edit page is one calm page (18) instead of an empty chain
and an empty pane: the mark, "Plug in your pedal", both families with what
they cover and whether TonePush is still looking for one, what is already
fine on this computer, what to do first, and Line 6's model data as a step,
with Find the installer and Download HX Edit and a plain note that the
StompStation PRO needs nothing. The library is one click away at the foot,
beside the guide and what is new.

The window that blocked the first run until HX Edit's data was in place is
gone: the step is on this page, and on an HX pedal's block pane when the data
is missing. Machines with HX Edit installed still get its data copied without
asking, and an installer dropped on the window is still read.

Scenes: `no-device` (looking for both families) and `connect-not-found`
(new: nothing found, and no HX Edit data, as on a first run). Their `before/`
is `before/no-device`.

- TonePush looks for a pedal when it starts and when asked; it does not watch
  USB. Once the first look finds nothing, the cards say "Not found" and Look
  again looks once more, where the design shows "Looking on USB" throughout.
- "This computer can talk to USB pedals" checks for the access rule where
  `install.sh` and the packages put it, and shows only on Linux, the one
  system that needs it; without the rule it says how to get it.
- The welcome window's credits line is not carried over; the page keeps its
  licence and the note that TonePush is not affiliated with Yamaha Guitar
  Group. "What's new" opens the latest release.
