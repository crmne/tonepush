/* Synthetic demo content. Preset, tone, setlist and capture names are
   invented; HX model names and knob names are as HX Edit's catalog lists
   them; StompStation PRO block names are the firmware 2.2.6 schema's. */
(function () {
  const D = {};
  const letters = 'ABC';

  /* ---------- HX Stomp: 126 slots, 42 banks of three ---------- */
  const hxNames = ['Glass Clean', 'Plexi Crunch', 'Brown Lead', 'Edge of Breakup', 'Ambient Swell', 'Slapback Twang',
    'Doom Fuzz', 'Worship Pad', 'Funk Rhythm', 'Velvet Lead', 'Tape Echo Clean', 'Octave Fuzz', 'Shimmer Pad',
    'Chime Clean', 'Smooth Overdrive', 'Surf Spring', 'Tremolo Clean', 'Desert Rock', 'Jazz Box', 'Bass DI',
    'Acoustic Sim', 'Wall of Fuzz', 'Clean Comp', 'Big Room Lead', 'Vibe Rhythm', 'Dotted Eighths', 'Crunch Stack',
    'Soft Swell', 'Garage Grit', 'Lead Boost', 'Tight Rhythm', 'Sparkle Verb', 'Rotary Clean', 'Twang Machine',
    'Cave Drone', 'Blues Edge', 'Dream Pop', 'Grunge Crunch', 'Octave Lead', 'Reverse Swell', 'Practice Clean', 'Studio Rhythm'];
  D.hxSlot = (i) => String(Math.floor(i / 3) + 1).padStart(2, '0') + letters[i % 3];
  D.hxPresets = (opts = {}) => {
    const out = [];
    const libSame = new Set([0, 1, 2, 3, 4, 6, 7, 9, 12, 13, 15, 18, 21, 24, 27, 30]);
    const libDiff = new Set([10, 16]);
    const favs = new Set([0, 1, 9, 12]);
    for (let i = 0; i < 126; i++) {
      const name = hxNames[i];
      out.push({ i, slot: D.hxSlot(i), name: name || '', empty: !name, bank: i % 3 === 0 && i > 0,
        lib: libSame.has(i) ? 'same' : libDiff.has(i) ? 'differs' : null, fav: favs.has(i) });
    }
    return out;
  };

  /* ---------- StompStation PRO: 60 slots, 20 banks of three ---------- */
  const proNames = ['Clean Machine', 'Glass Wall', 'Crunch Room', 'Stereo Swirl', 'Lead Machine', 'Plexi Jump',
    'Modern Chug', 'Velvet Drive', 'Shimmer Lead', 'Twin Clean', 'Brit Crunch', 'Fuzz Wall', 'Detuned Dream',
    'Rotary Blues', 'Para Clean', 'Spring Surf', 'Slap Echo', 'Doom Room', 'Worship Swell', 'Funk Comp', 'Edge Lead',
    'Bass Grit', 'Practice', 'Studio DI'];
  D.proSlot = (i) => String(Math.floor(i / 3) + 1).padStart(2, '0') + letters[i % 3];
  D.proPresets = () => {
    const out = [];
    const libSame = new Set([0, 1, 2, 3, 4, 7, 9, 12, 13]);
    for (let i = 0; i < 60; i++) {
      const name = proNames[i];
      out.push({ i, slot: D.proSlot(i), name: name || '', empty: !name, bank: i % 3 === 0 && i > 0,
        lib: libSame.has(i) ? 'same' : null, fav: i === 7 || i === 3 });
    }
    return out;
  };

  /* ---------- the HX preset on screen: 01B Plexi Crunch ---------- */
  D.hxChain = () => ({
    kind: 'hx',
    input: { label: 'In', sub: 'Multi' }, output: { label: 'Out', sub: 'Main L/R' },
    items: [
      { t: 'block', b: { name: 'Teardrop 310', cat: 'wah', on: false, tr: [{ text: 'EXP 1' }] } },
      { t: 'block', b: { name: 'Minotaur', cat: 'dist', tr: [{ fs: 'FS1', led: 'ltorange' }] } },
      { t: 'block', b: { name: 'US Double Nrm', cat: 'amp', sel: true, tl: [{ icon: 'camera' }], tr: [{ fs: 'FS2', led: 'green' }] } },
      { t: 'split', lanes: [[{ name: '2x12 Silver Bell', cat: 'cab' }], [{ name: '1x12 Field Coil', cat: 'cab' }]], mergeStereo: true },
      { t: 'block', b: { name: 'Transistor Tape', cat: 'delay', on: false, stereo: true, tl: [{ icon: 'camera' }], tr: [{ fs: 'FS2', led: 'green' }] } },
      { t: 'block', b: { name: 'Plateaux', cat: 'reverb', stereo: true, tl: [{ icon: 'camera' }], tr: [{ fs: 'FS3', led: 'turquoise' }] } },
    ],
  });

  /* US Double Nrm, in snapshot 2: HX Edit's twelve knobs in the device's order. */
  D.amp = [
    { label: 'Drive', val: '4.5', frac: 0.45, def: 0.35, snap: true },
    { label: 'Bass', val: '5.5', frac: 0.55, def: 0.44 },
    { label: 'Mid', val: '6.0', frac: 0.60, def: 0.52 },
    { label: 'Treble', val: '6.5', frac: 0.65, def: 0.57 },
    { label: 'Presence', val: '4.0', frac: 0.40, def: 0.10 },
    { label: 'Ch Vol', val: '7.5', frac: 0.75, def: 0.85, chips: [{ fs: 'FS2', led: 'green' }] },
    { label: 'Master', val: '10.0', frac: 1.0, def: 1.0 },
    { label: 'Sag', val: '5.0', frac: 0.5, def: 0.5 },
    { label: 'Hum', val: '5.0', frac: 0.5, def: 0.5 },
    { label: 'Ripple', val: '5.0', frac: 0.5, def: 0.5 },
    { label: 'Bias', val: '6.0', frac: 0.6, def: 0.6 },
    { label: 'Bias X', val: '5.0', frac: 0.5, def: 0.5 },
  ];

  D.hxSnaps = () => ({
    snaps: [{ name: 'Verse' }, { name: 'Chorus', on: true }, { name: 'Solo' }],
    blocks: [
      { name: 'Teardrop 310', cat: 'wah', on: [false, false, false], note: 'EXP 1 engages it' },
      { name: 'Minotaur', cat: 'dist', on: [false, true, true] },
      { name: 'US Double Nrm', cat: 'amp', on: [true, true, true] },
      { name: '2x12 Silver Bell', cat: 'cab', on: [true, true, true] },
      { name: '1x12 Field Coil', cat: 'cab', on: [true, true, true] },
      { name: 'Transistor Tape', cat: 'delay', on: [false, false, true] },
      { name: 'Plateaux', cat: 'reverb', on: [true, true, true] },
    ],
    values: [
      { block: 'US Double Nrm', param: 'Drive', cat: 'amp', v: ['4.5', '4.5', '5.8'] },
      { block: 'Transistor Tape', param: 'Mix', cat: 'delay', v: ['20%', '20%', '28%'] },
      { block: 'Plateaux', param: 'Mix', cat: 'reverb', v: ['18%', '22%', '30%'] },
    ],
  });

  D.hxSwitches = () => [
    { t1: 'FS1 · Auto colour', t2: 'Minotaur', led: 'ltorange', mode: 'Toggles',
      carries: [{ cat: 'dist', block: 'Minotaur', what: 'On/Off' }], foot: 'Lit orange, the colour of what it carries' },
    { t1: 'FS2 · Green', t2: 'Lead', led: 'green', mode: 'Toggles', on: false,
      carries: [{ cat: 'delay', block: 'Transistor Tape', what: 'On/Off' }, { cat: 'amp', block: 'US Double Nrm', what: 'Ch Vol', range: '7.5 → 8.6' }], foot: 'Scribble strip reads LEAD' },
    { t1: 'FS3 · Turquoise', t2: 'Plateaux', led: 'turquoise', mode: 'Toggles',
      carries: [{ cat: 'reverb', block: 'Plateaux', what: 'On/Off' }], foot: 'Name left blank: the pedal shows Plateaux' },
    { kind: 'exp', t1: 'EXP 1 · heel to toe', t2: 'Teardrop 310', mode: 'Auto on',
      carries: [{ cat: 'wah', block: 'Teardrop 310', what: 'Position', range: '0% → 100%' }], foot: 'Turns the wah on when the pedal leaves the heel' },
    { kind: 'empty', t1: 'EXP 2', t2: 'Not assigned', emptyText: 'Right-click a knob and choose EXP 2' },
  ];

  /* HX Edit's Distortion › Mono shelf, with each model's controls and their
     defaults, as the catalog lists them. */
  D.distMono = [{"name":"Kinky Boost","p":[["Drive",0.55,"k"],["Boost",0,"sw"],["Bright",0,"sw"]]},{"name":"Deranged Master","p":[["Drive",0.77,"k"],["Bass",0.52,"k"],["Treble",0.65,"k"],["Level",0.91,"k"]]},{"name":"Minotaur","p":[["Gain",0.42,"k"],["Tone",0.53,"k"],["Level",0.6,"k"]]},{"name":"Teemah!","p":[["Gain",0.52,"k"],["Bass Cut",0.5,"k"],["Treble Cut",0.5,"k"],["Clipping",0,"k"],["Level",0.62,"k"]]},{"name":"Heir Apparent","p":[["Gain",0.5,"k"],["Tone",0.5,"k"],["Presence",0,"k"],["Clipping",0,"k"],["Gain Mod",1,"k"],["Level",0.62,"k"],["Voltage",0,"sw"]]},{"name":"Tone Sovereign","p":[["Gain 1",0.5,"k"],["Tone 1",0.5,"k"],["Presence 1",0,"k"],["Clipping 1",0.5,"k"],["Gain Mod 1",0,"k"],["Level 1",0.5,"k"],["Gain 2",0.5,"k"],["Tone 2",0.5,"k"],["Presence 2",0,"k"],["Clipping 2",0,"k"],["Gain Mod 2",1,"k"],["Level 2",0.6,"k"],["Voltage",0,"sw"]]},{"name":"Alpaca Rouge","p":[["Drive",0.34,"k"],["Hi Cut",0.5,"k"],["Volume",0.7,"k"]]},{"name":"Compulsive Drive","p":[["Gain",0.66,"k"],["Tone",0.5,"k"],["Peak Type",0,"sw"],["Version",0,"sw"],["Level",0.65,"k"]]},{"name":"Dhyana Drive","p":[["Gain",0.44,"k"],["Voice",0.56,"k"],["Tone",0.44,"k"],["Level",0.4,"k"]]},{"name":"Horizon Drive","p":[["Drive",0.25,"k"],["Attack",0.2,"k"],["Bright",0.37,"k"],["Gate",1,"k"],["Gate Range",0,"sw"],["Level",0.5,"k"]]},{"name":"Valve Driver","p":[["Gain",0.65,"k"],["Bass",0.55,"k"],["Treble",0.45,"k"],["Level",0.66,"k"]]},{"name":"Top Secret OD","p":[["Gain",0.65,"k"],["Level",0.83,"k"]]},{"name":"Prize Drive","p":[["Drive",0.42,"k"],["Spectrum",0.74,"k"],["Level",0.4,"k"],["Bass Cut",1,"sw"],["Voltage",0,"sw"]]},{"name":"Scream 808","p":[["Gain",0.52,"k"],["Tone",0.65,"k"],["Level",0.67,"k"]]},{"name":"Pillars","p":[["Gain",0.47,"k"],["Tone",0.62,"k"],["Level",0.2,"k"],["Mode",0,"k"]]},{"name":"Hedgehog D9","p":[["Gain",0.7,"k"],["Tone",0.27,"k"],["Level",0.68,"k"]]},{"name":"Stupor OD","p":[["Drive",0.62,"k"],["Tone",0.5,"k"],["Level",0.5,"k"]]},{"name":"Deez One Vintage","p":[["Drive",0.8,"k"],["Tone",0.5,"k"],["Level",0.75,"k"]]},{"name":"Deez One Mod","p":[["Drive",0.63,"k"],["Tone",0.62,"k"],["Level",0.25,"k"],["Clipping",1,"sw"]]},{"name":"Ratatouille Dist","p":[["Gain",0.74,"k"],["Filter",0.28,"k"],["Level",0.7,"k"]]},{"name":"Vermin Dist","p":[["Gain",0.68,"k"],["Filter",0.6,"k"],["Level",0.85,"k"]]},{"name":"Vital Dist","p":[["Gain",1,"k"],["Filter",0,"k"],["Level",0.5,"k"],["Clipping",0,"k"],["Octave",0.4,"k"]]},{"name":"Vital Boost","p":[["Boost",0.5,"k"]]},{"name":"KWB","p":[["Gain",0.67,"k"],["Push Diode",0.33,"k"],["Pull Diode",1,"k"],["Bass",0.5,"k"],["Treble",0.5,"k"],["Level",0.69,"k"],["Asymmetry",0,"k"]]},{"name":"Legendary Drive","p":[["Drive",0.77,"k"],["Bass",0.54,"k"],["Middle",0.5,"k"],["Treble",0.5,"k"],["Presence",0.55,"k"],["Volume",0.5,"k"]]},{"name":"Swedish Chainsaw","p":[["Drive",0.62,"k"],["Bass",0.5,"k"],["Treble",0.7,"k"],["Level",0.6,"k"]]},{"name":"Arbitrator Fuzz","p":[["Fuzz",0.9,"k"],["Level",0.55,"k"]]},{"name":"Pocket Fuzz","p":[["Drive",0.49,"k"],["Level",0.45,"k"]]},{"name":"Bighorn Fuzz","p":[["Sustain",0.5,"k"],["Tone",0.5,"k"],["Level",0.5,"k"]]},{"name":"Triangle Fuzz","p":[["Sustain",0.83,"k"],["Tone",0.56,"k"],["Level",0.81,"k"]]}];

  /* ---------- the PRO preset on screen: 03B Velvet Drive on 2.2.6 ---------- */
  D.proChain = () => ({
    kind: 'pro',
    input: { label: 'In', sub: '1 + 2' }, output: { label: 'Out', sub: '1 + 2' },
    items: [
      { t: 'block', b: { name: 'Gate', sub: 'Noise gate', cat: 'dyn', lock: true } },
      { t: 'block', b: { name: 'Dynamic Comp', short: 'Comp', sub: 'Fast', cat: 'dyn' } },
      { t: 'block', b: { name: 'Wah', sub: 'Expression', cat: 'wah', on: false, tr: [{ text: 'CTRL' }] } },
      { t: 'block', b: { name: 'Drive', sub: 'TS808 · Drive 9', cat: 'dist', stereo: false, tr: [{ text: 'F3' }] } },
      { t: 'empty', slot: 4 }, { t: 'empty', slot: 5 }, { t: 'empty', slot: 6 },
      { t: 'block', b: { name: 'Amp', sub: 'JCM800 + AC30', cat: 'amp', lock: true, sel: true, stereo: true, tr: [{ text: 'F1' }] } },
      { t: 'block', b: { name: 'IR', sub: 'V30 + Greenback', cat: 'ir' } },
      { t: 'block', b: { name: 'EQ', sub: 'Parametric', cat: 'eq' } },
      { t: 'par', lanes: [[{ name: 'Detune Chorus', short: 'Detune', sub: 'Wide', cat: 'mod' }], [{ name: 'Rotary', sub: 'Slow', cat: 'mod', on: false }]] },
      { t: 'empty', slot: 12 },
      { t: 'block', b: { name: 'Delay', sub: 'Tape · 410 ms', cat: 'delay', lock: true, tr: [{ text: 'F2' }] } },
      { t: 'block', b: { name: 'Reverb', sub: 'Shimmer', cat: 'reverb', lock: true, tr: [{ text: 'F4' }] } },
      { t: 'empty', slot: 15 },
    ],
  });

  /* ---------- library ---------- */
  D.tones = [
    { name: 'Plexi Crunch', dev: 'HX Stomp', chain: ['wah!', 'dist', 'amp', ['cab', 'cab'], 'delay!', 'reverb'], song: 'Original', artist: '', char: 'Drive', rating: 4, added: '12 Sep', ver: 'v2', pedal: '01B', sync: 'same' },
    { name: 'Glass Clean', dev: 'HX Stomp', chain: ['dyn', 'amp', 'cab', 'mod', 'delay', 'reverb'], song: 'Harbour Lights', artist: 'The Night Signals', char: 'Clean', rating: 4, added: '10 Sep', ver: 'v1', pedal: '01A', sync: 'same' },
    { name: 'Brown Lead', dev: 'HX Stomp', chain: ['dist', 'amp', 'cab', 'delay', 'reverb'], song: 'Static Bloom', artist: 'June Arcade', char: 'Hi-gain', rating: 4, added: '11 Sep', ver: 'v3', pedal: '01C', sync: 'same' },
    { name: 'Ambient Swell', dev: 'HX Stomp', chain: ['vol', 'mod', 'delay', 'delay', 'reverb'], song: 'Original', artist: '', char: 'Clean', rating: 3, added: '10 Sep', ver: 'v1', pedal: '02B', sync: 'same' },
    { name: 'Velvet Drive', dev: 'StompStation PRO', chain: ['dyn', 'dyn', 'wah!', 'dist', 'amp', 'ir', 'eq', ['mod', 'mod!'], 'delay', 'reverb'], song: 'Low Tide', artist: 'Marlow Kent', char: 'Drive', rating: 5, added: '28 Sep', ver: 'v4', pedal: null, sync: null },
    { name: 'Edge of Breakup', dev: 'HX Stomp', chain: ['dyn', 'dist', 'amp', 'cab', 'reverb'], song: 'Original', artist: '', char: 'Drive', rating: 4, added: '14 Sep', ver: 'v1', pedal: '02A', sync: 'same' },
    { name: 'Doom Fuzz', dev: 'HX Stomp', chain: ['dist', 'dist', 'amp', 'cab', 'reverb!'], song: 'Iron Valley', artist: 'Slow Comet', char: 'Fuzz', rating: 3, added: '15 Sep', ver: 'v1', pedal: '03A', sync: 'same' },
    { name: 'Worship Pad', dev: 'HX Stomp', chain: ['vol', 'pitch', 'delay', 'reverb', 'reverb'], song: 'Original', artist: '', char: 'Clean', rating: 5, added: '16 Sep', ver: 'v2', pedal: '03B', sync: 'differs' },
    { name: 'Glass Wall', dev: 'StompStation PRO', chain: ['dyn', 'amp', 'ir', 'eq', 'mod', 'delay', 'reverb'], song: 'Original', artist: '', char: 'Clean', rating: 4, added: '29 Sep', ver: 'v1', pedal: null, sync: null },
    { name: 'Slapback Twang', dev: 'HX Stomp', chain: ['dyn', 'amp', 'cab', 'delay', 'reverb'], song: 'Dust Road', artist: 'The Night Signals', char: 'Clean', rating: 0, added: '17 Sep', ver: 'v1', pedal: null, sync: null },
    { name: 'Funk Rhythm', dev: 'HX Stomp', chain: ['dyn', 'filter', 'amp', 'cab', 'reverb'], song: 'Original', artist: '', char: 'Clean', rating: 3, added: '18 Sep', ver: 'v1', pedal: '03C', sync: 'same' },
    { name: 'Tape Echo Clean', dev: 'HX Stomp', chain: ['amp', 'cab', 'delay', 'reverb'], song: 'Paper Boats', artist: 'June Arcade', char: 'Clean', rating: 4, added: '19 Sep', ver: 'v2', pedal: '04B', sync: 'differs' },
    { name: 'Shimmer Lead', dev: 'StompStation PRO', chain: ['dyn', 'dist', 'amp', 'ir', 'delay', 'reverb'], song: 'Low Tide', artist: 'Marlow Kent', char: 'Hi-gain', rating: 4, added: '29 Sep', ver: 'v2', pedal: null, sync: null },
    { name: 'Surf Spring', dev: 'HX Stomp', chain: ['amp', 'cab', 'mod', 'reverb'], song: 'Original', artist: '', char: 'Clean', rating: 3, added: '20 Sep', ver: 'v1', pedal: '06A', sync: 'same' },
    { name: 'Octave Fuzz', dev: 'HX Stomp', chain: ['pitch', 'dist', 'amp', 'cab'], song: 'Original', artist: '', char: 'Fuzz', rating: 2, added: '21 Sep', ver: 'v1', pedal: '04C', sync: 'same' },
    { name: 'Dream Pop', dev: 'HX Stomp', chain: ['mod', 'amp', 'cab', 'delay', 'reverb'], song: 'Glasshouse', artist: 'Slow Comet', char: 'Clean', rating: 4, added: '23 Sep', ver: 'v1', pedal: '13A', sync: 'same' },
    { name: 'Garage Grit', dev: 'HX Stomp', chain: ['dist', 'amp', 'cab'], song: 'Original', artist: '', char: 'Drive', rating: 3, added: '24 Sep', ver: 'v1', pedal: null, sync: null },
  ];

  D.setlists = [
    { name: 'Album release show', venue: 'Lido Rooftop, Berlin', date: '4 Oct 2026', dev: 'HX Stomp', count: 42, ver: 'v2', state: '6 slots differ' },
    { name: 'Rehearsal', venue: 'Room 3', date: '28 Sep 2026', dev: 'HX Stomp', count: 38, ver: 'v1', state: 'matches the pedal' },
    { name: 'Studio session', venue: 'Kranhaus Studio', date: '12 Sep 2026', dev: 'HX Stomp', count: 24, ver: 'v1', state: '19 slots differ' },
    { name: 'Summer tour', venue: 'Various', date: '2 Aug 2026', dev: 'StompStation PRO', count: 21, ver: 'v3', state: 'for another pedal' },
    { name: 'Acoustic night', venue: 'Café Wendel', date: '18 Jul 2026', dev: 'HX Stomp', count: 12, ver: 'v1', state: '30 slots differ' },
  ];

  /* ---------- PRO device libraries ---------- */
  D.nams = [
    { n: 1, name: 'JCM800 2203 · Crunch', gear: 'Marshall JCM800 2203', arch: 'Standard', used: 6 },
    { n: 2, name: 'AC30 TB · Top Boost', gear: 'Vox AC30 Top Boost', arch: 'Standard', used: 4 },
    { n: 3, name: 'Twin Reverb · Vibrato ch', gear: 'Fender Twin Reverb', arch: 'Lite', used: 3 },
    { n: 4, name: 'Plexi 1959 · Jumped', gear: 'Marshall 1959 SLP', arch: 'Standard', used: 2 },
    { n: 5, name: 'Recto Modern · Ch3', gear: 'Mesa Dual Rectifier', arch: 'Standard', used: 1 },
    { n: 6, name: 'Bassman 5F6A · Bright', gear: 'Fender Bassman 5F6A', arch: 'Lite', used: 2 },
    { n: 7, name: '5150 III · Blue', gear: 'EVH 5150 III', arch: 'Standard', used: 1 },
    { n: 8, name: 'ODS-style · Drive', gear: 'Overdrive Special clone', arch: 'Standard', used: 0 },
    { n: 9, name: 'DC30 · Ch1', gear: 'Matchless DC-30', arch: 'Feather', used: 1 },
    { n: 10, name: 'BE-100 · BE', gear: 'Friedman BE-100', arch: 'Standard', used: 2 },
    { n: 11, name: 'Rockerverb · Dirty', gear: 'Orange Rockerverb 50', arch: 'Lite', used: 0 },
    { n: 12, name: 'Shiva · Ch2', gear: 'Bogner Shiva', arch: 'Standard', used: 1 },
    { n: 13, name: 'Mark IIC+ · Lead', gear: 'Mesa Mark IIC+', arch: 'Standard', used: 1 },
    { n: 14, name: 'SLO-100 · OD', gear: 'Soldano SLO-100', arch: 'Standard', used: 0 },
  ];
  D.irs = [
    { n: 1, name: 'V30 · SM57 cap edge', ch: 'mono', len: '200 ms', used: 7 },
    { n: 2, name: 'Greenback · R121', ch: 'mono', len: '200 ms', used: 4 },
    { n: 3, name: 'Oxford Room', ch: 'L', len: '500 ms', used: 2, pair: 'start' },
    { n: 4, name: 'Oxford Room', ch: 'R', len: '500 ms', used: 2, pair: 'end' },
    { n: 5, name: 'Alnico Blue · 57 + 121', ch: 'mono', len: '200 ms', used: 3 },
    { n: 6, name: 'Jensen C10Q · SM57', ch: 'mono', len: '170 ms', used: 1 },
    { n: 7, name: 'Studio Wide', ch: 'L', len: '320 ms', used: 1, pair: 'start' },
    { n: 8, name: 'Studio Wide', ch: 'R', len: '320 ms', used: 1, pair: 'end' },
    { n: 9, name: 'Bass 8x10 · D112', ch: 'mono', len: '200 ms', used: 1 },
    { n: 10, name: 'Celestion G12M · 414', ch: 'mono', len: '200 ms', used: 0 },
    { n: 11, name: 'Acoustic body · DI blend', ch: 'mono', len: '400 ms', used: 1 },
  ];
  window.D = D;
})();
