# Backup and restore

Goal: **bulletproof backup and restore** for the HX Stomp - never lose a preset
again - covering presets, global settings, IRs, setlists, and favorites, with
automatic backups so a mistake is a shrug, not a crisis.

## Two formats, on purpose

| | MessagePack bundle (`.hxpreset` per preset) | HX Edit `.hxb` |
|---|---|---|
| Fidelity | **byte-exact** (what the pedal stores) | faithful, needs conversion |
| Portability | same pedal / firmware | **any firmware, any Helix-family unit** |
| Write path | `write_preset` + `save_preset` (hardware-validated) | needs a device→JSON converter (see below) |
| Risk | none - no transform | a wrong transform loses routing/snapshots |

Decision: the **MessagePack bundle is the safety/auto-backup format and our
restore path** - it cannot lose anything. The `.hxb` is a portable **export**,
added once the faithful converter exists. We already **read** `.hxb`
(`hx_catalog::read_backup`) and **write** the container byte-for-byte
(`hx_catalog::Container`, proven against four real backups).

## What a complete backup holds

- **Presets** - 126 slots, byte-exact `.hxpreset`. `backup-all` once did this
  by loading each preset, **~114 s** for a full read that also discarded unsaved
  edits; it now uses the fast read below and never changes the loaded preset.
- **Global settings** - **154** answering device objects (`object(id)`), which line
  up with HX Edit's `GLOB` block (156 named fields: EQ 13, Tuner 11, DSP 5, System 127).
  Store id→value; labels can come later.
- **Setlists** - one on the HX Stomp, named `PRESETS`. A Helix Floor, LT or
  Rack has eight, and a bundle holds every one: the first in `presets/`, the
  rest in `presets-1/`, `presets-2/` and so on, numbered as `tonepush
  setlists` numbers them, with their slot names in the manifest's
  `more_setlists`. A restore puts each back in the setlist it came from.
  Bundles made before this held only the first setlist; restoring one says so
  and leaves the other setlists as they are. An `.hxb` export carries the
  first setlist only, and says so.
- **IRs** - read-back decoded: `op12` for the descriptor, `op11` for the samples
  (see below). Carmine's pedal normally carries **none** (cab models, not IRs),
  so today's backups are complete without them either way.
- **Favorites** - tonepush-local.

## Cadence [built]

The fast read changed this. A full capture is **~4 s** and never moves the
loaded preset, so it runs **on every connect** - one automatic bundle, kept at
`~/.local/share/tonepush/backups/automatic.hxbundle`. Every **save** then
refreshes just the preset it changed, which is milliseconds and silent. On top
of that, **Back up…** and **Restore…** on the preset list do it on demand.

So the copy on disk is never older than the last thing you did, and nothing
ever interrupts playing.

## Where a bundle may go [built]

