---
title: StompStation PRO
description: Use TonePush’s full editor, library, Cloud, NAM, IR, and verified backup workflows with the Sonulab StompStation PRO.
nav_order: 3
---

The StompStation PRO uses a different USB protocol and has a different fixed
signal chain from a Line 6 pedal. It does not use a different TonePush. Once it
connects, the same editor appears: the pedal and its presets in the sidebar,
the loaded preset across the top, and the same three pages. **Edit** holds the
modelled signal chain and the selected block's knobs, **Library** the full
local and Cloud library, and **Pedal** the pedal's backups, NAM and IR
libraries and settings.

TonePush support is hardware-tested with a StompStation PRO running firmware
1.5.12. The protocol implementation is independent and based on the vendor's
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

Select a chain tile to edit it. Numeric parameters use the same knobs as the HX
editor, two-state parameters use the same switches, and enumerated parameters
use the same selectors. Changes go to the live edit buffer and are audible
immediately. The dot beside the preset name means the edit buffer differs from
the saved preset; Save commits it.

Tiles show the selected model rather than only the block type:

- Compressor shows its `Dyn` or `Studio` mode.
- Pitch, modulation, and reverb show their selected algorithms.
- Drive and Amp show the selected NAM model.
- Cab / IR shows the selected impulse response.

Selecting one of those tiles opens the same right-hand model shelf. For NAM and
IR blocks the shelf is populated from the pedal's library; for compressor,
modulation, pitch, and reverb it contains the choices advertised by the live
schema.

## Local tones, setlists, and Cloud

The Library page is the same one used with an HX pedal:

- **Tones** stores byte-exact `.vxpreset` bytes in TonePush's content-addressed
  local library, with names, tags, ratings, Song details, immutable versions,
  and a chain summary.
- **Setlists** captures all 60 PRO slots in order. A setlist can be restored as
  a whole, or one tone can be sent back to one chosen slot.
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
cross-device tones as separate objects.

## NAM and impulse-response libraries

The Pedal page holds the PRO-specific libraries and backup tools, a tab each.
These are capabilities of the pedal, not a second preset UI:

- Import/export, rename, reorder, or clear NAM amp and NAM drive models.
- Import/export mono 48 kHz WAV impulse responses.
- Split one stereo 48 kHz WAV across a chosen left/right pair, or reconstruct a
  stereo WAV from two slots.

TonePush validates NAM JSON, gzip size, WAV shape, sample rate, channel count,
slot capacity, and every upload readback before reporting success. Renaming or
removing a NAM/IR item scans the current preset library and refuses the change
while any preset still names it.

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
wait for you to choose a complete backup. The line under the preset's name says
which it is ("Protected by the 14:02 backup", or "Saving waits for a backup of
this pedal"), and the foot of the sidebar keeps saying it on every page.

On a new machine, open the Pedal page and choose **Back up to a file…** on its
Backups tab, or **Use an existing backup…** for a bundle taken earlier. A
bundle contains:

- exact fixed-size bytes for all occupied presets, IRs, NAM amps, and NAM
  drives;
- every empty slot and library constraint;
- the device schema and safe global settings;
- a SHA-256, size, and manifest record for every file.

The completed directory is published atomically and with private filesystem
permissions. Load it as the current rollback once; persistent actions become
available only after identity, firmware, live names, slot contents, and safe
settings agree. A backup goes into a new folder or replaces an earlier bundle;
TonePush refuses a folder that already holds anything else rather than replace
it. Restore preflights the source before its first write and keeps
the original armed rollback available if the transport fails.

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
