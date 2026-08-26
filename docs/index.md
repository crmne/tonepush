---
layout: home
title: TonePush Editor
description: One open-source editor and tone library for Line 6 HX pedals and the StompStation PRO.
permalink: /
hero:
  name: TonePush
  text: Your pedals, one great editor
  tagline: The same polished editor, local library, setlists, and Cloud for Line 6 HX pedals and the StompStation PRO on Linux, macOS, and Windows.
  actions:
    - theme: brand
      text: Download
      link: /download/
    - theme: alt
      text: What is TonePush?
      link: /what-is-tonepush/
    - theme: alt
      text: GitHub
      link: https://github.com/crmne/tonepush
  image:
    src: /screenshot.png
    alt: "TonePush editing a preset on an HX Stomp: a wah, distortion, amp and cab along the main line with a second cab on a parallel branch, the wah's knobs and its expression pedal assignment below, and the library along the bottom"
    width: 2000
    height: 1300

features:
  - icon: 🎛️
    title: Your whole rig at a glance
    details: Modelled blocks and real knobs laid out like the pedalboard they are, with the controls and routing each connected processor can actually provide.
  - icon: 🐧
    title: Every OS, including Linux
    details: TonePush covers macOS and Windows, plus the Linux machine already sitting next to your pedalboard.
  - icon: ⚡
    title: Small and quick
    details: A few megabytes, opens in a blink, connects in a moment. No launcher, no installer ceremony, no waiting.
  - icon: 🔓
    title: Improving in the open
    details: TonePush is open source and moving, with reusable HX and VoidX protocol crates for anyone to build on.
    link: https://github.com/crmne/tonepush/blob/main/PROTOCOL.md
    link_text: Read the protocol
  - icon: ⌨️
    title: Script everything
    details: A full command-line tool for backups, preset changes, and parameter tweaks. Automate your rig like the computer it secretly is.
  - icon: 🛡️
    title: Safe with your presets
    details: Presets travel as each device's own data, byte for byte, and persistent PRO writes have a verified rollback guard. What you save is exactly what was there.
---

<style>
  /* The hero image slot is sized for a square logo; the screenshot needs the
     room. Page-scoped overrides, so the theme stays untouched. */
  .VPHero .image-container {
    width: 100% !important;
    height: auto !important;
    transform: none !important;
  }
  .VPHero .image-src {
    position: relative !important;
    top: auto !important;
    left: auto !important;
    transform: none !important;
    width: 100% !important;
    height: auto !important;
    max-width: 100% !important;
    max-height: none !important;
    padding: 0 !important;
    border-radius: 12px;
    box-shadow: 0 12px 48px rgba(0, 0, 0, 0.45);
  }
  @media (max-width: 959px) {
    .VPHero .image {
      margin: 0 0 24px !important;
    }
  }
</style>

<div style="max-width: 960px; margin: 3rem auto 0; text-align: center;">
  <h2 style="font-size: 1.5rem; font-weight: 600; margin-bottom: 0.75rem;">Find your sound on TonePush</h2>
  <p style="color: var(--vp-c-text-2); max-width: 640px; margin: 0 auto;">
    Find a Song (the musical idea), choose a playable Tone made for your hardware,
    and publish your own Songs and Tones from the editor. Visit
    <a href="https://tonepush.rocks">tonepush.rocks</a> to find your sound.
  </p>
</div>
