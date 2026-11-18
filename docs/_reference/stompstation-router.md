---
title: StompStation PRO Routing
description: How firmware 2.x describes, edits and stores the StompStation PRO's signal chain in the root\app\router node.
nav_order: 2
---

Firmware 2.x replaced the PRO's fixed 1.5.12 chain with a router. This page
records what the router node holds and how the pedal treats a write, read from
the 2.0.10 and 2.2.6 firmware (the router code is identical in both apart from
how parallel branches mix) and from a 2.0.10 pedal's schema. On a pedal at
2.0.10, writes covering every rule below (fixed positions, moves, two- and
three-way banks, connector normalisation, duplicates, unknown paths, malformed
values, both chain actions and a preset reload) read back exactly as described.
A 2.4.4 pedal describes the same router (one row of 14 positions, with the amp,
delay and reverb fixed); on it one exchange of two movable positions was written,
read back exactly and restored, and the other rules were not rechecked.
Facts not yet checked on hardware are marked *(unconfirmed)*.

## The value

`root\app\router` has type `router`. It is live state inside the loaded
preset; it is not a global setting. Its value is a list of rows; the PRO has
one row of 14 positions, the StompStation CORE one row of 5. Take the size from
the lengths: a row has `2C - 1` strings for `C` positions.

```
row = [ B0, L1, B1, L2, B2, ..., L13, B13 ]
B   = "" | "root\\app\\<block>"      an empty position or a block's node path
L   = "s" | "p"                       series or parallel, between B(c-1) and B(c)
```

Input and output are implicit, before `B0` and after `B13`; neither
`root\app\output` nor anything else stands for them. Every position is stereo.

A browse also returns `def` (the default chain) and `fixed`, a row of 27
booleans interleaved like the value (even indexes are positions, odd ones
connectors). On the PRO the amp (position 6), delay (11) and reverb (12) are
fixed; no connector is.

The default chain, which `root\app\output\d_chain` restores and which every
preset without a router record loads (all presets saved on 1.5.12), is the old
fixed order:

```json
[["root\\app\\gate","s","root\\app\\pitch","s","root\\app\\exp","s","root\\app\\comp","s","root\\app\\mod_pre","s","root\\app\\drive","s","root\\app\\amp","s","root\\app\\ir","s","root\\app\\eq","s","","s","root\\app\\mod","s","root\\app\\delay","s","root\\app\\reverb","s",""]]
```

`root\app\output\c_chain` empties every position that is not fixed and sets
every connector to `"s"`. Both actions run when written `{"value":"run"}` and
return to `idle`.

## Parallel

A run of positions joined by `"p"` is one parallel bank. Each position in it is
one branch of exactly one block; every branch receives the bank's input, and
their outputs are summed without gain compensation into the next position.
Delay ∥ reverb after the modulation block:

```json
[["root\\app\\gate","s","root\\app\\pitch","s","root\\app\\exp","s","root\\app\\comp","s","root\\app\\mod_pre","s","root\\app\\drive","s","root\\app\\amp","s","root\\app\\ir","s","root\\app\\eq","s","","s","root\\app\\mod","s","root\\app\\delay","p","root\\app\\reverb","s",""]]
```

In series a switched-off block bypasses itself, as on 1.5.12. Inside a bank the
firmware versions differ:

How a bank sounds was read from the firmware, not measured *(unconfirmed)*:

| Branch in a bank | 2.0.10 | 2.2.6 |
|---|---|---|
| block on | the block's output | the block's output |
| block off | nothing | the block's tail only |
| empty position | nothing | the dry input |
| no branch contributes | the dry input passes | silence |

## Blocks

Placeable blocks are the children of `root\app` whose `item_type` is `block`
and whose first child is `on_off`. Their style string carries `cat:`, `color:`
and `img:`. On 2.0.10: gate, pitch, exp, comp, mod_pre, flanger, chorus, drive,
amp, ir, eq, eq2, crossover, mod, delay, delay2, reverb, reverb2, pickup. 2.2.6
adds pitch_time, dyna_comp, parametric_eq, dtchr and rotary; its
`root\app\pcm42` node is never connected and must not be offered. A block's
parameters keep their 1.5.12 meanings. There are no mixer or split nodes: a
bank's balance comes from each block's own mix and level controls. The
crossover is an ordinary single-position block.

Blocks outside the chain stay live and writable but are silent, are not saved,
and reset to their defaults when a preset loads.

## Writing

Write the whole value as a raw JSON array, with nothing else in the object:

```
write root\app\router:{"value":[[...27 strings...]]}
```

The pedal applies these rules, in order:

1. A value that is not exactly one row of 27 JSON strings changes nothing.
2. Connectors are lower-cased; anything but `"p"` becomes `"s"`.
3. Fixed positions keep their current block whatever the write says.
4. Block strings are not validated: an unknown path is stored and acts as an
   empty position.
5. A block written twice keeps the later position; a copy of a fixed block is
   always the one dropped.
6. A write that changes nothing does nothing; any change applies live (a
   short dip in the output) and marks the preset edited.

The reply echoes the request exactly, whether it was applied, normalised or
ignored, so read `root\app\router` afterwards and treat that as the result.

Saving a preset stores the router as its first record,
`root\app\router:{"value":[[...]]}`, and drops the parameters of every block
not in the chain. A preset document must keep that record on export and import.

## Global quick controls

`root\assign1` to `root\assign4` (F1 to F4) are global, not per preset, and on
2.x refer to positions such as `0_2` rather than block paths, so they follow a
position when blocks move *(unconfirmed)*.
