# HX protocol notes

Working notes on the USB protocol used by Line 6 HX-family hardware, reconstructed
by observation. Nothing here is derived from Line 6 source code.

Test rig: HX Stomp (firmware 3.80), macOS 27.0 on Apple Silicon, HX Edit 3.82.

Confidence is marked throughout: **[confirmed]** means verified against captured
traffic on this rig, **[inferred]** means a hypothesis that fits the data but has
not been isolated, and **[open]** means unresolved.

## Device topology

HX Stomp enumerates as a composite device, `0e41:4246`
(vendor `0x0E41` = Line 6), device version 2.00, serial `3196883`.

| Iface | Class | Endpoints | Role |
|---|---|---|---|
| 0 | `0xFF/00` vendor specific | `0x01` bulk OUT, `0x81` bulk IN - 512 B | **editor channel** |
| 1 | `0x01/01` audio control | - | audio |
| 2 | `0x01/02` audio streaming | `0x03` isoc OUT, 224 B (alt 1) | playback |
| 3 | `0x01/02` audio streaming | `0x83` isoc IN, 224 B (alt 1) | capture |
| 4 | `0x01/03` MIDI streaming | `0x02` bulk OUT, `0x82` bulk IN - 512 B | musical MIDI |
| 5 | `0x03/00` HID | `0x84` interrupt IN, 8 B | switches/knobs |

Interface 0 has no kernel driver bound to it, and HX Edit holds it exclusively
while running - so a third-party client must wait for HX Edit to quit.

Known product IDs (`0x0E41` vendor): `0x4246` HX Stomp, `0x4253` HX Stomp XL,
`0x4248` Helix Floor. No public PID is known for HX Effects, POD Go, Helix LT or
Helix Rack.

**The editor protocol is not on MIDI. [confirmed]** Every editor transfer in our
captures is on `0x01`/`0x81` after `libusb_claim_interface(0)`. Interface 4 carries
only ordinary musical MIDI. This decides platform reach: iOS gives third-party
apps no raw USB access, so **iOS cannot be supported** unless a MIDI path is
found. Android is unaffected - its USB Host API reaches interface 0.

The device does answer a standard Universal Identity Request on its CoreMIDI port:

    TX  F0 7E 7F 06 01 F7
    RX  F0 7E 7F 06 02 00 01 0C 21 00 06 00 03 50 00 00 F7

giving manufacturer `00 01 0C` (Line 6), family `0x0021`, member `0x0006`
(HX Stomp), software revision `03 50 00 00`. The concatenation `0x00210006` is
the device identifier reused in `HX Edit.prefs` and in the `device` field of
`.hlx` preset files.

### Firmware version encoding [confirmed]

Earlier notes could not decide whether `03 50` meant 3.50 or 3.80. It is **3.80**,
and both encodings appear:

- Over the wire, the preset payload carries `35: 0x03800000` - byte `0x80` read as
  BCD is 80, giving 3.80. Other version gates in `HelixModelDefs.bin` are
  `0x02990000`, `0x03690000`, `0x03790000`; `0x99` and `0x69` are only meaningful
  as BCD, which fixes the encoding.
- Over MIDI, the same version appears as `0x50` = 80 **decimal**. It has to: SysEx
  data bytes cannot exceed `0x7F`, so `0x80` is unrepresentable and Line 6 switch
  to plain decimal for that field.

So read the internal field as BCD and the MIDI field as decimal. Both yield 3.80,
consistent with the paired HX Edit 3.82.

Note the preset also carries a build string `37: 'v3.71-32-g1039661'`, which does
**not** match 3.80 - it is most likely the firmware that last serialised the
preset rather than the running firmware. **[inferred]**

## How these captures were taken

macOS on Apple Silicon cannot capture USB traffic without disabling SIP, and even
then Apple Silicon is reported to return all-zero payloads. Wireshark's ChmodBPF
does not help - it only chowns `/dev/bpf*` and cannot create the `XHC*`
pseudo-interfaces, which stay hidden while SIP is on.

We sidestep packet capture entirely. HX Edit links a **bundled copy of libusb**
(`@executable_path/../MacOS/libusb-1.0.0.dylib`), so every byte it exchanges with
the device passes through a handful of known entry points. `tools/hxsniff`
interposes them with `DYLD_INSERT_LIBRARIES` and logs complete buffers - better
than a packet capture, because message boundaries are exact and no reassembly of
USB transactions is needed.

The shipped app is signed with the hardened runtime, which makes dyld ignore
`DYLD_INSERT_LIBRARIES`, so `run.sh` works on a *copy* of the bundle and re-signs
it ad-hoc. The installed app is never modified. Its entire entitlement set is four
`com.apple.security.cs.*` keys all set to `false`, so dropping the signature costs
nothing functionally.

### Do not call USB reset

`libusb_reset_device` / `nusb`'s `Device::reset` takes the HX Stomp **off the bus
and it does not re-enumerate** - recovering it needs a physical unplug/replug.
This was tried as a way to clear stale session state and cost a reconnect, so it
is called out here rather than left to be rediscovered.

### The device can wedge, and only a power cycle clears it [confirmed]

A channel left mid-conversation by a client that exited without closing can end
up refusing new sessions: it answers every handshake with a bare acknowledgement
carrying its last sequence number and never opens.

This is **not** a defect in this client. Captured evidence: after it happened,
HX Edit 3.82 was started against the same device, sent its own handshake, got
nothing but repeating acknowledgements on channel `0x1001`, and released the
interface without connecting. Line 6's own editor cannot recover it either.

Recovering it needs the **9V adapter pulled**, not just the USB cable: the unit
is externally powered, so it keeps its session across a USB replug. Confirmed by
watching its sequence counter carry on from `0x86` to `0xcc` across a
re-enumeration onto a different bus.

#### The big one: nothing is posted to read between operations [confirmed]

This was the cause of most lock-ups seen during development, and it is a host
bug rather than a device one.

The device emits notifications whether or not anyone asked. If the client only
posts a USB read buffer while a request is in flight - the obvious way to write
it - then between operations the device has nowhere to put them. Once its
outgoing queue is full it stops draining the **incoming** endpoint as well, and
the next write simply times out with nothing visibly wrong. The symptom is a
device that took several operations happily and then refused, so it reads like
something you did on the last one rather than a backlog from all of them.

Draining the endpoint before each request, and acknowledging any channel that
received bytes while nobody was waiting on it, moved this from failing on the
third preset-document write to comfortably past ten. Measured on an HX Stomp
running 3.80.

Two things that look like fixes and are not:

- **Acknowledging the channel a reply just arrived on, before returning.** The
  acknowledgement already rides in the header of every frame the client sends,
  so this adds nothing - and the extra ACK frame burns a sequence number
  mid-transaction. It made the next read time out immediately.