`tonepush back-up DIR` (and the editor's **Back up…**) writes the bundle as
`DIR` itself, built beside it and swapped in whole. `DIR` may be new, empty, or
an earlier TonePush bundle, which is replaced. Anything else is refused: a
folder of your own files is never moved or deleted to make room for a backup.
The old bundle sits briefly in `DIR.previous` during the swap; a `DIR.previous`
that is not a bundle is left alone, and a backup that would need its name stops
and says so.

## The `.hxb` container (AF6L) - decoded

```
[0:4]   "AF6L"
[4:8]   version (u32, = 1)
[8:16]  offset of the block table (u64)
[16:24] block count (u64)
[24:..] blocks, packed back to back (first is IDXH, at offset 24)
[table] 36-byte entries: tag(4) offset(u64) stored_len(u64) flags(u32) raw_len(u64) reserved(u32)
```

Blocks: `IDXH` (device id + Unix timestamp - the field that looked like a
checksum), `GLOB` (globals JSON), `SDMU`, `MNLS` (the name `PRESETS`), `SL00`
(setlist: `data.presets[]`, all 126 as `{meta,device,tone,device_version}`).
Compressed blocks are one zlib stream (flags==1); integrity is zlib's adler32,
so there is **no container checksum**. Implemented in `hx-catalog/src/hxb.rs`.

## MessagePack tone → HX Edit JSON [built, checked against 94 real presets]

The device document (`Preset.tone`, numeric keys) and HX Edit's `.hlx`/`.hxb`
JSON (symbolic) are two encodings of the same tone. `to_hlx` today emits blocks +
params + models correctly but **drops routing, snapshots, and the sections** - it
is lossy. A faithful converter must map, both directions:

- **Paths** `tone[0]`, `tone[1]` → `dsp0`, `dsp1`. Each path is `{21: routing, 22: [slots]}`.
- **Slots** (positional array) → **named nodes**. Slot `kind`: 0 input, 6 block,
  and 1/2/3 the routing nodes, 8 empty. JSON names them `inputA/inputB`,
  `block0..N`, `split`, `join`, `outputA/outputB`, `cab0`. Block bodies:
  `24` model-ref (`25` model number → symbol), `10` enabled, `11` values,
  `12` paired (cab) values, `9` routing/attach. Even factory presets use a Y-split.
- **Sections** → named: `tone[2..7,10]` carry the three snapshots (per-block
  bypass, controller values, name/color/tempo/valid), `global`
  (`@tempo/@topology*/@guitarinputZ/...`), `variax`, `controller`, `footswitch`.
  Each needs byte-exact identification against the golden pairs.

Validation is fully offline: the presets exist as both `.hxpreset` (from a
backup) and HX Edit JSON (from the `.hxb`), matched by name - golden pairs for
both directions. **All 94 that pair up now agree exactly** on node structure,
model names and all three snapshots' bypass states.

What it took, beyond the blocks that were already written:

- **The wiring.** `inputA`/`inputB`, `outputA`/`outputB`, `split` and `join`.
  A is the main output and B the send, on all 94.
- **The split's kind**, off the junction body's model number: 256 is A/B, 257 a
  Y, 258 a crossover. Calling them all Y described a chain that forks wrongly.
- **Snapshots**, with what each switches on. A junction joins that list only
  when its body says a snapshot may switch it (key 18) - 31 of the 97 do.
- **The cab as `cab0`**, not another block number, which is what HX Edit calls
  it. It carries no position because it belongs to the amp before it, so the
  reader puts the Nth cab after the Nth amp.
- **The looper**, slot kind 7, which was being dropped: it carries its model
  inline like a junction rather than under a model reference.
- **Shared model names** - `HD2_DistScream808`, not the `…Mono` firmware symbol.

Still not written: `variax`, `controller`, `footswitch`, and most of `global`
(only the tempo goes out). Those are device state rather than tone, and nothing
yet reads them back, so writing them would be guesswork rather than fidelity.

## Writing an `.hxb` [built]

`tonepush export-hxb <bundle> <out.hxb>` turns a TonePush backup into an
HX Edit bundle. It needs only the direction above, because a `.hxb` stores
presets as that same symbolic JSON. Checked by generating one from a real pedal
backup, reading it back, and comparing every preset against HX Edit's own bundle:
**97 of 97 agree** on nodes, models and snapshots.

One block is deliberately absent. A real backup carries `SDMU`, an archive of
980 model descriptors that is HX Edit's catalog cache rather than anything about
the pedal's presets; inventing one would be inventing data.

**HX Edit accepts the bundle without it.** Tested against HX Edit 3.82: its
Extract Files From Backup reads the file, reports the right device and firmware,
and writes out all 126 presets as `.hlx`, snapshots included. Those files read
back correctly here too, so the round trip is whole - our bundle, out through HX
Edit, and back in.

One thing to know if you try it: HX Edit lists a backup by its **file name**, not
its contents, so a bundle it will happily read is invisible in that dialog until
it is called `HX Stomp Backup <YYYY-Mon-DD>.hxb`.

## The editor's other files [built]

- **`.hls`** (export setlist): a JSON wrapper around base64 of a zlib stream,
  whose payload is the same `{meta, presets}` the `.hxb` holds in `SL00`. The
  wrapper states its decompressed size and a CRC32; both are checked.
- **`.fav`** (export favourite): plain JSON holding one block, an amp bringing
  its cab as a second slot.

Both read via `read_setlist_file` / `read_favourite_file`, verified against
files HX Edit wrote.

## The fast read: opcode 109 (found in the Mac capture)

A `tools/hxsniff/capture.sh backup` capture of HX Edit backing up shows it does
**not** load each preset. After a short handshake (`END`, `LIST_SETLISTS`,
`LIST_PRESETS` args=2 → 126 names, `READY`, `LIST_IRS`, `BEGIN`) it drives the
whole backup with **opcode 109**, a chunked *transfer* read, and the device
streams **342 KB** back on the reply channel - the pedal shows "transferring
data" rather than cycling presets. This is the fast read we could not find by
probing (`READ_PRESET`+index returns the *loaded* preset; `LIST_PRESETS`
selectors 0–6 return only names+flags).

Opcode 109 request args (RPC envelope: `100`=opcode, `101`=args, `102`=txn):

```
{ 64: <id>, 106: false }              # start transferring object <id>
{ 64: <id>, 106: true, 105: 48 }      # continue; 105 = flow-control offset/ack
{ 64: <id>, 106: true, 105: 687 }     # ...
```

In the backup capture, opcode 109 transfers **803 distinct objects** (`64` ids
0..832, with gaps), so it is not a preset reader - it is a general
**object-store transfer**. Its write inverse is **opcode 111**, same argument
shape, and a restore-everything sends exactly the same 803 ids back.

Still to pin down: the reply framing on `1001→03ef` (where each object's bytes
land) and how a transfer terminates. Captures:
`captures/mac-backup-capture.log`, `captures/mac-partition-capture.log`.

## The fast read was opcode 4 all along [confirmed]

Opcode **4** was written off above as "a per-preset prepare/lock" because the
backup sends it 126 times. It is not. It reads a preset:

```
op4 {107: setlist, 108: index, 101: 2}   → the whole preset document
op5 {107, 108, 123, 124, 125, 110: doc}  → write one back
op8 {107, 108, 109: name, 110: doc}      → write one with a name (paste, import)
op16 {107: setlist, 108: index}          → empty a slot
```

This is the fast per-preset read the design went looking for and could not find
by probing - `READ_PRESET` plus an index returns the *loaded* preset, so the
conclusion was that indexed reads did not exist. They do, under an opcode that
was sitting in the very first capture being read as something else. It answers
in about 50 ms with no preset load, against the ~114 s a `backup-all` took
when it loaded all 126 in turn.

`capture.sh library` is what settled it: exporting a single preset to a file
sends exactly one op4, and exporting the setlist sends 126 - no object store
involved. Importing a setlist is op5 per slot, op16 for the empty ones.

So there are two fast paths, not one, and the simpler one is enough:

| | Whole object store | Per preset |
|---|---|---|
| Read | op109, 803 objects | **op4 by index** |
| Write | op111, same 803 | **op5**, or op8 to name it |
| Covers | everything, undifferentiated | presets only |
| Framing | chunked, still undecoded | one message each, already decodable |

## What a restore actually sends [confirmed]

Not one mechanism but four, chosen by what is ticked in the dialog - which is
why ticking one kind at a time partitions the work by **opcode** rather than by
object id:

- **Global settings** → a single **op86** carrying one 619-byte msgpack blob.
  The whole globals block in one write.
- **Presets/setlists** → **op5** per slot, **op16** for the ones that should be
  empty. 126 messages, paced by the device's flash writes.
- **IRs** → op9, the same upload the editor uses for an import.
- **Everything ticked** → all of the above *plus* **op111** for the full 803-id
  object store.

Capture: `captures/mac-partition-capture.log` (and the earlier
`captures/mac-restore-capture.log`, which shows the same op5/op86/op111 mix and
no op109 at all).

## Capturing the restore

The backup capture gives the fast **read**; the matching **write** is captured
too - HX Edit doing **File → Restore from Backup** of the `.hxb` it had just
made, so the same data went back and nothing was lost. It is
`captures/mac-restore-capture.log` (`capture.sh restore`), and decoding it is
what turns op 109's write inverse into the restore path we want instead of 126
individual `save_preset`s, pacing included.

## Capturing IRs and the settings HX Edit can write

Two more scenarios, both step-at-a-time with a `### MARK` per click so
`attribute.py CAPTURE --marks` says which messages each one produced:

```sh
bash tools/hxsniff/capture.sh ir         # import, export, rename, copy, clear
bash tools/hxsniff/capture.sh globals    # every device setting HX Edit exposes
```

Three more finish what those left half-known, each one sitting:

```sh
bash tools/hxsniff/capture.sh enums      # ids 198 and 200, and the enums of 14, 97-99
bash tools/hxsniff/capture.sh partition  # which op-109 objects are presets/globals/IRs
bash tools/hxsniff/capture.sh library    # favorites and the setlist buttons
bash tools/hxsniff/capture.sh assign     # the switch settings on the assign page
```

### What the IR capture settled [confirmed]

Reading an IR back is **not** part of the op-109 object store. It is its own
pair of opcodes on the control channel, and each export sends both:

```
op12 {112: slot}          → {112: slot, 113: checksum, 109: name,
                             114: 1, 115: 3, 123: false, 124: false, 125: 0}
op11 {112: slot, 101: 2}  → <blob of samples>
```

`op12` returns the **descriptor** - the same argument map `upload_ir` sends
under op 9, checksum included - and `op11` returns the **samples**. Op 10
renames a slot. A copy between slots is op12+op11 followed by op 9: the samples
travel to the host and back rather than moving inside the device.

**Both implemented and verified on hardware** (`read_ir`, `rename_ir`): a ramp
whose every sample is its own index goes up and comes back bit-identical. One
correction to the note below: what comes back is as many samples as the *upload
declared*, not always 2048 - our uploader declares the 1024 size code for
anything that fits it, and the device then stores and returns 1024.

The probe IRs say what the device does to what it is given. Everything comes
back **48 kHz mono 32-bit float, always 2048 samples**, whatever went in:
`ramp1024` returns as its exact `i/4096` ramp zero-padded to 2048, `steps2048`
with its powers of two intact, and `long96k` - 4096 samples at 96 kHz -
resampled rather than decimated (its values fall between the source's, so a
filter ran). Key 115 is 3 on every read, so the stored length is always the
2048-sample code.

**Stereo IR Import is inverted, or its labels are. [confirmed, one import per
setting]** The probe's left channel is `+ramp` and its right is `-ramp`, so the
sign of what comes back names the channel that was kept. Importing under **Use
Left Channel** returned the *negative* ramp - the right channel - and under
**Use Right Channel** the positive one. Mix Both Channels returned silence,
which is exactly `(L + -L)/2` and confirms the mechanism works per import.

**IRs, both directions.** IR *read-back* is the last unknown in the inventory
below, and it cannot be captured on a pedal with no IRs on it - so the scenario
imports first and exports after. The probe files come from
`tools/hxsniff/make-irs.py` and exist to be recognisable in a stream we cannot
yet frame: `ramp1024` is `s[i] = i/4096`, exact in both 24-bit PCM and f32, so
any four bytes on the wire give their own sample index; `steps2048` is a
staircase of powers of two, which makes chunk boundaries visible; `stereo512`
has the channels opposite in sign, so the bytes say what the Stereo IR Import
preference actually did; `long96k` is over both the 2048-sample limit and the
device's rate, so the capture shows what HX Edit converts before it uploads.
The scenario also takes a backup **with IRs present**, which is what places IRs
in the op-109 object store and in the `.hxb` blocks, and it clears the slots
again at the end. Export the files back out and keep them next to the capture -
the wire bytes are only decodable against the files they came back as.

### What the settings capture settled [confirmed]

`op25 {118: id, 119: value}`, one marked change at a time, names them outright.
HX Edit streams a write per intermediate value while a control is dragged, so a
distinct target value per control is what keeps them apart:

| id | Setting | Values |
|---|---|---|
| 14 | tempo source | `0` Per Snapshot, `1` Per Preset, `2` Global; Host Sync writes nothing standalone, so `3` is presumed |
| 16 | tempo, BPM | float |
| 27 | preset numbering format | `false` = 01A-42C, `true` = 000-125 |
| 95 | EXP/FS Tip | `false` = EXP 1, `true` = FS4 |
| 96 | EXP/FS Ring | `false` = EXP 2, `true` = FS5 |
| 97 | FS3 function | one shared enum, below |
| 98 | FS4 function | " |
| 99 | FS5 function | " |
| 190–192 | Global EQ low peak | freq, Q, gain |
| 193–195 | Global EQ mid peak | freq, Q, gain |
| 196–198 | Global EQ high peak | freq, Q, gain |
| 199 | Global EQ low cut | freq |
| 200 | Global EQ high cut | freq |
| 203 | Global EQ enabled | bool |

The footswitch function enum is one numbering shared by all three ids, and FS3
simply offers fewer of it - no banking, no FS Mode:

| | | | | | |
|---|---|---|---|---|---|
| `0` Tap/Tuner | `1` Stomp N | `2` Bank Up | `3` Bank Down | `4` Preset Up | `5` Preset Down |
| `6` Snapshot Up | `7` Snapshot Down | `8` FS Mode > | `9` < FS Mode | `10` All Bypass | `11` Toggle EXP |

Ids 198 and 200 were inferred from `op76` before they were seen written, and
`capture.sh enums` then wrote them: 198 took +6.0 dB and 200 took 12.3 kHz,
exactly where the array said they would be. `op76` returns the whole set as
`{63: enabled, 55: [110, 0.707, 0, 2000, 0.707, 0, 8000, 0.707, 0, 19.9, 20100]}`
- eleven values that 190–200 land on in exactly that order. `op77` is the
window's RESET and answers with the same shape. Opening the window reads ids
201 and 202 (202 = 1, 201 = null on an HX Stomp) rather than the coefficients.

**A negative result worth as much as the map.** Show/Hide Names, Manage
Favorites By and Stereo IR Import send **nothing to the device** - they are
HX Edit's own preferences. Preset Numbering Format sits beside Stereo IR Import
on the same tab and *is* a device write. Nothing in the dialog distinguishes
them.

**Settings.** Most of the pedal's globals are reachable only from its own Global
Settings menu; HX Edit writes a subset, and that subset is what a capture can
name. From HX Edit 3.82's resources it is: **Preferences → View** (name labels,
favourites ordering, preset numbering format), **Preferences → Presets/IRs**
(stereo IR import), **Preferences → Hardware Compatibility** - the device half,
pedal jacks and footswitch functions, which is populated per device and so is
prompted for by name at capture time - the **Global EQ** window (bypass, and the
eleven coefficients: Low Cut, three peaks of freq/Q/gain, High Cut - its
apply-to-1/4"-or-XLR selector is in `globaleq.xml` but hidden on an HX Stomp,
which has no XLR outs), the **tempo** field and its per-snapshot /
per-preset / global / host-sync mode, and a **tuner** panel that ships in
`tuner.xml` though whether an HX Stomp exposes it is untested. Each is changed
alone, under its own mark, which is what turns opcode-25 writes into named
settings and closes the id ↔ name item below.

