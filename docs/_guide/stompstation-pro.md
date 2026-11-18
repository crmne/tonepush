---
title: StompStation PRO
seo:
  title: Sonulab StompStation PRO editor
description: Use TonePush’s full editor, library, Cloud, NAM, IR, and verified backup workflows with the Sonulab StompStation PRO.
nav_order: 3
---

The StompStation PRO uses a different USB protocol and has a different fixed
signal chain from a Line 6 pedal. It does not use a different TonePush. Once it
connects, the same editor appears: the pedal and its presets in the sidebar,
the loaded preset across the top, the modelled signal chain and the selected
block's knobs under it, and the full local and Cloud library along the bottom.
A click on the pedal's card opens its own pages: its backups, NAM and IR
libraries, settings and firmware.

TonePush support is hardware-tested with a StompStation PRO running firmware
1.5.12, 2.0.10 and 2.2.6: every write it makes was read back on each. Patch
releases are treated alike, so a pedal on 1.5.10 or 2.0.8 saves too. On
firmware TonePush does not know yet, it opens the pedal read only: browsing,
export, backups, picking presets and turning knobs work, and nothing is
written to the pedal's memory. On 2.0.x, NAM models cannot be reordered,
because that firmware keeps playing the old model after a move until it
restarts.

Firmware 2.2 stops answering over USB once the computer has used the pedal
as a sound card (PipeWire and Windows do so on their own), unless requests
stay small and the link goes quiet between replies. TonePush paces itself
on 2.2 the way VoidX Control does, so a full backup takes about six minutes
instead of two and a half. The protocol
implementation is independent and based on the vendor's
[public VoidX protocol documentation](https://www.voidxdevteam.com/voidx-control/voidx-protocol/)
plus traffic captured from our own pedal.

## Connect

Quit VoidX Control first, connect the PRO over USB, and start `tonepush-gui`.
TonePush discovers the pedal's USB serial port automatically. On Linux,
`install.sh` installs a udev rule matching `SONULAB` / `StompStation PRO`; replug
the pedal after installation so the rule takes effect.

The card at the top of the sidebar shows the identity and firmware actually
read from the pedal.
TonePush enables persistent writes only for the hardware identity and firmware
combination that was verified during development. An unknown future firmware
can still be inspected without pretending its write behavior is unchanged.

## Edit with the same TonePush UI

The Edit page draws the PRO's chain on the same board as an HX's: its blocks
as tiles between the input and output jacks, in the pedal's fixed order on
firmware 1.5.12, and as the pedal routes them on 2.x (below). A tile says what
its block holds:

- Compressor shows its `Dyn` or `Studio` mode.
- Pitch, modulation, and reverb show their selected algorithms.
- Drive and Amp show the selected NAM model.
- IR shows the selected impulse response.

A dashed tile marked OFF is a block that is off. Click a tile, or a jack for
the input and output settings, to edit it below the board. The block's on/off
is drawn as the footswitch it is; a NAM model or an impulse response is a wide
cell whose **Change** lists every one the pedal holds; every other control is a
knob, a switch or a menu, as the pedal's live schema describes it. Drag a knob,
or click its reading to type a value; double-click it for the default. Changes
go to the live edit buffer and are audible immediately. The line under the
preset's name says when the edit buffer differs from the saved preset; Save
commits it. On a large window the pedal's protection and what your library
knows of the preset sit beside the block.

### The chain on firmware 2.x

Firmware 2.x replaced the fixed chain with a router: fourteen positions, each
holding a block or nothing, joined in series or in parallel. The board draws
it as the pedal reports it:

- An empty position is a narrow dashed slot. Click its **+** to choose a block
  for it from the ones the chain does not hold, by category.
- The amp, the delay and the reverb are fixed in their positions and wear a
  lock.
- Positions joined in parallel are a bank, stacked as lanes between a split and
  a sum. Every block in a bank gets the same input, and their outputs are added
  together.

To change the chain:

- Drag a block onto another position to swap the two; an empty position takes
  it.
- Point at the wire between two positions, or between two lanes of a bank, and
  click the chip that appears to run them in parallel, or in series again.
- Right-click a block to replace it, run it in series or in parallel with a
  neighbour, or take it out of the chain; the block's head under the board
  offers **Replace** and remove too. A block taken out goes silent and keeps
  its settings until another preset loads, but they are not saved with the
  preset.
- The **…** at the top right of the board loads the default chain, the one
  every preset saved on 1.5.12 plays, or clears every position that is not
  fixed.

Each change goes to the pedal as one whole chain, and the board shows the
chain the pedal reads back afterwards. Undo and Redo take changes back one at
a time, and a changed chain is an unsaved change until you save. Like turning
a knob, it changes only the live preset, so it works on firmware TonePush has
not verified for saving too.

How a bank mixes differs between firmware versions, as read from the firmware
rather than measured: on 2.0.10 an empty position in a bank passes nothing, so
its wires are drawn dashed; on 2.2.6 it passes the dry signal, so removing a
block from a bank there also offers to run the rest of the bank in series.

Saving stores the chain with the preset, and an exported `.vxpreset` keeps it.
A preset saved on 1.5.12 has no chain of its own and loads the default one,
which is also what its preview in the library shows.

## Local tones, setlists, and Cloud

The library under the editor is the same one used with an HX pedal:

- **Tones** stores byte-exact `.vxpreset` bytes in TonePush's content-addressed
  local library, with names, tags, ratings, Song details, immutable versions,
  and a chain summary. A click plays a tone in the loaded preset's place until
  you keep it or put it back.
- **Setlists** captures all 60 PRO slots in order. A setlist can be restored as
  a whole, or tones can be put back in chosen slots: a slot that holds a
  preset asks first, and without a checked backup the question offers to take
  one before it writes.
- **Cloud** searches compatible StompStation PRO tones, downloads them into the
  local library, auditions them in the live edit buffer, and publishes native
  `.vxpreset` artifacts with the same Song/Tone metadata workflow.

VoidX's [public protocol](https://www.voidxdevteam.com/voidx-control/voidx-protocol/)
defines preset lists and their byte payloads but does not define a portable PRO
preset file extension or an outer file container. `.vxpreset` is therefore a
TonePush extension for that exact protocol payload: export removes only the
slot's fixed-capacity padding, and import validates and restores it. It is not a
translation into an invented preset model, and TonePush can add an official
vendor container alongside it if VoidX publishes one later.

Right-click a preset in the list and choose **Keep in library** to keep it, or
**Update in library** when your library holds a different version under the
same name. A check beside a preset means your library holds it unchanged, and
an orange compare mark that it holds a different version. The computer icon at
the top of the preset list captures the whole pedal. To send a local or Cloud
tone, press its pedal action and choose the destination in the actual preset
list; free slots are amber, and an occupied row says what it would replace
before you click.

HX and PRO tones share one library without becoming interchangeable. TonePush
routes only a compatible native artifact to the connected pedal, prevents a
setlist for one family being written to the other, and keeps identically named
cross-device tones as separate objects. A PRO tone's marker says the firmware
its chain needs: **PRO 2.x** when its preset has a chain layout, **PRO 1.5**
when it has none. A PRO on 2.x plays both, a 1.5 tone with 2.x's default
chain; a PRO on 1.5 cannot play a 2.x tone, and its row says so. Firmware is
compared by release, so 1.5.10 and 1.5.12 count as the same.

## NAM and impulse-response libraries

The pedal's own pages hold the PRO-specific libraries and backup tools, a tab
each.
**NAM amps**, **NAM drives** and **Impulse responses** each list the library's
slots with what each file says about itself (the gear a capture models, who
captured it and its WaveNet size; an impulse response's length) and how many
presets play it, read from the checked backup that guards the pedal. Choose a
slot for its details, the presets that use it, and what can be done with it:

- Export it; rename or remove it; replace it with a file, or move it up or
  down a slot, from its menu.
- Import NAM models or mono 48 kHz WAV impulse responses into the first free
  slot. Dropping a WAV on the window does the same; a stereo 48 kHz WAV takes
  two free slots side by side, left then right.
- A stereo pair (two adjacent slots TonePush named `… L` and `… R`) shows as
  one row with both sides' shapes, and exports as one stereo WAV or either side
  alone.

TonePush validates NAM JSON, gzip size, WAV shape, sample rate, channel count,
slot capacity, and every upload readback before reporting success. Presets find
a model by its name, so renaming or removing a NAM/IR item scans the current
preset library and refuses the change while any preset still names it; the
page says so beside the buttons it disables.

## The rollback guard

PRO presets, IRs, and NAM models live in flash. Before TonePush makes a
persistent change, it requires a complete verified backup matching the pedal's
current state. On connection it automatically finds and arms the newest
matching `.vxbundle`, so normal Save and library actions need no extra ceremony.
If only settings changed, TonePush refreshes the schema while reusing library
slots whose names and contents still verify: presets and IRs are compared byte
for byte, and NAM models by their first chunk and the gzip trailer, whose
CRC-32 covers the whole model. A preset saved over itself under the same name
is therefore noticed and read again. Full reads batch several
strictly identified chunks per protocol frame. A pedal with no prior bundle
still opens immediately for live editing; only Save and other flash operations
wait for a complete backup. The line under the preset's name says which it is
("Protected by the 14:02 backup", or "Saving waits for a backup of this
pedal"), and the foot of the sidebar keeps saying it on every page.

On a new machine, **Back up to unlock saving** takes Save's place, under a
strip that says why: it reads the whole pedal into a checked backup in
TonePush's backups folder and arms it. **Use an existing backup…** beside it,
or on the pedal's Backups page, checks a bundle taken earlier against the
pedal instead, and **Back up to a file…** there puts one where you choose. A
bundle contains:

- exact fixed-size bytes for all occupied presets, IRs, NAM amps, and NAM
  drives, and on firmware that has them (2.4) the Parametric NAM amps and
  drives and the VxAmp models;
- every empty slot and library constraint;
- the device schema and safe global settings;
- a SHA-256, size, and manifest record for every file.

The completed directory is published atomically and with private filesystem
permissions. Load it as the current rollback once; persistent actions become
available only after identity, firmware, live names, slot contents, and safe
settings agree. A bundle has to hold every library the pedal has, so one that
lacks a library the pedal now has, such as a backup made before an update added
Parametric NAM, is not accepted as a rollback: take a new backup. A backup goes
into a new folder or replaces an earlier bundle; TonePush refuses a folder that
already holds anything else rather than replace it. Restore preflights the
source before its first write and keeps the original armed rollback available
if the transport fails.

## Command line

PRO commands are namespaced so their 1-based slot numbers cannot be confused
with HX slot labels:

```sh
tonepush pro info
tonepush pro list presets
tonepush pro schema 'root\app'
tonepush pro select 1
tonepush pro set 'root\app\amp\gain' 42
tonepush pro export presets 1 clean.vxpreset
tonepush pro export amps 1 amp.nam
tonepush pro export-stereo-ir 1 2 room.wav
tonepush pro backup stompstation.vxbundle
tonepush pro verify-backup stompstation.vxbundle
```

On firmware 2.x, `tonepush pro schema 'root\app\router'` shows the chain as
one row of positions and connectors, and `tonepush pro set 'root\app\router'`
with such a row writes the whole chain, saying so when the pedal reads back
something else: it keeps fixed blocks in place and a block written twice in
its later position.

Persistent CLI operations require both the matching rollback and an explicit
`--yes`:

```sh
tonepush pro save 'My Clean' --rollback stompstation.vxbundle --yes
tonepush pro import presets 12 clean.vxpreset \
  --rollback stompstation.vxbundle --yes
tonepush pro preflight-restore old-rig.vxbundle
tonepush pro restore old-rig.vxbundle \
  --rollback stompstation.vxbundle --yes
```

Run `tonepush pro --help` or `tonepush pro <command> --help` for the complete
argument list.

## Firmware updates

TonePush installs Sonulab's firmware the way VoidX Control does, from the
`.zip` on the [StompStation PRO page](https://sonulab.com/stompstationpro/)
or the `.upd` inside it. The pedal checks nothing it receives, so TonePush
checks the file first: it must be a 64-bit ARM program and either an official
release TonePush lists by its SHA-256, or a release whose file name
(`s_pro_2_2_6.upd`) and contents agree on its version.

On the pedal's own pages, **Firmware** takes you through it in five steps,
always in view: Back up, Update Mode, Write, Restart, Check.

1. Choose the `.zip` or the `.upd`. TonePush checks it, then backs the pedal
   up and checks the backup; Continue waits for both.
2. Start the pedal in Update Mode: unplug its power, leave the USB cable in,
   wait ten seconds and plug it back in, and once the Sonulab logo shows, hold
   UPD for a few seconds until the screen says Update Mode. TonePush moves on
   by itself when it finds the pedal there, reading only who it is.
3. It asks before writing, naming the version, the backup and what it
   restores onto, then sends the firmware, each batch confirmed by the pedal.
4. It says when the pedal has the whole file, counts down the five minutes the
   pedal needs to finish writing, then asks you to unplug its power, wait ten
   seconds and plug it back in, without holding UPD.
5. When the pedal starts again, TonePush checks the version it reports and
   backs it up again: the old backup describes the old firmware.

If an update stops, TonePush says where, keeps the firmware file and the
backup, and tries again from Update Mode. If it starts while the pedal is
already in Update Mode, it connects to it without reading anything else and
offers the update, standing on a checked backup of the pedal from the last day.

```sh
tonepush pro firmware-inspect s_pro_2_2_6.zip
tonepush pro backup before-2.2.6.vxbundle
# Unplug the pedal, wait ten seconds, plug it in, and once the Sonulab
# logo shows, hold UPD for a few seconds until it shows Update Mode.
tonepush pro info        # firmware: Update Mode
tonepush pro firmware-update s_pro_2_2_6.zip \
  --backup before-2.2.6.vxbundle --expect-version 2.2.6 --yes
```

Hold UPD only after the logo appears: held while power arrives, it starts
the pedal's Raspberry Pi in its low-level USB boot mode instead, which shows
nothing on screen and appears on the computer as "BCM2711 Boot". Unplug it
and try again. In update mode the pedal names itself on USB as a Raspberry
Pi serial port; TonePush reads its identity before treating it as the PRO.

In update mode the pedal cannot show its libraries, so the update asks for a
complete, verified backup taken in the last 24 hours. Every batch must be
confirmed by the pedal with the exact number of bytes sent (about a minute
over USB); the pedal writes
the new program only when the last byte arrives, so an update that stops
early changes nothing. TonePush never restarts the pedal. When it reports
that the pedal has the whole file, leave it on for five minutes, turn it off,
wait ten seconds, and turn it on without holding UPD.

Holding UPD always starts the pedal's built-in updater, even when the
installed program does not start, so an update can be repeated, or 1.5.12
installed again. Firmware 2.x keeps presets in larger slots: back up again
after updating, and restore a 1.5.12 backup only to a pedal on 1.5.12.
TonePush opens a pedal on firmware it has not been verified against read
only.