- **Wrapping the acknowledgement at 16 bits.** Failures cluster near where a
  16-bit counter plus the `0x1000` base would overflow, but masking changed
  nothing and sessions still failed with as little as 21 KB received.

**Resolved: deferred operations must be paced on notification 20.
[confirmed]** The sustained-write failure was the last piece of this story. A
deferred operation (select preset, write preset, IR upload) answers status 1 -
accepted - and announces actual completion later as notification 20 carrying
the same transaction id. HX Edit will not start the next such operation until
that notification arrives: fourteen consecutive captured undo writes all follow
the pattern, each taking ~300 ms reply-to-notification. A client that treats
the status-1 reply as completion races its next write against the device's
still-running commit; the device tolerates roughly a dozen racing commits and
then stops accepting writes. With the wait in place, twenty back-to-back
document writes complete and the device stays healthy -
`crates/hx-usb/tests/device.rs` holds the regression test.

Three further things make it more likely, all learned the hard way:

- **Re-running the handshake on an open channel.** This is the big one. An
  earlier version answered a request timeout by redoing the whole handshake, up
  to four times, sending fresh HELLO frames on channels the device already had
  open. Every failure amplified into a burst of them, and a GUI that polls
  continuously produced them fastest - which is exactly when lock-ups happened.
  **Handshake once per session and never again.** Report a timeout instead of
  trying to fix it. **[inferred, strongly]**
- **Sustained reads.** A drain loop that keeps the IN endpoint busy for tens of
  seconds also coincided with lock-ups. Bound it to a few seconds.
- **Opening service 2 on the control channel at connect time.** HX Edit opens
  service 5 first and only reaches for 2 later in its session. **[inferred]**

The general lesson: this device's session layer is not defensive. It assumes a
well-behaved peer that opens each channel once and then speaks in order. Anything
that looks like a second client arriving mid-conversation can wedge it.

There is also a host-side trap that resembles a dead device but is not one. A
client that exits without closing leaves the device acknowledging into a buffer
nobody reads, and that backlog survives process restarts: the next session reads
hundreds of stale acknowledgements with steadily climbing sequence numbers
instead of its own handshake reply. Draining until reads genuinely come up empty
distinguishes the two - a real backlog clears (about 100 frames here), a wedged
device does not.

**The teardown exists, and is a HELLO. [confirmed]** A clean quit, captured
with the interposer, shows HX Edit acknowledging anything outstanding, sending
a bare type-0x02 frame on each of the three channels, collecting the device's
answering 0x02s, and releasing the interface. The 0x02 message is a session
*boundary*, not just an opening handshake - it appears at both ends of the
conversation, and the closing form is just the 8-byte channel header with the
current sequence and acknowledgement. An earlier version of this section
claimed HX Edit sent nothing recognisable on quit; that conclusion came from
`captures/04-quit.log`, which on re-reading records HX Edit failing against an
already-wedged device - repeating stale ACKs answering every HELLO - not a
quit. This client now performs the same teardown when a session drops.

## Layer 1 - USB framing [confirmed]

Every bulk transfer on `0x01`/`0x81` is one framed message:

```
offset  size  field
0       2     originator: 1 host, 0 device                [confirmed]
2       2     service id; garbage on some device replies  [confirmed]
4       4     MessagePack body length, u32 LE
8       n     MessagePack body
```

`tools/hxsniff/reassemble.py` implements exactly this and walks a whole capture
without desynchronising, which is the evidence that the layering is right: a
strict MessagePack reader fails immediately if the framing is off by a byte.

The two leading fields were resolved by a census of every stream message in the
captures (~900):

- **Offset 0 is the originator.** Host messages always carry 1, device messages
  always 0, with zero exceptions. It is not a flags word.
- **Offset 2 is the service id, but the device does not always initialise it.**
  Host messages always carry the true service. Device messages usually do, but
  certain replies - the first large reply on the control channel, and every
  jumbo preset document - carry junk instead: the *same* reply arrives as
  `0x0000`, `0x28e1` or `0x2100` in different captures of the same action, and
  the junk bytes are recognisable residue (frame-flag values, fragments of the
  blob length, even ASCII from neighbouring buffers). The receiver must trust
  only the length at offset 4 and ignore offset 2 on device messages, which is
  what this implementation does.

Two small related facts fell out of the same census: the reply to opening a
service carries a one-byte body echoing the service number, and a capture that
starts mid-session begins mid-message - the framing recovers at the next
message boundary because the length walk stays consistent from there.

## Layer 4### Global settings: opcode 24 reads, opcode 25 writes [confirmed, writing is hazardous]

Device settings are a flat numbered namespace rather than a structured
document. `op24 {118: id}` reads and `op25 {118: id, 119: value}` writes; 147
of the first 160 ids answer on an HX Stomp. Known ids: **16** tempo in BPM,
**28** current preset index, **192** global EQ low-peak gain, **203** global EQ
enabled. HX Edit's Global EQ window reads 201–203 when it opens, and its
Save Preset is a different opcode entirely (**71**, `{107, 108, 109: name}` -
the operation that moves an edit out of the edit buffer and into storage).

The value's type must match what the device already holds; a float where it
wants a boolean is refused with error −3.

Writing works and round-trips. **A note on how nearly it was mis-recorded:**
this section briefly said op25 destabilised the device, on the strength of an
elimination experiment - removing the settings-write test took the hardware
suite from two passing tests to twelve. The inference was wrong. That test also
called `irs()` on a control channel nothing had opened yet, and a cold control
channel does not answer its first request; the resulting timeout left the
session unhealthy and the *next* session unable to open. The write was carrying
the blame for its neighbour. With the health check corrected, sixteen tests
including the settings round trip pass twice over with no power cycle between.

The lesson generalises: elimination finds *a* thing that changes the outcome,
not necessarily the cause. Two changes in one test is one too many.

### Controller assignments [confirmed]

HX Edit's Bypass/Controller Assign page puts any parameter under any source.
Four opcodes cover it:

| Opcode | Arguments | Does |
|---|---|---|
| 37 | `{98: block, 26: 0, 28: param, 29: true, 74: source, 71: 4, 129: false}` | put a parameter under a controller |
| 36 | `{98, 29, 26, 28}` | read a parameter's assignment |
| 56 | `{98: block, 102: switch}` | put a block's bypass on a footswitch |
| 57 | `{98: block, 102: switch}` | take it off again |
| 33 | `{102: switch}` | read a footswitch's configuration |
| 65 | `{98, 29, 26, 28, 119: value}` | the controller's Min |
| 66 | `{98, 29, 26, 28, 119: value}` | its Max |
| 58 | `{102: switch, 65: momentary}` | Type: latching or momentary |
| 59 | `{102: switch, 109: label}` | set the switch's custom label |
| 60 | `{102: switch}` | clear it again |
| 61 | `{102: switch, 66: colour}` | set the switch's LED colour |
| 62 | `{102: switch}` | put the LED back to Auto Color |

