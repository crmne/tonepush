---
title: Opcode Dictionary
description: The HX USB protocol's operation vocabulary, derived from annotated captures of HX Edit driving an HX Stomp.
nav_order: 0
---

Derived from `captures/03-feature-sweep.log` (49 marked UI actions in HX Edit 3.82
driving an HX Stomp on firmware 3.80), cross-checked against
`captures/01-connect-and-sync.log` and `captures/02-ui-actions.log`. Layers 1–4 are
described in [`PROTOCOL.md`](https://github.com/crmne/tonepush/blob/main/PROTOCOL.md); this document covers layer 4's
vocabulary only.

Nothing here is derived from Line 6 source code.

## Confidence key

- **[confirmed]** - isolated to one marked UI action, or cross-checked against a
  second independent source (the preset document, or HX Edit's own data files).
- **[inferred]** - consistent with everything observed, not isolated.
- **[open]** - explicitly undetermined. Read these as "do not guess".

## How the attribution was done

`tools/hxsniff/attribute.py` reassembles each channel's byte stream while keeping a
byte-offset → USB-transfer index, so every decoded application message can be traced
back to the transfer that carried its first byte and from there to the `### MARK`
line in force at that moment.

```
tools/hxsniff/attribute.py CAPTURE.log --marks          # every mark + what it produced
tools/hxsniff/attribute.py CAPTURE.log --mark PARAM-    # one group of marks
tools/hxsniff/attribute.py CAPTURE.log --opcode 30      # one opcode and its replies
tools/hxsniff/attribute.py captures/*.log --dict        # the inventory below
```

### A framing correction that changes the readings

`PROTOCOL.md` lists channel message type `0x0c` as "start-of-stream", and
`reassemble.py` used to keep only type `0x04`. That is wrong: **bit `0x04` means
"carries stream data"**, and it is set on `0x04` (data), `0x0c` (data + piggybacked
acknowledgement) and `0x14` (data on a keep-alive slot). Reading only `0x04`
silently dropped 26 messages in `03-feature-sweep` and 10 in `01-connect-and-sync`
- including two thirds of every slider gesture, which made it look as though the
device were pushing unsolicited replies. Both scripts now match `typ & 0x04`.
**[confirmed]** - after the fix every request in all three captures has a matching
reply, every reply a matching request, and all three streams decode to the last byte
with no MessagePack desynchronisation.

The layer-3 field at offset 2 (the one `PROTOCOL.md` flags as usually the service
id) is **not reliable**: the byte-identical opcode-0 reply carries `0x0000` in
captures 01/02 and `0x28e1` in capture 03. Ignore bytes 2..3; only the `u32` length
at offset 4 matters. **[confirmed]**

## Transaction discipline

`txn` (key 102) is a per-channel counter starting at 1000, allocated by the host
only. Every request gets exactly one reply. **[confirmed]**

**Status (key 103) is a completion mode, not an error code. [confirmed]**

| 103 | meaning |
|---|---|
| `0` | complete - `104` holds the result |
| `1` | accepted, asynchronous - `104` is nil and the real completion arrives later as **notification 20** carrying the same `102` |
| `255` | refused - `104` is `{111: signed error code}` |

Both async cases in the captures are unambiguous:

```
# 02-ui-actions, mark LOAD-PRESET-05A-FX-5th-Then-7th
--> {102: 1009, 100: 20, 101: {107: 0, 108: 12}}
<-- {102: 1009, 103: 1, 104: None}
<-- {105: 20, 106: {102: 1009, 103: 0, 104: None}}     # 1 ms later, on 0x1002

# 03-feature-sweep, mark UNDO
--> {102: 1041, 100: 21, 101: {110: <2643-byte preset document>}}
<-- {102: 1041, 103: 1, 104: None}                     # 83 66 cd 04 11 67 01 68 c0
<-- {105: 20, 106: {102: 1041, 103: 0, 104: None}}     # 82 69 14 6a 83 66 cd 04 11 67 00 68 c0
```

A client must therefore not treat `103: 1` as failure, and must keep a pending-txn
table keyed on 102 so notification 20 can complete it.

HX Edit's traffic contains only 0 and 1; status 255 surfaced once deliberately
bad requests were sent (a parameter on an empty slot, snapshot 7, model 99999),
answering `{111: -3}`, `{111: -46}` and `{111: -302}` respectively. Note that
the asynchronous acceptances are **not validated up front** - selecting preset
999 on a 126-preset device answers `103: 1` and then nothing happens - and that
a no-op such as clearing an empty IR slot is an honest `103: 0`.

---

## 1. Opcode table

Channel column is the device node the request goes to: `0x1001` session control,
`0x1080` preset and global data. Nothing is ever sent by the host on `0x1002` beyond
the channel hello.

| Op | Chan | Triggered by | Args (key 101) | Reply (key 104) | Conf |
|---|---|---|---|---|---|
| 0 | `0x1001` | connect | nil | `[{setlist: name}]` | [confirmed] |
| 1 | `0x1001` | connect | `{107: setlist, 101: 2}` | `[{index: {109: name, 123: bool, 124: bool, 125: int}}]` ×126 | [confirmed] |
| 13 | `0x1001` | connect | `{101: 2}` | nil | [inferred] |
| 20 | `0x1080` | double-click a preset in the librarian | `{107: setlist, 108: index}` | nil, `103: 1` | [confirmed] |
| 21 | `0x1080` | Undo (of a model change) | `{110: <preset document>}` | nil, `103: 1` | [confirmed] |
| 22 | `0x1080` | connect; after a preset load | nil | preset document (blob) | [confirmed] |
| 23 | `0x1080` | connect; after a preset load | nil | `{107, 108, 109: name, 117: bool, 83: [int,int], 92: int}` | [confirmed] |
| 24 | `0x1080` | connect; Global Settings gear | `{118: object id}` | `{118: id, 119: value}` | [confirmed] |
| 30 | `0x1080` | any parameter edit | `{98, 29, 26, 28, 119}` - see §4 | echo of the args | [confirmed] |
| 40 | `0x1080` | pick a model in the browser; Redo | `{98: block, 100: {23: bool, 25: model, 26: model}}` | `{13: 1, 24: <slot object>}` | [confirmed] |
| 42 | `0x1080` | click a routing choice on an Input or Output block | `{98: slot, 51: destination}` | nil | [confirmed] |
| 41 | `0x1080` | click a block's bypass switch | `{98: block, 59: enabled bool}` | nil | [confirmed] |
| 64 | `0x1003` | set which MIDI CC drives an assigned *parameter* | `{98: block, 29: true, 26: 0, 28: param, 71: cc}` | nil | [confirmed] |
| 76 | `0x1080` | connect | `{}` | `{63: bool, 55: [11 floats]}` - Global EQ | [confirmed] |
| 78 | `0x1080` | click a block in the signal chain | `{98: block, 26: 0}` | nil | [confirmed] |
| 99 | `0x1080` | connect | `{}` (arguments ignored) | `{63: bool}` - is the tempo driven by external MIDI clock. HX Edit shows `[External]` instead of the BPM when true. | [confirmed] |
| 112 | `0x1001` | connect | nil | nil | [open] |
| 254 | `0x1001` | connect | `{}` | nil | [open] |

Opcodes 6, 25, 59, 61 and 68 listed in `PROTOCOL.md` come from `kempline/helix_usb`
and **do not appear in any of our captures**. They are not corroborated here.

### 1.1 Worked examples

Each is quoted as the layer-3 body (after the 8-byte stream header), taken verbatim
from the capture.

**op78 - select block.** `BLOCK-select-EQ1`, transfer 2298:

```
83 66 cd 03 f2 64 4e 65 82 62 02 1a 00
--> {102: 1010, 100: 78, 101: {98: 2, 26: 0}}
<-- {102: 1010, 103: 0, 104: None}
<-- {105: 39, 106: {82: 1, 68: 3, 121: 19, 106: {98: 2, 26: 0}}}
```

Block 2 of `CT-Sad` is `HD2_CaliQMono` - the EQ block the operator clicked. The four
`BLOCK-select-*` marks produced `98:` 1, 2, 3 and 6, matching preset slots 1
(Scream 808), 2 (Cali Q), 3 (Cali Rectifire + cab) and 6 (LA Studio Comp) exactly.

**op41 - bypass.** `BLOCK-bypass-toggle` then `BLOCK-bypass-restore`, transfers 5264
and 5322:

```
83 66 cd 04 07 64 29 65 82 62 06 3b c2   --> {102: 1031, 100: 41, 101: {98: 6, 59: False}}
83 66 cd 04 08 64 29 65 82 62 06 3b c3   --> {102: 1032, 100: 41, 101: {98: 6, 59: True}}
<-- {105: 49, 106: {82: 0, 68: 5, 121: 17, 106: {98: 6, 59: False}}}
```

The block was on before the click, so **59 = enabled** (`True` = active, `False` =
bypassed), not "bypassed". **[confirmed]**

**op40 - change a block's model.** `MODEL-change-pick-first`, transfer 7152:

```
83 66 cd 04 0f 64 28 65 82 62 01 64 83 17 c2 19 cd 01 84 1a ff
--> {102: 1039, 100: 40, 101: {98: 1, 100: {23: False, 25: 388, 26: -1}}}
<-- {102: 1039, 103: 0,
     104: {13: 1, 24: {19: 6, 20: {24: {23: False, 25: 388, 26: -1},
                                   9: 1, 10: True,
                                   11: {2: 3, 3: 3, 4: [0.55, False, False]},
                                   12: {2: 0, 3: 0, 4: []}}}}}
```

Note key `100` is reused *inside* `101` as the model descriptor. It is nested, so
there is no ambiguity, but a naive "key 100 means opcode" reader will break.

**op24 - read a device object.** `GLOBAL-settings-gear` opened the Global Settings
dialog and read five objects in a row, 52 ms apart:

```
--> {102: 1033, 100: 24, 101: {118: 95}}   <-- {118: 95, 119: False}
--> {102: 1034, 100: 24, 101: {118: 96}}   <-- {118: 96, 119: True}
--> {102: 1035, 100: 24, 101: {118: 97}}   <-- {118: 97, 119: 0}
--> {102: 1036, 100: 24, 101: {118: 98}}   <-- {118: 98, 119: 10}
--> {102: 1037, 100: 24, 101: {118: 99}}   <-- {118: 99, 119: 11}
```

The connect sequence reads ids 128, 14, 73, 136 and 27. **Which setting each id names
is [open]** - the capture only shows values, and no click in the sweep changed one.
Two ids are pinned from elsewhere: **16 = tempo in BPM** and **28 = current preset
index** (see §3, notification 22).

**No write counterpart to opcode 24 was captured.** The three `GLOBAL-tab-*` marks
and `GLOBAL-close` produced zero traffic, so the opcode that writes a global setting
is **[open]**.

### 1.2 The connect sequence

Identical in content and order across all three captures - only `117` in the op23
reply differed (see §2.2) **[confirmed]**:

```
0x1001 hello(5)  ·  0x1080 hello(6)  ·  0x1002 hello(4)  ·  0x1001 hello(2)
0x1080  op76 {}            -> global EQ
0x1080  op24 {118: 128}    -> 0
0x1080  op23 nil           -> {107: 0, 108: 7, 109: 'CT-Sad', ...}
0x1080  op22 nil           -> 2531-byte preset document
0x1080  op24 {118: 14}     -> 1
0x1080  op24 {118: 73}     -> 0
0x1080  op24 {118: 136}    -> 0
0x1001  op254 {}           -> nil
0x1080  op24 {118: 27}     -> False
0x1080  op99 {}            -> {63: False}
0x1001  op0 nil            -> [{0: 'PRESETS'}]
0x1001  op1 {107: 0, 101: 2} -> 126 preset names
0x1001  op112 nil          -> nil
0x1001  op13 {101: 2}      -> nil
```

126 entries confirms **key 108 is a linear zero-based preset index over the whole
setlist**, not a bank number: the librarian's `05A` is `108: 12`, and 12/3+1 = 5,
12 mod 3 = 0 → `A`. **[confirmed]**

---

## 2. Key dictionary

### 2.1 Envelope keys

| Key | Meaning | Type |
|---|---|---|
| 100 | opcode (request) | int |
| 101 | arguments (request) | map or nil |
| 102 | transaction id | int, from 1000 per channel |
| 103 | completion mode - 0 done, 1 async (see above) | int |
| 104 | result (response) | any |
| 105 | notification id | int |
| 106 | notification payload; **also** the inner argument key one level down | map or nil |

### 2.2 Argument and result keys

| Key | Meaning | Type | Conf |
|---|---|---|---|
| 2 | parameter-array capacity | int | [inferred] |
| 3 | parameter-array count actually present | int | [inferred] |
| 4 | parameter value array | array | [confirmed] |
| 9 | model id, repeated alongside `24.25` | int | [inferred] |
| 10 | block enabled | bool | [inferred] |
| 11 | primary model's parameter group `{2, 3, 4}` | map | [confirmed] |
| 12 | secondary (cab) model's parameter group | map | [confirmed] |
| 13 | in an op40 result - always `1`; inside a split or join body it is the slot the branch attaches before | int | [confirmed for split/join, open elsewhere] |
| 16 | tempo, BPM (preset-level, key `5`) | float | [confirmed] |
| 19 | slot kind: 0 input, 1 output, 2 split, 3 join, 6 block, 8 empty | int | [confirmed] |
| 20 | slot contents | map or nil | [confirmed] |
| 22 | array of 20 slots | array | [confirmed] |
| 23 | model descriptor has a paired second model (amp+cab) | bool | [inferred] |
| 24 | model descriptor `{23, 25, 26}`; in an op40 *result*, the whole slot object | map | [confirmed] |
| 25 | primary model id - index into `Helix.sym` | int | [confirmed] |
| 26 | in a model descriptor: secondary model id, `-1` = none | int | [confirmed] |
| 26 | in op30 / op78 / preset key 6: block sub-address, always `0` here | int | [open] |
| 28 | parameter index - see §4 | int | [confirmed] |
| 29 | in op30, always `True` | bool | [open] |
| 55 | Global EQ: 11 floats, three bands of freq/Q/gain plus low-cut and high-cut | array | [confirmed] |
| 59 | block enabled (op41) | bool | [confirmed] |
| 63 | an enable flag; Global EQ on/off in op76, something else in op99 | bool | [inferred] |
| 68 | notification topic - see §3 | int | [inferred] |
| 82 | notification flag, 0 or 1, tracks `68` - see §3 | int | [open] |
| 83 | in op23: `[int, int]`, stable per preset, changes with the preset - most likely DSP usage in hundredths of a percent per core | array | [inferred] |
| 92 | in op23, always 0 | int | [open] |
| 98 | block index - index into the preset's slot array `0.22[]` | int | [confirmed] |
| 107 | setlist index | int | [confirmed] |
| 108 | linear zero-based preset index (0..125 on HX Stomp) | int | [confirmed] |
| 109 | name | string (C string, NUL included in the msgpack length) | [confirmed] |
| 110 | a whole preset document, as op21's argument | blob | [confirmed] |
| 117 | in op23 - `True` in captures 01/02, `False` in 03 for the same preset; most likely "has unsaved edits" | bool | [inferred] |
| 118 | device object id (op24, notification 22) | int | [confirmed] |
| 119 | value - of an object (op24) or a parameter (op30) | float / bool / int | [confirmed] |
| 121 | notification sub-type - see §3 | int | [inferred] |
| 123, 124, 125 | per-preset flags in the op1 listing; `False, False, 0` for all 126 presets | bool, bool, int | [open] |

Key 101 is used both as the top-level "arguments" key and, inside op1 and op13's
arguments, as an ordinary key with the value 2. Its inner meaning is **[open]**.

---

## 3. Notification events

All notifications arrive on channel `0x1002` (device node `0x1002`, host `0x03f0`).
Two shapes exist:

```
{105: 20, 106: {102: txn, 103: status, 104: result}}      # deferred completion
{105: id, 106: {82: f, 68: topic, 121: sub, 106: args}}   # state change
```

**Dispatch on 105, and on 121 within it.** `105` alone is not enough: id 22 covers
two different messages and id 49 covers three. The `(82, 68, 121)` tuple was
perfectly stable across all three captures. **[confirmed]**

| 105 | 82 | 68 | 121 | Args | Produced by | Conf |
|---|---|---|---|---|---|---|
| 4 | 1 | 1 | 6 | `{107, 108}` | preset load - the *last* event of the load | [inferred] |
| 8 | 1 | 1 | 5 | `{107, 108}` | preset load - the *first* event of the load | [inferred] |
| 20 | - | - | - | `{102, 103, 104}` | deferred completion of an earlier `103: 1` reply | [confirmed] |
| 21 | - | - | - | nil | after every document-write completion - follows notification 20 in all fourteen captured undos | [confirmed pattern, meaning inferred: post-commit tick] |
| 22 | 0 | 9 | 25 | `{118: id, 119: value}` | a device object changed | [confirmed] |
| 22 | 0 | 10 | 27 | nil | see below | [open] |
| 30 | 0 | 6 | 20 | `{98, 29, 26, 28, 119}` | a parameter changed | [confirmed] |
| 39 | 1 | 3 | 19 | `{98, 26}` | selected block changed | [confirmed] |
| 49 | 0 | 5 | 10 | `{98}` | slot rebuilt (op40) | [confirmed] |
| 49 | 0 | 5 | 17 | `{98, 59}` | bypass changed | [confirmed] |
| 49 | 0 | 5 | 47 | nil | emitted with 49/10 on every op40 | [confirmed] |

`82` and `68` are both functions of the message: `68 ∈ {1,3}` always came with
`82: 1`, `68 ∈ {5,6,9,10}` always with `82: 0`. What either field *means*
individually is **[open]**; the tuple behaves like a `(class, interface, method)`
tag from whatever RPC framework Line 6 use internally.

### 3.0 The pedal's own controls announce the same events

Watched on an HX Stomp at 3.80 while its controls were used, with no host
request in flight **[confirmed on that device]**:

- a preset footswitch: 8, then 22 for objects 16, 15 and 28, then 4, as for a
  load the host asked for;
- a snapshot footswitch: `42 {92: index}`, a 49/17 for each block whose bypass
  the snapshot changes, `23 {23: 0}`, then `46 {92: index}`;
- a stomp switch: `41 {70: switch, 63: pressed, 66: n}` for the press and again
  for the release, each followed by 39 and a 49/17 with the block and its new
  state. What key 66 holds is **[open]**;
- a turned knob: one 30 per step, with the block, the parameter index and the
  value.

### 3.1 Notifications echo the host's own writes

Every notification in the sweep was caused by a *host* request, and the device still
broadcast it. A client that also renders the signal chain will see its own edits come
back and must not treat them as external changes without comparing values.
**[confirmed]**

### 3.2 Notification 22 / (0, 9, 25) - object changed

Loading a preset produced three of these:

```
{105: 22, 106: {82: 0, 68: 9, 121: 25, 106: {118: 16, 119: 120.0}}}
{105: 22, 106: {82: 0, 68: 9, 121: 25, 106: {118: 16, 119: 120.0}}}
{105: 22, 106: {82: 0, 68: 9, 121: 25, 106: {118: 28, 119: 12}}}
```

Object 28's value 12 is the preset index just selected. Object 16's value 120.0 is
the *newly loaded* preset's `5.16`; the preset being replaced had `5.16 =
75.789474`, so the notification is reporting the change rather than echoing the old
state. Both are **[confirmed]** by that cross-check. This is the same `118` id space
opcode 24 reads, so `op24 {118: 16}` should read the tempo back - untested, because
the connect sequence never asks for it.

### 3.3 Notification 22 / (0, 10, 27) - an argument-free tick

479 of these across the captures, always with `106: None`, so they carry no
information beyond "something happened". Every inter-arrival gap is an exact multiple
of 75 ms, and they cluster densely just after the DSP rebuilds (preset load, model
change, undo/redo). **What they report is [open].** They are safe to ignore: nothing
in the sweep depends on them.

### 3.4 The event order for a model change

```
--> op40 {98: 1, 100: {23: False, 25: 388, 26: -1}}
<-- rsp  {13: 1, 24: <new slot object>}
<-- ev49 {82: 0, 68: 5, 121: 47, 106: None}
<-- ev49 {82: 0, 68: 5, 121: 10, 106: {98: 1}}
<-- ev39 {82: 1, 68: 3, 121: 19, 106: {98: 1, 26: 0}}
<-- ev22 x11  (0, 10, 27)
```

The op40 reply already contains the rebuilt slot including its new default parameter
values, so a client does not need to re-read the preset after a model change.
**[confirmed]**

---

## 4. Parameter addressing - opcode 30

This is the load-bearing result. Six `PARAM-*` marks in the sweep each touched a
different control on one block, and one more in capture 02 touched a different block.

The block under edit was preset slot 6 of `CT-Sad`, model id **120 =
`HD2_CompressorLAStudioCompMono`** - LA Studio Comp, whose parameters in
`Helix.sym` are, in order:

```
0 PeakReduction   1 Gain   2 Type   3 Emphasis   4 Mix   5 Level
```

What each mark sent:

| Mark | args | 28 | resolves to |
|---|---|---|---|
| `PARAM-PeakReduction-slider` | `{98: 6, 29: True, 26: 0, 28: 0, 119: 0.33 → 0.05}` | 0 | PeakReduction |
| `PARAM-Gain-slider` | `{98: 6, 29: True, 26: 0, 28: 1, 119: 0.57 → 0.34}` | 1 | Gain |
| `PARAM-Type-toggle-Limit` | `{98: 6, 29: True, 26: 0, 28: 2, 119: False → True}` | 2 | Type |
| `PARAM-Type-toggle-Compress` | `{98: 6, 29: True, 26: 0, 28: 2, 119: True → False}` | 2 | Type |
| `PARAM-Mix-slider` | `{98: 6, 29: True, 26: 0, 28: 4, 119: 1.0 → 0.78}` | 4 | Mix |
| `PARAM-Level-spinner-up` | `{98: 6, 29: True, 26: 0, 28: 5, 119: 0.0 → -0.1}` | 5 | Level |

Index 3 (Emphasis) was never touched, and no mark ever produced `28: 3` - the gap is
exactly where it should be.

**So: `98` is the block index and `28` is the parameter index. `26` is neither.
[confirmed]**

### 4.1 Key 28 indexes `Helix.sym`, not `.models`

`Helix.sym` lists 5 parameters for `HD2_Cab4x121960T75`; `cab.models` lists 7 for the
same model, because it adds the structural pseudo-parameters `@mic` and `@enabled`.
The preset stores exactly the 5, in `Helix.sym` order. Index against the `Helix.sym`
parameter list (or filter the `@`-prefixed entries out of `.models`). **[confirmed]**

### 4.2 Key 119 is the parameter's native value, not a normalised 0..1

`HelixModelDefs`' entry for LA Studio Comp settles this:

| Param | `valueType` | `min` | `max` | wire value seen |
|---|---|---|---|---|
| PeakReduction | 1 (float) | 0.0 | 1.0 | 0.33, 0.05 |
| Gain | 1 | 0.0 | 1.0 | 0.57, 0.34 |
| Type | 2 (bool) | false | true | `False`, `True` |
| Mix | 1 (`percent`) | 0.0 | 1.0 | 1.0, 0.78 |
| Level | 1 (`volume`) | **-120.0** | **12.0** | 0.0, **-0.099998** |

Level moved by exactly one 0.1 dB spinner step to **-0.1 dB** - a raw decibel value,
impossible under a 0..1 normalisation. (The mark is named `-spinner-up` but the value
went *down*; the click landed on the down arrow. The step size is what matters.) The
same holds elsewhere in the preset: slot 3's cab stores `LowCut 80.0` (Hz),
`HighCut 8000.0` (Hz) and `Distance 3.0` (inches), while the amp's twelve knobs are
all 0..1.

**Key 119 carries the value in whatever units `HelixModelDefs` declares via
`min`/`max`/`valueType` for that model's parameter. Some are 0..1 (knobs shown
0.0–10.0, percentages shown 0–100 %), some are real units (dB, Hz, inches, ms), some
are bools. There is no single scaling. [confirmed]**

Booleans are MessagePack `true`/`false` (`0xc3`/`0xc2`), not floats. For LA Studio
Comp's Type, `False` = Compress and `True` = Limit, from the marks. **[confirmed]**

Floats are float32, so values round-trip inexactly - `-0.1` arrives as `-0.099998`.
Compare with a tolerance.

### 4.3 One gesture is three requests

Every slider drag, toggle click and spinner click sent **three** op30s:

```
177.369 --> {102: 1013, 100: 30, 101: {98: 6, 29: True, 26: 0, 28: 0, 119: 0.33}}   # old value
177.371 <-- {102: 1013, 103: 0, 104: {98: 6, 29: True, 26: 0, 28: 0, 119: 0.33}}
177.419 --> {102: 1014, 100: 30, 101: {98: 6, 29: True, 26: 0, 28: 0, 119: 0.05}}   # new value
177.422 <-- {102: 1014, 103: 0, 104: {98: 6, 29: True, 26: 0, 28: 0, 119: 0.05}}
177.423 <-- {105: 30, 106: {82: 0, 68: 6, 121: 20, 106: {98: 6, ..., 119: 0.05}}}
177.471 --> {102: 1015, 100: 30, 101: {98: 6, 29: True, 26: 0, 28: 0, 119: 0.05}}   # re-commit
177.474 <-- {102: 1015, 103: 0, 104: {98: 6, 29: True, 26: 0, 28: 0, 119: 0.05}}
```

The first request always re-sends the value the preset already held (verified against
the preset document for all seven parameter marks), the second carries the new value,
the third repeats it. Only the value-changing request produces notification 30.
This looks like HX Edit's mouse-down / move / mouse-up handling rather than anything
the protocol requires; **a client can send one op30 [inferred]** - nothing in the
capture suggests the first and third are needed.

The wire form of the middle request and the resulting notification, verbatim:

```
--> 83 66 cd 03 f6 64 1e 65 85 62 06 1d c3 1a 00 1c 00 77 ca 3d 4c cc cd
<-- 82 69 1e 6a 84 52 00 44 06 79 14 6a 85 62 06 1d c3 1a 00 1c 00 77 ca 3d 4c cc cd
```

### 4.4 What keys 26 and 29 are - not determined

`26` was `0` and `29` was `True` in all 21 op30 calls across all three captures, so
neither can be pinned. **[open].** Two readings of `26` fit everything seen, and the
capture cannot separate them:

1. **Which of the slot's two models** - a slot carries a primary parameter group
   (key `11`) and a secondary one (key `12`) for the cab half of an Amp+Cab. `26`
   would select between them, and would have stayed 0 because no cab parameter was
   ever edited. This is supported by `26` naming the *secondary model id* inside a
   model descriptor, and by op78 `{98, 26}` addressing the same pair.
2. **Path index** - HX Stomp has one signal path, so it would always be 0.

The experiment that settles it: select the Amp+Cab block, edit a cab parameter
(Low Cut), and see whether `26` becomes 1 or `28` continues past the amp's 12
parameters.

`29` may be a "commit to the preset" or "this came from the editor" flag; nothing in
the sweep varies it.

### 4.5 A mark whose label is wrong

`PARAM-Drive-set` in `02-ui-actions` is labelled for a distortion block, but by then
the operator had loaded preset 12 (`FX:5th Then 7th`), whose slot 1 is an LA Studio
Comp. The request `{98: 1, 29: True, 26: 0, 28: 0, 119: 0.78 → 0.19}` matches that
preset's stored `PeakReduction 0.78` exactly. The addressing is confirmed; the
*label* is not. Read that mark as "some parameter 0 of slot 1".

---

## 5. Block and model representation

**A block is addressed by its index in the preset document's slot array
`preset[0][22]`, a fixed 20-entry array. [confirmed]** Key `98` is that index
everywhere it appears.

| Index | Contents on HX Stomp |
|---|---|
| 0 | input (`19: 0`) |
| 1..8 | path A block slots (`19: 6`, or `19: 8` when empty) |
| 9 | output (`19: 1`) |
| 10 | split (`19: 2`) |
| 11..18 | path B block slots |
| 19 | join (`19: 3`) |

The 1..8 / 11..18 split is **[inferred]** - preset `FX:5th Then 7th` has processing
blocks at 1, 3, 4, 5, 6 and at 12, with a split at 10 and a join at 19, which is
exactly one parallel path.

### 5.1 Model identity on the wire - resolved

**The numeric model id in keys 25 and 26 is the zero-based index into
`Helix.sym`. [confirmed]** This closes the "numeric model id ↔ symbolic name"
gap that has since been closed - see `PROTOCOL.md` on the section table and
`Preset::computed_sections`.

Validated on ten independent models - both the symbolic name and the *parameter
count* match what the preset serialises:

| id | `Helix.sym` symbol | params in `.sym` | params in preset | where |
|---|---|---|---|---|
| 18 | `HD2_AmpCaliRectifire` | 12 | `11: {2: 12, 3: 12}` | CT-Sad slot 3 |
| 64 | `HD2_Cab4X12CaliV30` | 5 | `12: {3: 5, 4: [3.0, 80.0, 8000.0, 0.0, 0.0]}` | slot 3's cab, captures 01/02 |
| 62 | `HD2_Cab4x121960T75` | 5 | `12: {3: 5, 4: [8.0, 19.9, 10200.0, 0.24, 0.0]}` | slot 3's cab, capture 03 |
| 80 | `HD2_DelaySimpleDelayMono` | 6 | 6 values | FX preset slot 3 |
| 101 | `HD2_DistScream808Mono` | 3 | `11: {2: 3, 3: 3}` | CT-Sad slot 1 |
| 120 | `HD2_CompressorLAStudioCompMono` | 6 | 6 values | CT-Sad slot 6 |
| 129 | `HD2_EQGraphic10BandMono` | 11 | 11 values | CT-Sad slot 5 |
| 151 | `HD2_AppDSPFlowJoin` | 6 | 6 values | slot 19 |
| 257 | `HD2_AppDSPFlowSplitY` | 3 | 3 values | slot 10 |
| 323 | `HD2_CaliQMono` | 6 | 6 values | CT-Sad slots 2, 4 |
| 388 | `HD2_DistKinkyBoostMono` | 3 | op40 reply `[0.55, False, False]` | MODEL-change |
| 510 | `HD2_DistDerangedMasterMono` | 4 | op40 reply `[0.77, 0.522, 0.65, 0]` | MODEL-change |

The cab case is the strongest single check: model 64's stored
`[3.0, 80.0, 8000.0, 0.0, 0.0]` lines up with `Helix.sym`'s
`['Distance', 'LowCut', 'HighCut', 'EarlyReflections', 'Level']` and with
`cab.models`' `HD2_Cab4X12CaliV30` defaults of exactly 3.0 in, 80 Hz and 8000 Hz -
five values, five names, five defaults, all in order. Model 62 in capture 03 has the
same shape with the user's own values, and its high cut of 10200 Hz sits inside the
declared 500–20100 Hz range.

`Helix.sym` is Line 6 proprietary data shipped inside HX Edit. Read it from the
user's own installation at runtime; do not vendor it. See the [model catalog](https://github.com/crmne/tonepush/blob/main/docs/_reference/model-catalog.md).

### 5.2 Model descriptor

```
24: {23: <paired>, 25: <primary model id>, 26: <secondary model id or -1>}
```

For an ordinary block: `{23: False, 25: n, 26: -1}`. For CT-Sad's Amp+Cab slot:
`{23: True, 25: 18, 26: 62}` - amp `HD2_AmpCaliRectifire` plus cab
`HD2_Cab4x121960T75`, with parameter group `11` holding the amp's 12 values and group
`12` the cab's 5. **[confirmed]** that this is the layout; **[inferred]** that `23`
means "paired".

Split and join slots put the model id in key `8` instead
(`15: {8: 257, ...}`, `17: {8: 151, ...}`) with parameters under key `7`.
**[confirmed]** from the same cross-check.

### 5.3 Changing a model

```
--> {102: t, 100: 40, 101: {98: <slot>, 100: {23: False, 25: <new model>, 26: -1}}}
<-- {102: t, 103: 0, 104: {13: 1, 24: <the rebuilt slot object>}}
```

The device supplies the new model's default parameters in the reply. Setting an
Amp+Cab pair presumably means sending `{23: True, 25: amp, 26: cab}`, but that was
never captured - **[inferred]**.

**The device never enumerates models.** All fifteen `CAT-*` marks and `CAT-Distortion-open`
produced **zero USB traffic**: HX Edit's model browser is
served entirely from its local catalog files. A third-party editor must read
`HX_ModelCatalog.json` / `HelixModelDefs.bin` / `Helix.sym` from the user's HX Edit
installation, or ship its own table. **[confirmed]**

---

## 6. Presets, setlists, snapshots, tempo, globals, undo/redo, copy

### 6.1 Setlists and preset lists - [confirmed]

`op0` returns `[{0: 'PRESETS'}]`: one setlist, index 0, named `PRESETS`. `op1
{107: 0, 101: 2}` returns 126 entries `{index: {109: name, 123: False, 124: False,
125: 0}}`. Flags 123/124/125 never varied → **[open]**.

### 6.2 Loading a preset - [confirmed]

Selecting a preset in the librarian list sends nothing; the three
`SELECT-PRESET-*` marks in `02-ui-actions` produced zero traffic. Only opening it
(double-click) does:

```
op20 {107: 0, 108: 12}  ->  103: 1
ev20 {102: <same txn>, 103: 0}        # completion
ev8  (1,1,5)  {107: 0, 108: 12}       # load started
ev22 (0,9,25) {118: 16, 119: 120.0}   # tempo of the new preset
ev39 (1,3,19) {98: 4, 26: 0}          # its saved cursor position
ev22 (0,9,25) {118: 28, 119: 12}      # current preset index
ev4  (1,1,6)  {107: 0, 108: 12}       # load finished
op23 nil -> metadata incl. the name
op22 nil -> the preset document
```

The name is not in the preset document; it comes from op23. HX Edit re-reads both
after the load rather than trusting a cached copy.

### 6.3 Undo and redo - [confirmed], and they are client-side

`UNDO` after two model changes sent **opcode 21 with a complete 2643-byte preset
document** in key 110 - HX Edit pushed the previous state wholesale rather than
asking the device to undo:

```
--> {102: 1041, 100: 21, 101: {110: <'l6-helix' + section table + preset map>}}
<-- {102: 1041, 103: 1, 104: None}
<-- {105: 39, ...}                      # cursor restored to block 1
<-- {105: 20, 106: {102: 1041, 103: 0}} # completion
<-- {105: 21, 106: None}
```

`REDO` did **not** use opcode 21 - it re-issued the original `op40 {98: 1, 100:
{23: False, 25: 510, 26: -1}}`. So the undo stack lives in the editor, and opcode 21
is really **"write this preset document into the edit buffer"** - which is also the
opcode a third-party tool would use to upload a preset. **[inferred]** that it is a
general write rather than an undo-specific call; nothing observed contradicts it, but
it was only ever seen carrying a previously-read document.

Notification 21 follows the completion of every document write - fourteen
consecutive captured undos all show `20` then `21`. It reads as a post-commit
tick; nothing acts on it. [confirmed pattern]

### 6.4 Snapshots - no traffic, no conclusion

`SNAPSHOT-selector-open` and `SNAPSHOT-pick` both produced **zero USB traffic**.
Either the picker is local until a different snapshot is chosen, or the click landed
on the snapshot already active. **No opcode for switching snapshots was captured here**, but it is opcode 88 -
found later by driving the snapshots from the keyboard rather than the mouse,
since HX Edit sends nothing when a click lands on the already-active snapshot.
See PROTOCOL.md. [confirmed]

What the capture does show is where snapshots live in the preset document: key `10`
holds `{6, 7, 8, 9: 20, 10: [<3 snapshot objects>], 13: [bool × 20]}`, each snapshot
carrying its name (`4: 'SNAPSHOT 1'`), tempo (`5: 120.0`) and per-block state.
`10.8` was 0 for `CT-Sad` and 1 for `FX:5th Then 7th`, so **`10.8` is plausibly the
active snapshot index [inferred]**.

### 6.5 Tempo

`TEMPO-click` produced **zero traffic** - the click opened a local control.
Tempo itself is reachable: it is device object **id 16**, reported as
`{118: 16, 119: 120.0}` and stored in the preset as `5.16`. **[confirmed]** for the
identity of the object; the opcode that *writes* it is **[open]** (op24 is a read;
no write was captured).

### 6.6 Global settings

`GLOBAL-settings-gear` read objects 95–99 via op24. The three `GLOBAL-tab-*` marks
and `GLOBAL-close` produced **zero traffic** - the dialog's tabs are local and
nothing was changed, so no write opcode was exercised. **[open].**

### 6.7 Preset copy

`PRESET-copy` and `PRESET-select-target` both produced **zero traffic** (keep-alives
only) - so copy in the librarian is a client-side clipboard operation, which is an
answer rather than a gap. Paste is a document write (op21): this project implements
copy, paste, import and export that way, and the round trip is verified byte-exact
against hardware. [confirmed]

---

## 7. Marks that produced no traffic

Reported honestly rather than guessed at. 31 of the 49 marks are here - only 18
marks in the sweep produced any application traffic at all.

| Mark(s) | Reading |
|---|---|
| `BLOCK-select-Dist` | The block was already selected - the earlier `TEST-MARK` click had selected slot 1, and re-clicking the selected block sends nothing. The later `BLOCK-select-Dist-again` did send `op78 {98: 1, 26: 0}`. |
| `CAT-*` (15 marks), `CAT-Distortion-open` | The model browser is local; the device is not consulted. |
| `TAB-*` (5 marks) | Editor tabs are local. |
| `SNAPSHOT-selector-open`, `SNAPSHOT-pick` | See §6.4 - cannot distinguish "local UI" from "click missed". |
| `TEMPO-click` | Local control. |
| `GLOBAL-tab-*` (3), `GLOBAL-close` | Local; nothing was changed. |
| `PRESET-copy`, `PRESET-select-target` | Client-side clipboard; librarian selection is local. |

Verified with `attribute.py --marks`, which counts *transfers* per mark as well as
decoded messages: each of these windows contains only keep-alives and bare
acknowledgements, so the silence is real and not a decoding failure.

---

## 8. Open questions worth one more capture

1. **What key 26 selects in op30** - edit a cab parameter on an Amp+Cab block (§4.4).
2. **The write side of op24** - change a value in Global Settings while capturing.
3. **Snapshot switching** - switch to a genuinely different snapshot.
4. **Real error codes** - send a malformed request (e.g. a block index of 99) and see
   what `103` comes back.
5. **Key 29 in op30** - no idea how to vary it from the UI.
6. **Whether op21 accepts an arbitrary preset document** - the only way to know
   whether a third-party tool can upload presets with it.
7. **Notification 22/(0,10,27)** - 479 argument-free ticks on a 75 ms grid.
