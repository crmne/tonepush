---
title: Using the CLI
description: Script an HX pedal or StompStation PRO, from preset backups to parameter changes.
nav_order: 5
---

The `tonepush` command does everything the editor does, plus a few things only a command line makes convenient: bulk backups, scripted parameter changes, and protocol work.

Commands in this page without a namespace operate on Line 6 devices. The
StompStation PRO uses `tonepush pro …`; see the [StompStation PRO guide](/stompstation-pro/)
for its schema paths, 1-based slots, NAM/stereo-IR operations, and verified
rollback requirements.

## Everyday commands

```sh
tonepush list             # find attached devices
tonepush info             # identity and firmware
tonepush presets          # every preset by name
tonepush select 7         # load index 7, which the device labels 03B
tonepush chain            # the signal chain, named, with values
tonepush topology         # the chain as it is wired, one row per branch
tonepush set 4 Drive 5.0  # set a parameter by name, in displayed units
tonepush enable 4 off     # bypass a block
tonepush snapshot 2       # switch snapshot
tonepush move 4 5         # reorder blocks
tonepush save             # commit the edit buffer; without this, edits are lost
```

Preset indices are zero-based within a setlist: index 7 is `03B`. Parameter values are typed in the units HX Edit displays (`5.0` on a knob shown 0..10, `100` on a percentage, `Limit` on a switch) and converted for you.

## Backups and files

```sh
tonepush backup tone.bin  # the loaded preset, byte for byte
tonepush restore tone.bin # and back again
tonepush backup-all dir/  # every preset in the setlist, one file each
tonepush export tone.json # human-readable export, for diffing
tonepush import a.hlx --dry-run   # preview an .hlx; needs no hardware
```

A backup is the device's own preset document, untouched. Restoring one puts back exactly what was there, including anything this editor does not model.

## Blocks, routing, settings

```sh
tonepush copy-block 1 3   # copy a block over another slot
tonepush copy-snapshot 1 2
tonepush route 0 "Return" # route an input or output
tonepush setting 203      # read a device setting; set-setting writes
tonepush rename 7 "New Name"
tonepush ir-load 1 cab.wav
```

## Watching and poking

```sh
tonepush watch            # stream front-panel activity
tonepush models           # browse the catalog; needs no hardware
tonepush decode capture.log   # decode a USB capture; needs no hardware
```

`watch` prints what the device reports as you touch the front panel, which is also the fastest way to discover what an unlabelled control actually sends. `decode` replays the captures the protocol documentation was reconstructed from; the [opcode dictionary](/opcodes/) is the map.