**58 to 62 count from zero, like 56 and 57. [confirmed]** Every one of them was
caught in `mac-assign-capture.log` against Footswitch 1 as `102: 0`. Only 33
takes a one-based number.

**Opcode 61 takes a colour *index*, not an RGB value. [confirmed]** The capture
sets White with `66: 1`, and HX Edit's own `HelixControls.json` gives the list
under `footswitchLED`: Auto Color, White, Red, Dark Orange, Light Orange,
Yellow, Green, Turquoise, Blue, Violet, Pink, Off. Index 0 is Auto Color, which
is why 62 exists at all - the one entry of that list you cannot send as a value.

**Opcode 33 answers with that same index. [confirmed]** Setting 1, 2, 5, 6, 8
and 11 through 61 and reading the switch back through 33 gave the same number
every time, and opcode 62 leaves key 66 nil rather than reporting 0. So the
outer key 66 is an index and the key 66 *inside* an assignment entry is a real
`0xRRGGBB`: one field, two dialects, told apart by size, an index being under a
dozen.

The inherited one is the LED as it is actually lit, dimming included. A switch
carrying a bypassed Trinity Chorus reports `1037` = `0x00040D`, a dim blue,
where the same block switched on would report the Modulation colour at full
brightness.

Key **74 is the source**, as an ordinal in the order HX Edit lists them: **0
None**, 1–2 the expression pedals, 3–7 the footswitches, 8 MIDI CC, 9 Snapshots
- `74: 1` for EXP 1 and `74: 9` for Snapshots are ours, from the assign capture.
Key **71 is not the constant it looks like**: it is the assignment's on switch,
`4` when one is made and `0` when it is removed, and removing sends `{74: 0,
71: 0}` rather than a separate opcode.

**Min and Max are opcodes 65 and 66. [confirmed]** Dragging either end streams
one write per intermediate value, the same way the global EQ does. They work,
and there is a trap in checking whether they did: see below.

### Read assignments from the document, not from opcode 36 [confirmed]

Opcode 36 is wrong twice over, and the preset document is right about both.

**It answers with the source at key 0, not at key 74.** Opcode 37 *takes* the
source at 74; the reply puts it at 0. A parameter nothing controls answers `nil`
rather than a map. Reading 74 made every parameter look unassigned, which hid
every assignment the editor had.

**It reports the travel as the defaults, always.** Keys 2 and 3 of the reply
read 0 and 1 however far the ends have been moved. Writing 0.25 and 0.75 through
opcodes 65 and 66 changes the document and does not change what 36 says.

The document carries the lot, so one read answers for every block at once
instead of one round trip per parameter. **Top-level key 4 is an array indexed
by the source's ordinal**: entry 1 is everything Expression Pedal 1 drives,
entry 3 Footswitch 1's. Each item is `{0: place-in-table, 1: assignment}`:

```text
4: [ nil,
     [ {0: 1, 1: {0: 1, 1: 0, 2: nil, 3: nil, 4: 2, 5: 1, 9: 5, 10: 300, ...}},
       {0: 0, 1: {0: 1, 1: 4, 2: 0.25, 3: 0.75, 4: 0, 5: 1,
                  6: {28: 0, 29: 0, 41: false}, 7: 0, 13: false}} ],
     [ {0: 2, 1: {0: 2, 1: 4, 2: 0, 3: 1, 4: 0, 5: 2, 6: {28: 0}, ...}} ],
     nil, nil, nil, nil, nil, nil, nil ]
```

Inside an assignment: **0** the source again, **1** the kind (4 a parameter,
0 a bypass), **2** and **3** the ends of the travel, **5 the block**, **6** the
parameter when it is a parameter, **9** and **10** the bypass target and scope
when it is a bypass.

**Inside key 6, the parameter's index is key 29 and key 28 is the path.
[confirmed]** That is the opposite way round from the *request* that makes the
assignment, where 26 is the path, 28 the index and 29 a commit flag. Reading the
request's shape out of the document made every assignment in every preset report
parameter 0. `CT-Master Lead` has one assignment, on parameter 1:

```text
4: [ nil, nil, nil,
     [ {0: 0, 1: {0: 3, 1: 4, 2: 1, 3: 4, 4: 0, 5: 3,
                  6: {28: 0, 29: 1, 41: false}, 7: 0, 13: false}} ],
     nil, nil, nil, nil, nil, nil ]
```

Its ends read 1 and 4, in that parameter's own units.

**Key 5 is the block; the entry's own key 0 is not. [confirmed]** Key 0 is the
assignment's place in that controller's table, and it equals the block often
enough to look like one: in the sample above the wah's bypass sits at 1 and is
on block 1. Reading key 0 put the wah's Position on the input block. Both were
checked against opcode 36, which agrees with key 5.

The example above is a factory wah preset, and it reads correctly: Expression
Pedal 1 drives block 1's Position *and* block 1's bypass, which is auto-engage,
and Expression Pedal 2 drives the volume block.

**A bypass's Source list holds only footswitches and None. [confirmed]** No
expression pedals - a bypass is a switch - and no MIDI CC either, because MIDI
is not a source for a bypass: the assign page gives it its own **MIDI In** row,
sent as `op37 {98: block, 95: 5, 96: 300, 74: 0, 71: cc}` with no parameter keys
at all, and switched off again by the same message with `71: 0`. So opcode 68,
which the inferred table calls "set MIDI CC", was never sent.

**Key 71 is that CC number. [confirmed]** This paragraph has been wrong twice,
in opposite directions, and the reason is worth keeping: every capture until now
only ever switched the row *on*, and the pedal defaults to CC 4, so key 71 read
as a constant 4 and was written down first as the number's home, then as an on
switch. `mac-cc-capture.log` sets the row to 42, and then 43, 44, 45 and 46,
and sends exactly those under key 71. Taking the row off sends `71: 0`. Key 95
is `5`, the bypass target, throughout. The reply names the same number:

```text
{0: 8, 1: 4, 2: nil, 3: nil, 4: 2, 5: 12, 9: 5, 10: 300, 11: true, 12: 0}
```

Key 0 is 8, MIDI CC, so the source is inferred from the shape rather than sent
at 74, and **key 12 is the CC number** coming back.

**A parameter's CC is set somewhere else entirely: opcode 64. [confirmed]**
`{98: block, 29: true, 26: 0, 28: param, 71: cc}`. The two forms do not share a
mechanism, which is why looking for a parameter's CC inside its assignment
message never found one: an assignment says a controller exists - key 71 is the
constant 4 there, for a footswitch as much as for MIDI - and opcode 64 says
which CC. HX Edit sends one per intermediate value as the field is spun, the way
a knob drag streams, so the last one is the answer.