The scenario deliberately leaves alone Clear Preset Library, Clear IR Library,
Restore Factory Setlists and Restore Factory Settings: they wipe the pedal, and
nothing is learned that a single setting write does not already show.

**A partition falls out of the restore dialog.** *Restore From Backup* lets you
tick which items to restore, so restoring **only Global Settings** from a `.hxb`
writes the globals and nothing else - the cleanest way to learn which `64` ids
in the op-109 object store are globals rather than presets or IRs. Same data
going back, so it costs nothing to run.

## Inventory: do we have everything?

| Thing | Backup (read) | Restore (write) |
|---|---|---|
| Presets | ✓ byte-exact; fast via op 109 (decoding) | `write_preset`+`save_preset` (slow); fast via op 109 inverse (capture restore) |
| Global settings | ✓ 154 objects; also in op-109 store | `set_object` (works); fast via op 109 inverse |
| Setlists | ✓ (name `PRESETS` + the 126 presets) | same as presets |
| IRs | ✓ op12 descriptor + op11 samples, decoded | `upload_ir` (works) |
| Favorites (device models) | ✓ in the op-109 object store | via op 109 inverse |
| Favorites (tonepush-local) | ✓ local file | local |

## Other open reverse-engineering

- **IR read-back.** Decoded - op12 then op11, above. What is left is the reply
  framing of op11's blob, against `captures/ir-exports/`: those are the files
  HX Edit wrote from the same reads, so the bytes on the wire have a known
  answer to be checked against.
