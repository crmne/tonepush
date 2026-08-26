---
title: What is TonePush?
description: Why TonePush exists, what it does, and what it honestly does not do yet.
nav_order: 0
---

## The problem

Modern floor processors are small boxes holding large amp-and-effects rigs.
Their vendor editors are closed, usually platform-limited, and rarely share a
good library workflow with hardware from another maker. If your studio machine
runs Linux, you want to script the pedal, or you want one carefully designed
place for your tones, you are otherwise out of luck.

TonePush is an open-source editor for Line 6 HX-family devices and the Sonulab
StompStation PRO. Both use the same TonePush editor, local library, setlists,
Cloud workflow, and visual language. Small device adapters translate those
shared operations into the very different HX USB and VoidX serial protocols.
It runs on Linux, macOS, and Windows, and the reusable protocol/client crates
make the hardware useful outside this editor too.

![TonePush editing a preset on an HX Stomp: a wah, distortion, amp and cab along the main line with a second cab on a parallel branch, the wah's knobs and its expression pedal assignment below, and the library along the bottom](/screenshot.png)

## What it does

The common workflow is the same whichever supported pedal is connected:

- **Your whole rig at a glance.** Modelled blocks and knobs are laid out like a
  pedalboard. HX routing branches where the hardware can branch; the PRO shows
  the fixed chain and algorithms its live schema advertises.
- **Editing.** Search and swap models, turn the same knobs, use Tap tempo, and
  undo, redo, or save from the same controls and keyboard shortcuts.
- **One library.** Keep native presets locally, freeze a whole pedal as an
  ordered setlist, and discover, audition, download, or publish compatible
  tones through TonePush Cloud.
- **Native capabilities.** HX snapshots, routing, favorites, and global EQ;
  PRO NAM amp/drive libraries, stereo IR pairs, settings, and verified
  rollback/restore. The UI appears only where the pedal can do the operation.
- **Exact files.** A kept tone is the device's own document, byte for byte, so
  nothing is silently rebuilt or lost.

## What it does not do yet

This is a young project, and it says so:

- Hardware verification covers an HX Stomp on firmware 3.80 and a StompStation
  PRO on firmware 1.5.12. Helix and Helix LT parse and render (two DSP paths,
  four lanes), but they have not met real hardware yet.
- The tuner is not here because it is not an HX Edit feature either: it lives on the hardware.
- HX model names, ranges, and artwork come from HX Edit's own data files, which
  are Line 6's and are not redistributed. The PRO describes its controls and
  installed NAM/IR choices itself and needs no vendor asset extraction.

If something misbehaves, [an issue](https://github.com/crmne/tonepush/issues)
with `tonepush chain` or `tonepush pro schema 'root\app'` output and what you
expected instead is gold.

## Where this is going

The editor is part of [TonePush](https://tonepush.rocks). A **Song** there is the musical idea: either a catalog song by an Artist or an original. A **Tone** is one playable, device-native preset belonging to that Song. Players find the Song first, then choose the Tone made for their hardware, or publish both from the editor.

## Prior art

TonePush stands on earlier efforts: [`kempline/helix_usb`](https://github.com/kempline/helix_usb) found the multi-channel structure, [`allansomensi/openhx`](https://github.com/allansomensi/openhx) listed and selected presets in Rust, and [`AntonyCorbett/HelixBackupFiles`](https://github.com/AntonyCorbett/HelixBackupFiles) and [`frankdeath/hx-tools`](https://github.com/frankdeath/hx-tools) decoded file formats.