**And it reads back in key 1, the same place a bypass's does. [confirmed]** The
two forms differ in how the number is *set* and not at all in where it is kept:
put block 5's Feedback under MIDI, send 42, 43 and 77 through opcode 64, and
opcode 36's reply and the document's controller entry both carry exactly those
under key 1.

```text
{0: 8, 1: 77, 2: 0, 3: 1, 4: 0, 5: 5, 6: {28: 0, 29: 1, 41: false}, 7: 0, 13: false}
```

So key 1 is the CC number under MIDI whatever it drives, and the constant 4 - the
CC the pedal picks for itself - is what made it look otherwise for the third time
in this file. Under any other source it really is a constant: an expression pedal
or a footswitch has no CC to give. Decide by the *source*, never by the number.

**A bypass on a footswitch is not in the document. [confirmed]** The controller
table at top-level key 4 holds every parameter assignment and the bypasses an
*expression pedal* drives - a wah's auto-engage - and no footswitch bypass at
all. Checked across all 126 presets on the pedal: not one. It lives in the
footswitch's own configuration, where opcode 33 reads it, so anything showing
what drives a block has to read both and put them together.

**And opcode 33's list is not all bypasses.** Key 67 names everything the switch
drives on a block, a parameter assignment included, and the entry looks the same
either way: key 69 names the *block* in both. Since the document holds every
parameter assignment and no footswitch bypass, an entry the document already
accounts for is that parameter and only an unaccounted one is the bypass. That
fails only where one switch drives both a knob and the bypass of the same block.
Key 109 inside the target is the target's name and reads as the block's name for
a bypass; whether it reads as the *parameter's* name for a parameter would
settle it outright, and no capture shows a switch carrying one. **Unverified.**

**Opcode 37's own reply reports the travel correctly**, where opcode 36 does
not: after moving the ends to 0.1 and 0.9 through 65 and 66, a reassignment
through 37 answers `2: 0.1, 3: 0.9` while 36 keeps saying 0 and 1.

**The ends are in the parameter's own units, not normalised. [confirmed]** The
document holds a pitch block's ends as `7` and `12` - semitones - and a wah's as
0 and 1 because that is a wah's range. Reading them as percentages showed a
pitch assignment sweeping "700% to 1200%".

**Switch numbering differs between these, and now we know how. [confirmed]**
Opcodes 56 and 57 count from **zero**, `102: 0` being Footswitch 1. Opcode 33
takes a **one-based** number and answers with the zero-based one: asking for 1
comes back `{102: 0}`, 2 comes back `{102: 1}`, and asking for 0 is refused with
error -3. So 33 is one-based in and zero-based out, which was the open question
here; it does not read a neighbour.

**Opcode 33 is also how you find out what a switch is set to. [confirmed]** An
unassigned switch answers:

```text
{102: 0, 65: false, 109: nil, 66: nil, 67: nil}
```

and one carrying a block's bypass fills in key 67:

```text
{102: 0, 65: false, 109: nil, 66: nil,
 67: [{59: true, 68: 1, 66: 16711683,
       69: {109: "Jazz Rivet 120", 98: 2, 29: false, 26: 0, 28: 0,
            120: {56: 0, 51: 1}}}]}
```

Key **67 is the assignment list**, one entry per thing the switch controls.
Inside an entry, **69 is the target** (`109` its name, `98` its block) and **66
is the colour**, `0xRRGGBB`: `16711683` is `0xFF0003`, the red HX Edit gives the
Amp category. That is the block's own colour travelling with the assignment,
which is what an Auto Color switch lights up as. Key 65 on the outer map is
momentary, 109 the custom label, 66 the chosen LED colour, and both are nil when
the switch is on its defaults.

Keys 72 and 73 carry the ends of the controller's travel, normalised. Bypass is a switch, so
only a footswitch or a CC can drive it - HX Edit lists expression pedals for a
bypass and then steps over them.

**Method: those dropdowns answer the scroll wheel. [method]** HX Edit's
custom-drawn menus ignore synthetic clicks, and its controls are invisible to
the accessibility API, which had this section stuck at "the menu opens and
nothing can be chosen". A scroll event over the closed dropdown steps its
selection and sends the traffic, which is how the whole source list was mapped
one entry at a time.

### A reply of `103: 1` means "accepted", not "failed" [confirmed]

Some opcodes on the control channel do not finish inside their reply. They
answer `{103: 1}` immediately and report the real outcome a millisecond later as
a notification carrying **the same transaction id under key 102**, with
`103: 0`:

```
--> op6  {102: 1005, 100: 6, 101: {107: 0, 108: 66, 109: 'DIR:Relief1'}}
<-- rsp  {102: 1005, 103: 1, 104: None}          # accepted
<-- ev6  {105: 6,  106: {…, 106: {107: 0, 108: 66, 109: 'DIR:Relief1'}}}
<-- ev1  {105: 1,  106: {102: 1005, 103: 0, 104: None}}   # done
```

Selecting a preset (opcode 20) behaves the same way, and there the completion
arrives as `ev20` rather than `ev1`, ahead of the ev8 / ev39 / ev4 sequence of
the preset loading. **The notification's own id varies with the operation, so
the transaction id is the only reliable link** - match on key 102, not on the
event number.

This matters because `103` is the error field everywhere else. A client that
reads a non-zero `103` as a refusal decides every rename and every preset change
failed, and then does not reload.

### Identifying a flag by patching the reply [method]

A boolean nothing acts on is indistinguishable from any other boolean, so the
way to learn what one means is to change it and watch the client. Opcode 99
returns `{63: bool}` and is `false` in every state reachable over USB, which
left it unexplained for a long time - polling it during edits, reloads, flash
writes and even with the tuner engaged never moved it.

`tools/hxsniff` can rewrite a reply on its way into HX Edit while leaving the
wire to the device untouched:

```sh
HXSNIFF_PATCH=68813fc2/68813fc3 tools/hxsniff/run.sh
```

That pattern is `104 -> {63: false}` becoming `true`, which matches only an
opcode-99 reply. With it applied, HX Edit's tempo readout changes from `120.0`
to **`[External]`** - so opcode 99 asks *is the tempo being driven by an
external MIDI clock*. Sending the device real MIDI beat clock confirms it from
the other side: the flag reads true for exactly as long as the clock runs.
`tools/midiclock.swift` generates the clock and
`crates/hx-usb/tests/device.rs` holds the regression test.

Two lesser findings came out of the same hunt. **The device stops answering the
editor entirely while its tuner is engaged** (CC68 over MIDI) and resumes when
it is dismissed - worth knowing before diagnosing a timeout as a wedge. And
**key 63 means "in effect"** rather than anything preset-related: opcode 76
uses it for whether the global EQ is switched on, alongside its eleven
coefficients under key 55.