- **Globals id ↔ name.** The 21 ids HX Edit writes are named above, values and
  all. The rest of the 154 still want correlating with the `GLOB` block's 156
  named fields by value - they are reachable only from the pedal's own menu, so
  no capture will ever name them.
- **The op-111 blob framing.** The only thing standing between us and the
  object-store path - and op4/op5 may make it unnecessary.
- **Favorites are their own opcodes**, not just object-store entries.
  **Implemented and verified on hardware** (`favourites`, `save_favourite`,
  `rename_favourite`, `clear_favourite`), with the argument shapes read out of
  `captures/mac-library-capture.log`:

  ```
  op112 {}                                  → [{118: index, 64: ?, 105: ?, 109: name}, …]
  op119 {98: block, 118: index, 31: true, 109: name}   keep a block (control channel)
  op113 {118: index}                        read one
  op114 {118: index, 34: {…}, 31: true, 109: name}     write one whole
  op117 {118: index, 109: name}             rename
  op116 {118: index}                        forget
  ```

  op119 is the editor's own "save to favourites" and is the one worth sending:
  it reads the block itself, so there is nothing to pass but where it is and
  what to call it. A favorite exports as an 896-byte JSON `.fav`, a setlist as a
  62 KB `.hls` - both in `captures/library-exports/`, still to be decoded.
