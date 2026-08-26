# Category icons

One per HX browse category, drawn for TonePush.

**These are ours.** Nothing here is traced from, derived from, or measured
against HX Edit's `icons_category` or `icons_models`, and none of Line 6's
artwork is in this repository or ever will be. That is the whole reason they
exist: the desktop app can ask a person to install HX Edit and read its files at
runtime, but a website cannot, and a tone browser that shows nothing at all is a
tone browser nobody looks at twice.

They are drawn in the same hand as `../ui`, which is Lucide's: a 24x24 box,
`fill="none"`, a 2px stroke, round caps and joins. Lucide has words for a
speaker and an arrow and not for a wah treadle, so the ones it has no word for
are ours, the same way `ui/pedal.svg` is.

`stroke="currentColor"`, so a caller sets the colour. The website already keeps
an accent per category in `app/helpers/tones_helper.rb`, picked to sit with its
own palette rather than with Line 6's, and these take that colour without
being touched.

Each is legible at 20px, which is the size a signal chain actually draws them.
That is what the shapes were chosen for: a compressor is two plates closing on a
signal rather than a picture of any particular compressor, and a filter is flat,
a corner, then nothing.

## What is missing

Per-*model* artwork - the picture of one specific pedal. There are about a
thousand of those, they are the bulk of what HX Edit ships (15 MB of its 17),
and drawing them is a different and much larger project. A category icon beside
a model's name carries most of the meaning at none of the cost.