**Feeding the device MIDI beat clock kills an open editor session.
[confirmed]** The flag tracks the clock faithfully while it runs, but when the
clock stops the device stops answering over USB and does not recover - twenty
seconds of patient polling never gets a reply, and the 9V adapter has to come
out. Nothing the editor does causes it and nothing it does avoids it, so a
client that wants to be safe simply should not be the thing sending clock. The
regression test for the flag is therefore opt-in
(`TONEPUSH_DESTRUCTIVE=1`).

## Layer 4 - MessagePack RPC [confirmed]

The body is standard **MessagePack** with integer keys. Line 6's strings are C
strings whose declared length *includes* the trailing NUL, so `0xa9` introduces
`"l6-helix\0"` - strip trailing NULs after decoding. Floats are `0xca` float32,
big-endian per the MessagePack spec.

Three message shapes:

```
request       {102: txn, 100: opcode, 101: args}
response      {102: txn, 103: status, 104: result}
notification  {105: event, 106: args}
```

`txn` starts at 1000 per channel and increments. `status` 0 is success.
Notifications are unsolicited device→host and carry no transaction id - they are
how the device reports front-panel activity, cursor moves and preset switches.

Two real exchanges from our capture:

```
--> {102: 1013, 100: 30, 101: {98: 1, 29: True, 26: 0, 28: 0, 119: 0.78}}
<-- {102: 1013, 103: 0,  104: {98: 1, 29: True, 26: 0, 28: 0, 119: 0.78}}
<-- {105: 39, 106: {82: 1, 68: 3, 121: 19, 106: {98: 1, 26: 0}}}
```

### Opcodes (key 100)

| Op | Meaning | Args |
|---|---|---|
| 1 | list presets | `{107: setlist, 101: 2}` → array of `{index: {109: name, …}}` |
| 20 | select preset | `{107: setlist, 108: index}` - answers `103: 1`, see below |
| 22 | read current preset document | nil |
| 23 | current preset metadata | nil - returns `{107, 108, 109: name}` |
| 24 | fetch object by id | `{118: id}` |
| 30 | set parameter | `{98: block, 29: true, 26: path, 28: index, 119: value}` |
| 41 | enable/bypass block | `{98: block, 59: enabled}` |
| 88 | select snapshot | `{92: zero-based index}` |
| 28 | clear block | `{98: block}` - **must be preceded by opcode 78 selecting that block** |
| 37 | assign a controller | `{98: block, 95: target, 96: scope, 74: flags, 71: MIDI CC}` |
| 9 | upload impulse response | `{112: slot, 113: checksum, 109: name, 114, 115, …}` then raw samples |
| 4 | **read a preset at an index, without loading it** | `{107: setlist, 108: index, 101: 2}` → the whole document |
| 5 | write a preset document into an index | `{107, 108, 123, 124, 125, 110: document}` |
| 8 | write a preset *with a name* - paste, and file import | `{107, 108, 109: name, 123, 124, 125, 110: document}` |
| 16 | empty a preset slot | `{107: setlist, 108: index}` |
| 86 | write the whole globals block | `{110: <one msgpack blob>}` |
| 111 | object-store transfer *in* - opcode 109's inverse | `{64: id, 106: continuing, 105: ack}` |
| 45 | read a block, to save it as a favorite | `{98: block}` → its model-ref and values |
| 112 | list favorites | `{}` |
| 113 | read a favorite | copy and export send it |
| 114 | write a favorite | paste and import send it |
| 116 | clear a favorite | - |
| 117 | rename a favorite | - |
| 12 | read an IR's descriptor | `{112: slot}` → the same map opcode 9 sends |
| 11 | read an IR's samples | `{112: slot, 101: 2}` → a blob of 32-bit floats |
| 10 | rename an impulse response | `{112: slot, 109: name}` |
| 76 | read the global EQ | `{}` → `{63: enabled, 55: [11 coefficients]}` |
| 77 | reset the global EQ | `{}` → the same shape as opcode 76 |
| 6 | rename preset | `{107: setlist, 108: index, 109: name}` |
| 68 | set MIDI CC / channel | - |
| 25 | set footswitch function | - |
| 78 | highlight slot | - |

Opcodes 59 and 61 are no longer inferred: `capture.sh assign` caught them, and
they are in the assignment table above with their real arguments. Opcode 112 is
now known too - it lists favorites, and the session-setup call is
just the editor populating that tab. Opcodes 0, 23, 76, 99 and 254 are still
only observed during session setup, and need hardware that exposes them: the Command Center opcodes are
inert on an HX Stomp, so a Helix Floor or LT is the prerequisite. Opcode **68 alone** is still `kempline/helix_usb`'s rather than ours
**[inferred]**, and it looks doubtful: a whole session of setting MIDI In on the
assign page never sent it (see below). Every other opcode that entry listed has
since been caught on this wire - 6, 25, 59, 61 and 78. Its "opcode 25, set footswitch function" is our own opcode 25 with
`{118: 97|98|99}`: a device setting like any other, not a separate operation.

Common argument keys: `107` setlist, `108` preset index, `109` name, `118` object
id, `119` value, `98` block index, `92` snapshot index.

**What HX Edit actually offers depends on the device. [confirmed]** Command
Center is inert on an HX Stomp - the menu item exists and clicking it does
nothing, because assigning banks of footswitches is a Helix Floor/LT feature. The
tuner is not in HX Edit at all; it lives on the hardware. Scoping "parity with HX
Edit" against a small device therefore covers less than the menu bar suggests.

### Impulse response upload [partly decoded]

Importing an IR sends opcode 9 on the data channel, followed by the audio as raw
bytes across subsequent messages - about 8 KB for a 1024-sample mono file:

```
{102: txn, 100: 9, 101: {112: slot, 113: 0xf7656589, 109: "test-impulse",
                         114: 1, 115: 3, 123: false, 124: false, …}}
```

**Key 113 is the samples summed as little-endian 32-bit words. [confirmed]**
Established by importing two IRs of different length through HX Edit and
testing candidates against both: CRC-32, Adler-32, byte sum and length all
fail; the wrapping word sum reproduces both exactly. Two earlier readings were
wrong - first a CRC-32 guess, then a conclusion that it was an identifier
rather than a digest, drawn from two files similar enough that their sums
shared a high half.

**HX Edit's full control-channel sequence around an upload. [confirmed]**

```
op 254 {}                      -> status 0
op 0   nil                     -> [{0: 'PRESETS'}]        setlists
op 1   {107: 0, 101: 2}        -> 126 preset entries
op 112 nil                     -> status 0
op 13  {101: 2}                -> [{112: slot, 109: name, ...}]   the IR list
op 255 {}                      -> status 0
op 9   {112, 113, 109, 114, 115, 123, 124, 125, 110}  -> status 1 (accepted)
op 254 {}
op 13  {101: 2}                -> the IR list again
```

Note op 9 answers **status 1**, so its outcome arrives later as a notification
rather than in the reply.

**Uploads work once the control channel does. [confirmed]** The long-running
failure was never IR-specific: op 9 rides on the control channel, and that
channel was silently misconfigured. With the channel fixed, a 256-sample IR
uploads, appears in its slot, and clears again.

**A one-frame upload fails too. [confirmed]** A 32-sample IR fits in a single
frame, so chunking and pacing are not the cause: it times out identically. The
fault is in the message itself - a missing or wrong field - not the transport.

Key 112 is the destination slot, 109 the display name, and **110 the samples as
little-endian `f32`** - the whole IR in one MessagePack blob, roughly 8 KB for a
1024-sample file. Key 113 is a wrapping sum of the sample bytes taken as
little-endian u32 words (not a CRC - see the checksum note below).

**Keys 114 and 115 declare the stored length. [confirmed]** The device stores
`114 × 256 × 2^115` samples. Isolated by uploading the same data under varied
values and comparing the stored content hash:

| 114 | 115 | data sent | stored |
|---|---|---|---|
| 1 | 3 | 1024 | 2048, zero-padded |
| 1 | 2 | 1024 | 1024, byte-identical |
| 1 | 1 | 512  | 512, byte-identical |
| 2 | 2 | 1024 | 2048, zero-padded - same image as 1/3 |
| 0 | –​ | any  | hangs the session |

So 115 is a length exponent and 114 a multiplier - plausibly a channel count,
though HX Edit always sends 1 and only the product is observable. Data shorter
than the declared length is zero-padded; data **longer** than declared wedges
the device's transfer state machine badly enough to need the 9V adapter pulled,
which is why this client derives the code from the sample count and refuses
anything over 2048 samples.

**The IR list reveals the stored bytes. [confirmed]** Each op 13 entry carries
key 104: the MD5, as lowercase hex, of the stored sample bytes *after* padding.
Uploading 1024 known samples under code 3 and hashing them locally with 4 KB of
zeros appended reproduces the device's value exactly. That makes end-to-end
verification of an upload free, and it is how the table above was measured.

Keys 123, 124 and 125 are not IR-specific - preset list entries carry the same
trio (`false`, `false`, `0` everywhere so far); their meaning is untested but
they echo back verbatim.

Opcode 15 `{112: slot}` empties a slot.

### Writing a preset document back [confirmed]

Opcode 21 takes a whole preset document. It is unforgiving: the preset carries a
table of byte offsets into itself, so a document that differs from the original
by even one byte of length leaves those offsets pointing at the wrong places, and
the device accepts it and then reads the preset as empty.

The cause here was that our MessagePack encoder normalised widths: a
value the device wrote as `0xcc 05` comes back out as `0x05`. Nothing in the
protocol objects, but the preset carries a **section offset table** (the second
of its three top-level values) holding byte offsets into the document - twelve
little-endian u32s: the offset of the tone map, the offsets of top-level keys
0, 1, 3, 4, 2, 5, 6, 7 and 10 in that fixed slot order (each pointing at the
key byte), then the total length twice. Decoded by matching candidate offsets
against the byte positions of the tone's sections in two captured presets, and
pinned by a test that recomputes the fixture's table byte for byte
(`Preset::computed_sections`). Change
any field's width and every offset after it is wrong, which is exactly the shape
of "device accepts it, preset reads back empty".

**Fixed by making the round trip byte-exact.** Three kinds of value had to keep
the tag width they arrived with - unsigned integers, signed integers and blobs -
because MessagePack lets the same value be written several ways and our encoder
chose the narrowest. In one captured preset, 91 of 103 wide integer tags would
have shrunk.

The diagnosis came from a test rather than from reasoning: a captured preset as
a fixture, re-encoded and diffed byte for byte, reporting where it first
diverges. It located each cause in turn - byte 10 (a blob tag), then byte 789
(an int16 zero) - where inspection had produced only plausible theories. That
test is `crates/hx-proto/tests/roundtrip.rs` and it needs no hardware.

**Uploads are verified end to end.** An earlier version of this section warned
that our upload hung the device; the cause was the control-channel
misconfiguration described above, not the message. The remaining hazard is the
declared-length rule: see keys 114/115 below.

**A message this large must be chunked, and paced. [confirmed]** The device
accepts 256 bytes of stream data per frame and paces the sender with
acknowledgements. Writing all 33 frames back to back fills its receive window and
stalls the endpoint: the transfer times out, and afterwards the interface will
not re-claim until the device is power-cycled. Read between chunks, as HX Edit
does.

**File dialogs are sheets, not windows. [method]** HX Edit's Import opens a
sheet attached to its main window, so `windows` shows one and the dialog looks
absent. `count sheets of window 1` finds it, and ⌘⇧G plus a path drives it.
Getting this wrong cost two rounds of concluding the feature was unreachable.

**Driving HX Edit's menu bar is the reliable way to capture features. [method]**
Its custom-drawn dropdowns ignore synthetic clicks entirely, which made several
operations look as though they generated no traffic. The menu bar is standard
AppleScript-addressable, and `File`, `Edit`, `Snapshots` and `Window` between
them expose preset import/export, block cut/copy/paste/clear, snapshot
operations, Global EQ and Command Center. Copying a block is purely local; only
the operations that change the device speak.

**Opcode 40 carries a model descriptor, not a bare model number. [confirmed]**
`{98: block, 100: {23: paired, 25: model, 26: second model or -1}}`. Sending
`{98, 25}` instead is answered with success and changes nothing - note that key
100 here means "model descriptor" while at the top level of a message it means
"opcode". Must still be preceded by a select.

**Clearing a block requires selecting it first. [confirmed]** Opcode 28 on its
own is answered with success and changes nothing - the quietest possible
failure. HX Edit always sends opcode 78 for the same block immediately before,
and with that the block disappears. Suspect the same pattern for any other
operation that reports success without effect.

**Snapshots switch with opcode 88. [confirmed]** An earlier capture concluded no
opcode existed, because clicking the snapshot menu produced no traffic - the
click was landing on the already-active snapshot, and HX Edit sends nothing for
a no-op. Driving it from the keyboard instead (⌘1/⌘2/⌘3) produced three clean
requests carrying `{92: 1}`, `{92: 2}`, `{92: 0}`, matching the shortcuts
exactly. Worth remembering as a method: when a UI action seems to produce no
traffic, check that it actually changed something.

**Key 108 is a linear zero-based preset index, not a bank number. [confirmed]**
`{107: 0, 108: 7}` is the preset the front panel labels `03B`, so the label is
`index / 3 + 1` followed by `A`/`B`/`C`. Selecting index 7 and reading the
metadata back returns `CT-Sad`, which is what HX Edit shows at 03B.

**Reply statuses (key 103). [confirmed]** Three values cover everything
observed:

| status | meaning |
|---|---|
| 0 | done |
| 1 | accepted; the operation completes later (select preset, write preset, IR upload) |
| 255 | refused; the result is `{111: signed error code}` |

HX Edit's own traffic contains only 0 and 1, which is why the refusals stayed
unmapped until deliberately bad requests were sent. Codes observed so far: `-3`
a bad block or parameter reference, `-46` an out-of-range snapshot, `-302` an
unknown model number. Two sharp edges: **accepted is not validated** - selecting
preset 999 on a 126-preset device answers 1 and simply does nothing - and a
no-op is not an error: clearing an already-empty IR slot answers 0.

The preset *name* is not part of the preset document - it comes from opcode 23.

A global-EQ read decodes as `{63: True, 55: [110.0, 0.707, 0.0, 2000.0, 0.707,
0.0, 8000.0, 0.707, 0.0, 19.9, 20100.0]}` - three bands of frequency/Q/gain plus
low-cut and high-cut, matching the device's Global EQ page.

## Layer 5 - the preset document [confirmed]

A preset arrives as an opcode-22/24 result: a MessagePack string/blob whose
contents are *themselves* MessagePack - three top-level values:

1. `'l6-helix'` - magic
2. a binary section table of u32 LE offsets
3. the preset map

The preset map decodes as:

```python
{7: {36: 'P33',                     # DSP / service name
     35: 0x03800000,                # firmware, BCD -> 3.80
     37: 'v3.71-32-g1039661'},      # build string
 0: {21: 0,
     22: [ {19: <slot type>, 20: {<parameters>}}, ... ]},
 1: {21: 0,
     22: [ {19: <slot type>, 20: {<parameters>}}, ... ]}}
```

Keys `0` and `1` are the two DSPs. A Helix LT capture has 20 slots under each,
while an HX Stomp preset has 20 under key `0` and leaves key `1` nil. Snapshot
state arrays are not DSP-local: they flatten the two arrays in DSP order, so the
LT snapshot has 40 entries. Code that validates a snapshot against only
`tone[0][22]` therefore rejects a valid full-size Helix preset.

Block parameter maps use `{2: n, 3: n, 4: [values]}` groups, and floats carry the
actual parameter values. Snapshot names (`SNAPSHOT 1`…) and the preset name appear
as plain strings.

### Model and parameter numbering [confirmed]

**`Helix.sym`'s array index is the device's model number.** The file ships with
HX Edit as a plain JSON array of 833 entries; entry *n* is model *n*. Index 247
is `HD2_ReverbRoomStereo` ("Room"), 296 is Simple Pitch, 180 is Bubble Vibrato -
each matching the model names that appear as text in captured presets. 829 of the
833 join to the `.models` catalog once mono/stereo suffixes are folded together.

Each entry also lists its parameters **in the order the device indexes them**,
which is what makes parameter addressing legible. A slot holding model 101
(Scream 808 - Gain, Tone, Level) carries exactly three values.

This was verified end to end: reading a preset and rendering it through the
catalog reproduces HX Edit's own parameter panel value for value, in order,
for a Cali Rectifire showing Drive 9.2, Bass 3.2, Mid 6.4, Treble 7.2,
Presence 6.2, Ch Vol 8.9, Master 1.8, Sag 2.0, Hum 2.7, Ripple 5.0, Bias 6.0,
Bias X 6.0.

Values on the wire are in the parameter's **native** units as the catalog defines
them, not normalised: a Mix shown as "100%" is `1.0`, a knob shown 0..10 is
stored 0..1. Switches are MessagePack booleans.

### Slot structure [confirmed]

The tone holds one fixed array of slots per DSP at `tone[0][22]` and
`tone[1][22]`, each slot `{19: kind, 20: body}`. Junction attachment indexes are
local to their DSP array; snapshot slot indexes are flattened across both arrays.

| Kind | Meaning | Model number at | Values at |
|---|---|---|---|
| 0 | input | - | `7` |
| 1 | output | - | `7` |
| 2 | split | `15.8` | `15.7` |
| 3 | join | `17.8` | `17.7` |
| 6 | effect, amp or cab | `24.25` | `11` |
| 8 | empty slot | - | - |

Values arrive as `{2: count, 3: count, 4: [...]}`, with switch parameters
appearing as booleans inline among the floats.

Two fields sit beside the values rather than among them, which is why they never
appeared as parameters:

| Key | On | Meaning |
|---|---|---|
| `20.5` | input | `Input From` - indexes the `input_type` menu in `HelixControls.json` |
| `20.6` | output | `Output To` - indexes `output_type` |
| `20.9` | effect slots | the model's engine class - see below **[confirmed]** |

`Input From` and `Output To` are the first control HX Edit shows on Input and
Main L/R. The device **does not apply a change to them from a preset-document
write** - it accepts the document, keeps everything else, and leaves the
routing as it was. They are changed with their own opcode, captured from HX
Edit's routing clicks:

**Opcode 42 `{98: slot, 51: destination}` routes an endpoint. [confirmed]**
Answered synchronously with status 0, echoed as notification 27 carrying the
same arguments, and reflected in the document's key `20.5`/`20.6` on the next
read. One caveat: the destination values are per device model. On an HX Stomp,
HX Edit's three input choices send 1 (Main L/R), 4 (Return L/R) - values that
do not line up with the generic `input_type` menu in `HelixControls.json`
(where 4 is Variax), so the names shown for routing values on a Stomp are
approximate until its own enumeration is mapped. **[partly open]**

Key `20.9` was pinned down by setting one model per category into the same slot
and reading it back:

| models | `20.9` |
|---|---|
| distortion, dynamics, EQ, modulation, pitch, wah, volume/pan, preamp, cab | 1 |
| delay, reverb | 8 |
| amp | 18 |
| IR block, 1024-tap | 19 |
| IR block, 2048-tap | 20 |
| send/return, looper | 25 |

The reading that fits is an **engine or resource class**: simple effects share
one code, effects needing delay RAM share another, amps their own, the two IR
lengths take adjacent codes for double the memory, and the two block types that
touch hardware I/O share the last. It was first misread as a branch index; the
branch is implied by array position, see below.

**It is not a function of the model alone. [confirmed]** The table above is what
one preset shows; across 240 captured presets holding 217 distinct models, 30 of
those models carry *two* different values. An amp carries **17 on its own and 18
with a cab riding along**, and a cab carries **15 alone and 16 as a dual** - the
tag describes what is in the slot, and an amp slot with a cab in it is not the
same shape as an amp slot without one. Keyed by the model's category *and*
whether it is paired, nothing is ambiguous:

| category | alone | with a cab |
|---|---|---|
| amp, preamp | 17 | 18 |
| cab | 15 | 16 |
| delay, reverb | 8 | - |
| distortion, dynamics, EQ, filter, modulation, pitch, wah, volume/pan | 1 | - |
| FX loop | 9 | - |
| looper | 22 | - |
| split, merge | 0 | - |

Two models sit outside their category: the **3 Note Generator** carries 23 rather
than 1, which fits - it has no input to process - and **Send** carries 25 where
the FX Loop it shares a category with carries 9.

Endpoints, splits and joins carry no key 9 at all. The device maintains the
value itself on model changes, so *editing* a preset only ever needs to carry it
through byte-exact. Deriving it matters for the other direction: **building** a
document from a symbolic tone, where there is nothing to carry through, and this
is the one field of a slot that a `.hlx` does not record.
`Catalog::type_tag` derives it and `hx-catalog/tests/type_tags.rs` pins the
derivation against every captured fixture.

### The second count in a value array [confirmed]

A slot's values arrive as `{2: count, 3: count, 4: [values…]}`, and the two
counts are not always the same. Key 2 is the number of values, always. Key 3 is
the same number for most models and **one less** for cabs, delays, reverbs and
the FX Loop.

Keyed by category the difference is 0 or 1 with no category showing both, and
the one category that did - Send/Return - splits by model exactly as the engine
class does: the FX Loop takes 1 and Send 0. So these models carry a parameter the
array holds and this count does not admit to. **Which** parameter is open; that
it costs exactly one is not, which is enough to write the field correctly.

Rebuilding every fixture's slots from their parsed form and re-encoding is what
surfaced this: with key 3 set naively to the value count, the presets holding
only simple effects came back byte-exact and every one holding a cab, a delay or
a reverb differed. `Catalog::value_count_2` derives it, pinned by the same test
as the engine class.

Together those two fields are everything a slot carries that a `.hlx` does not,
which is what the JSON-to-document direction has been waiting on.

### The slot array is a topology, not a running order [confirmed]

The array is laid out per signal path as:

```
input, blocks…, output, split, blocks…, join
```

The split appears *after* the output even though the signal reaches it first.
Read as a running order this puts the split and join on the end of the chain,
which is where they get drawn if you do not know better. HX Edit and Logic's
Pedalboard both draw a split as the wiring dividing into a second lane, not as a
box in the line.

Devices with more than one signal path repeat the whole pattern, so a Helix or
Helix LT preset with both paths split has four lanes. `tonepush topology` prints the
derived structure:

```
 A   0 Input [Multi (Guitar, Aux, Variax)]  ->   1 Cali Q Graphic  ->  …  ->   9 Output [Multi (1/4", XLR, …)]
 B  10 Split Y  ->  13 Line 6 2204 Mod  ->  19 Mixer
```

### A `.hlx` numbers cells along a row, not slots [confirmed]

HX Edit records where a block sits as two fields: `@path`, the branch, and
`@position`, its place along that branch's drawn row. Neither is the device's
slot index, and the two only agree on a chain with no split.

Settled by exporting one preset twice - the factory `DIR:Relief`, once by us
from the device's own bytes and once by HX Edit:

| Block | Device slot | `@path` | `@position` |
|---|---|---|---|
| US Deluxe Nrm | 1 | 0 | 0 |
| Plate | 6 | 0 | 5 |
| Mod/Chorus Echo | 12 | 1 | 1 |
| Octo | 13 | 1 | 2 |
| Particle Verb | 14 | 1 | 3 |
| Parametric | 15 | 1 | 4 |
| split (attaches before slot 2) | - | - | 1 |
| join (attaches before slot 7) | - | - | 6 |

So **branch 0's row is the whole main line**, from just after the input to just
before the output: `slot = position + input + 1`. The split and the join sit
*inside* that run rather than dividing it, which is why the blocks after a join
keep counting on from the ones before it, and why a junction's own number -
meaning "before this cell" - is the same arithmetic. **Branch 1's row is the
lower lane**, which begins in the slot after the split: `slot = position + split
+ 1`. `Layout::slot_of` and `Layout::position_of` are those two sums.

A `dspN` is a whole signal *path*, which only hardware with two DSPs has; a
branch of one path is `@path` inside the same dsp. Reading a branch as a second
dsp is what used to drop every block on it.

Two more of HX Edit's own conventions, from the same file. An amp names its cab
rather than the cab naming its amp: `"@cab": "cab0"`. And `@model` is the
*shared* model id - `HD2_ReverbPlate` - where the firmware has a mono symbol and
a stereo one, so an exact match on the symbol finds neither.

## Reference data shipped with HX Edit

HX Edit contains its full model catalog in the clear, which a third-party editor
needs for parameter ranges and names:

- `HelixModelDefs.bin` - MessagePack, 681 model definitions with `name`,
  `symbolicID`, `category`, and `params[]` carrying `min`, `max`, `default`,
  `valueType`, `displayType` and `assign` (the numeric parameter id).
- `HX_ModelCatalog.json` - 23 categories matching the editor UI.
- `Helix.sym`, `*.models`, `default_preset.hlx`.

**Licensing:** these are Line 6 proprietary data files. They must not be
redistributed. A third-party tool should read them from the user's own installed
HX Edit at runtime, and degrade gracefully when it is absent.

The `.hlx` preset file format is plain JSON and is independently documented; `.hxb`
bundles are a zlib-sectioned container (signature `AF6L`).

## Related work

- [`kempline/helix_usb`](https://github.com/kempline/helix_usb) - Python; the
  deepest prior effort. Found the three-channel structure and a module catalog.
- [`allansomensi/openhx`](https://github.com/allansomensi/openhx) - Rust;
  implements list and select preset, models a single channel.
- [`AntonyCorbett/HelixBackupFiles`](https://github.com/AntonyCorbett/HelixBackupFiles),
  [`frankdeath/hx-tools`](https://github.com/frankdeath/hx-tools) - file formats.

The Linux kernel's `snd-usb-line6` driver does **not** support HX devices; its
highest PID is `0x415A` and its SysEx protocol is the older POD/Variax one.

Helix Stadium XL uses a completely different transport (Bonjour + TCP + ZMTP),
so none of this applies to it.

## Tools

- `tools/hxsniff` - libusb interposer, capture driver, decoder, reassembler.
- `tools/midiprobe` - CoreMIDI list/listen/send with SysEx reassembly.
- `tools/usbprobe` - libusb interface/endpoint enumeration and claim testing.

Captures live in `captures/`.
