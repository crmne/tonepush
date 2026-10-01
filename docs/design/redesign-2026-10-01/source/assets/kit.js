/* TonePush redesign mockup kit. Every function returns an HTML string; the
   chain is laid out in code (positions, lanes, wires) the way egui's painter
   would draw it, so the geometry here is the specification. */
(function () {
  const TP = {};
  const W = () => window.innerWidth, H = () => window.innerHeight;
  TP.size = () => (W() <= 1100 ? 's' : W() >= 1800 ? 'l' : 'm');
  const z = (s, m, l) => ({ s, m, l })[TP.size()];
  TP.z = z;

  TP.setup = function (opts = {}) {
    const q = new URLSearchParams(location.search);
    const theme = q.get('theme') || opts.theme || 'dark';
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.size = TP.size();
    const root = document.documentElement.style;
    root.setProperty('--side', z(216, 232, 272) + 'px');
    root.setProperty('--deck', z(56, 64, 72) + 'px');
  };

  /* ---------- icons ---------- */
  TP.I = (name, cls = '') => {
    const body = (window.ICONS || {})[name];
    if (!body) { console.error('missing icon ' + name); return ''; }
    return `<svg class="i ${cls}" viewBox="0 0 24 24">${body}</svg>`;
  };
  const CATFILE = { dist: 'distortion', dyn: 'dynamics', eq: 'eq', mod: 'modulation', delay: 'delay', reverb: 'reverb',
    pitch: 'pitch-synth', filter: 'filter', wah: 'wah', amp: 'amp', preamp: 'preamp', cab: 'cab', ir: 'ir', vol: 'volume-pan',
    send: 'send-return', looper: 'looper', io: 'input', ampcab: 'amp-cab', in: 'input', out: 'output', split: 'split', merge: 'merge' };
  TP.G = (cat, cls = '') => {
    const body = (window.CATS || {})[CATFILE[cat] || cat];
    if (!body) { console.error('missing glyph ' + cat); return ''; }
    return `<svg class="i g ${cls}" viewBox="0 0 24 24">${body}</svg>`;
  };
  TP.CATNAME = { dist: 'Distortion', dyn: 'Dynamics', eq: 'EQ', mod: 'Modulation', delay: 'Delay', reverb: 'Reverb', pitch: 'Pitch/Synth',
    filter: 'Filter', wah: 'Wah', amp: 'Amp', preamp: 'Preamp', cab: 'Cab', ir: 'IR', vol: 'Volume/Pan', send: 'Send/Return', looper: 'Looper', io: 'Input' };
  TP.CATSHORT = { dist: 'Dist', dyn: 'Dyn', eq: 'EQ', mod: 'Mod', delay: 'Delay', reverb: 'Reverb', pitch: 'Pitch', filter: 'Filter',
    wah: 'Wah', amp: 'Amp', preamp: 'Preamp', cab: 'Cab', ir: 'IR', vol: 'Volume', send: 'Send', looper: 'Looper', io: 'I/O' };
  TP.LED = { white: 'var(--led-white)', red: 'var(--led-red)', dkorange: 'var(--led-dkorange)', ltorange: 'var(--led-ltorange)',
    yellow: 'var(--led-yellow)', green: 'var(--led-green)', turquoise: 'var(--led-turquoise)', blue: 'var(--led-blue)',
    violet: 'var(--led-violet)', pink: 'var(--led-pink)', off: 'var(--led-off)' };

  /* ---------- small pieces ---------- */
  TP.tag = (c) => {
    if (c.fs) return `<span class="tag"><span class="led" style="--led:${TP.LED[c.led] || c.led}"></span>${c.fs}</span>`;
    if (c.snap) return `<span class="tag" title="snapshot">${TP.I('history')}</span>`;
    if (c.icon) return `<span class="tag">${TP.I(c.icon)}${c.text || ''}</span>`;
    return `<span class="tag">${c.text}</span>`;
  };
  TP.stars = (n) => [1, 2, 3, 4, 5].map(i => `<svg class="${i <= n ? '' : 'e'}" viewBox="0 0 24 24">${ICONS.star}</svg>`).join('');

  /* ---------- knob ---------- */
  function polar(c, r, a) { return [c + r * Math.cos(a), c + r * Math.sin(a)]; }
  function arc(c, r, a0, a1) {
    const [x0, y0] = polar(c, r, a0), [x1, y1] = polar(c, r, a1);
    const large = a1 - a0 > Math.PI ? 1 : 0;
    return `M${x0.toFixed(2)} ${y0.toFixed(2)} A${r} ${r} 0 ${large} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
  }
  let gid = 0;
  TP.knobSvg = function (frac, S, def, opts = {}) {
    const c = S / 2, r = S / 2 - 2.5, A0 = Math.PI * 0.75, SW = Math.PI * 1.5;
    const a = A0 + SW * Math.max(0, Math.min(1, frac));
    const rf = r - 7.5;
    const id = 'kf' + (++gid);
    const light = document.documentElement.dataset.theme === 'light';
    const top = light ? '#ffffff' : '#363b45', bot = light ? '#e6e6e2' : '#1d2026', edge = light ? '#c4c6c1' : '#474d58';
    let s = `<svg class="knob" width="${S}" height="${S}" viewBox="0 0 ${S} ${S}">`;
    s += `<defs><linearGradient id="${id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${top}"/><stop offset="1" stop-color="${bot}"/></linearGradient></defs>`;
    s += `<path d="${arc(c, r, A0, A0 + SW)}" stroke="var(--line-strong)" stroke-width="3" fill="none" stroke-linecap="round"/>`;
    if (frac > 0.002) s += `<path d="${arc(c, r, A0, a)}" stroke="var(--cat, var(--accent))" stroke-width="3" fill="none" stroke-linecap="round"/>`;
    if (def != null) {
      const ad = A0 + SW * def; const [x0, y0] = polar(c, r + 3, ad), [x1, y1] = polar(c, r + 6, ad);
      s += `<line x1="${x0.toFixed(2)}" y1="${y0.toFixed(2)}" x2="${x1.toFixed(2)}" y2="${y1.toFixed(2)}" stroke="var(--faint)" stroke-width="1.4" stroke-linecap="round"/>`;
    }
    s += `<circle cx="${c}" cy="${c + 1.5}" r="${rf + 0.5}" fill="rgba(0,0,0,${light ? 0.10 : 0.45})"/>`;
    s += `<circle cx="${c}" cy="${c}" r="${rf}" fill="url(#${id})" stroke="${edge}" stroke-width="1"/>`;
    const [p0x, p0y] = polar(c, rf * 0.22, a), [p1x, p1y] = polar(c, rf * 0.8, a);
    s += `<line x1="${p0x.toFixed(2)}" y1="${p0y.toFixed(2)}" x2="${p1x.toFixed(2)}" y2="${p1y.toFixed(2)}" stroke="var(--text)" stroke-width="2.2" stroke-linecap="round"/>`;
    return s + '</svg>';
  };

  /* The block's on/off, drawn as the footswitch it is: a stomp button inside
     the pedal's LED ring, lit in the colour the pedal lights it. */
  TP.switchSvg = function (on, led, S) {
    const c = S / 2, r = S / 2 - 3;
    const light = document.documentElement.dataset.theme === 'light';
    const id = 'sw' + (++gid);
    const top = light ? '#ffffff' : '#41464f', bot = light ? '#dcdcd8' : '#22262c';
    let s = `<svg class="knob" width="${S}" height="${S}" viewBox="0 0 ${S} ${S}" style="overflow:visible">`;
    s += `<defs><radialGradient id="${id}" cx="50%" cy="38%" r="65%"><stop offset="0" stop-color="${top}"/><stop offset="1" stop-color="${bot}"/></radialGradient></defs>`;
    if (on) {
      s += `<circle cx="${c}" cy="${c}" r="${r + 1}" fill="none" stroke="${led}" stroke-opacity="0.22" stroke-width="7"/>`;
      s += `<circle cx="${c}" cy="${c}" r="${r}" fill="none" stroke="${led}" stroke-width="2.6"/>`;
    } else {
      s += `<circle cx="${c}" cy="${c}" r="${r}" fill="none" stroke="var(--faint)" stroke-width="1.6" stroke-dasharray="3 3"/>`;
    }
    s += `<circle cx="${c}" cy="${c}" r="${r - 6}" fill="var(--bg-deep)" stroke="var(--line-strong)" stroke-width="1"/>`;
    s += `<circle cx="${c}" cy="${c}" r="${r - 10}" fill="url(#${id})" stroke="${light ? '#b9bbb6' : '#4c525d'}" stroke-width="1"/>`;
    return s + '</svg>';
  };

  /* ---------- faceplate ---------- */
  /* Knob size follows the room: the largest of 68, 60, 52 or 44 pt whose
     balanced rows fit the pane, so a twelve-knob amp fills two rows of large
     knobs where a three-knob drive gets one. Cell = knob + 26 pt. */
  let K = 52;
  TP.cellW = () => K + 26;
  TP.knobS = () => K;
  TP.cell = function (k) {
    const cw = k.w || TP.cellW();
    if (k.t === 'fsw') {
      return `<div class="cell ${k.chips && k.chips.length ? 'assigned' : ''}" style="--cell:${cw}px">${TP.switchSvg(k.on, k.led || 'var(--accent)', TP.knobS())}
        <div class="kv" style="color:${k.on ? 'var(--text)' : 'var(--muted)'}">${k.on ? 'On' : 'Off'}</div><div class="kl">${k.label || 'On/Off'}</div>
        <div class="kc">${(k.chips || []).map(TP.tag).join('')}</div></div>`;
    }
    if (k.t === 'menu') {
      return `<div class="cell" style="--cell:${cw}px"><div style="height:${TP.knobS()}px;display:flex;align-items:center"><span class="btn sm" style="min-width:${Math.min(cw - 6, 96)}px;justify-content:space-between;padding:0 7px;font-weight:500">${k.val}${TP.I('chevron-down', 's12')}</span></div>
        <div class="kv" style="visibility:hidden">.</div><div class="kl">${k.label}</div><div class="kc">${(k.chips || []).map(TP.tag).join('')}</div></div>`;
    }
    if (k.t === 'toggle') {
      const S = TP.knobS();
      return `<div class="cell" style="--cell:${cw}px"><div style="height:${S}px;display:flex;align-items:center"><span style="width:34px;height:20px;border-radius:10px;background:${k.on ? 'var(--cat, var(--accent))' : 'var(--raised)'};border:1px solid ${k.on ? 'transparent' : 'var(--line-strong)'};position:relative;display:inline-block"><span style="position:absolute;top:2px;${k.on ? 'right:2px' : 'left:2px'};width:14px;height:14px;border-radius:50%;background:${k.on ? '#fff' : 'var(--muted)'}"></span></span></div>
        <div class="kv">${k.val || (k.on ? 'On' : 'Off')}</div><div class="kl">${k.label}</div><div class="kc"></div></div>`;
    }
    if (k.t === 'html') return `<div class="cell" style="--cell:${k.w}px;height:${TP.knobS() + 56}px;align-items:stretch;text-align:left">${typeof k.html === 'function' ? k.html() : k.html}</div>`;
    const chips = (k.chips || []).slice();
    if (k.snap) chips.unshift({ icon: 'camera' });
    return `<div class="cell ${chips.length ? 'assigned' : ''}" style="--cell:${cw}px">${TP.knobSvg(k.frac, TP.knobS(), k.def)}
      <div class="kv">${k.val}</div><div class="kl">${k.label}</div><div class="kc">${chips.map(TP.tag).join('')}</div></div>`;
  };
  function layoutRows(cells, width) {
    const cw = TP.cellW();
    const widthOf = (c) => c.t === 'div' ? 17 : (c.w || cw);
    const total = cells.reduce((a, c) => a + widthOf(c), 0) + 20;
    if (total <= width) return [cells];
    const count = cells.filter(c => c.t !== 'div').length;
    const per = Math.max(1, Math.floor((width - 37) / cw));
    const nrows = Math.ceil(count / per);
    const each = Math.ceil(count / nrows);
    const rows = []; let cur = [], n = 0;
    for (const c of cells) {
      if (c.t === 'div') { if (cur.length) cur.push(c); continue; }
      if (n === each) { rows.push(cur); cur = []; n = 0; }
      cur.push(c); n++;
    }
    if (cur.length) rows.push(cur);
    return rows;
  }
  TP.face = function (cells, width, opts = {}) {
    const sizes = opts.sizes || [68, 60, 52, 44];
    // The on/off switch owns a column spanning every row, so the knob
    // columns to its right line up however many rows they wrap into.
    const lead = cells[0] && cells[0].t === 'fsw' ? cells[0] : null;
    const rest = lead ? cells.slice(cells[1] && cells[1].t === 'div' ? 2 : 1) : cells;
    let rows;
    for (const k of sizes) {
      K = k;
      const leadW = lead ? TP.cellW() + 17 : 0;
      rows = layoutRows(rest, width - leadW);
      const h = rows.length * (K + 66) + 24;
      if (!opts.height || h <= opts.height) break;
    }
    const rowHtml = rows.map(r => `<div style="display:flex;align-items:flex-start">${r.map(c => c.t === 'div'
      ? `<div style="width:1px;align-self:stretch;margin:4px 8px 24px;background:var(--line)"></div>` : TP.cell(c)).join('')}</div>`).join('');
    const leadHtml = lead ? `${TP.cell(lead)}<div style="width:1px;align-self:stretch;margin:4px 8px 24px;background:var(--line)"></div>` : '';
    return `<div class="face" style="${opts.style || ''}">${opts.head || ''}<div style="display:flex;align-items:flex-start">${leadHtml}<div style="display:flex;flex-direction:column;row-gap:8px">${rowHtml}</div></div></div>`;
  };

  /* ---------- snapshot matrix ---------- */
  /* Which blocks are on in each snapshot, and every value a snapshot sets.
     The block rows come from the preset document's snapshot sections
     (hx_proto Preset::snapshot_details); the value rows need the snapshot
     controller values decoded, which is stage 4 of the implementation order. */
  TP.snapMatrix = function (o) {
    const n = o.snaps.length;
    const cols = `minmax(0,1fr) repeat(${n}, ${o.colW || 92}px)`;
    let h = `<div class="mx" style="grid-template-columns:${cols}">`;
    h += `<div class="h" style="justify-content:flex-start;padding-left:14px"></div>` + o.snaps.map((s, i) => `<div class="h ${s.on ? 'on acol' : ''}"><span class="n num">${i + 1}</span>${s.name}</div>`).join('');
    if (o.sectionBlocks !== false) h += `<div class="sec2" style="margin-top:0;border-top:none">${o.blocksTitle || 'Blocks on and off'}</div>`;
    for (const r of o.blocks) {
      h += `<div class="rl cat-${r.cat}"><span class="g">${TP.G(r.cat)}</span><span class="ell">${r.name}</span>${r.note ? `<span class="pn">${r.note}</span>` : ''}</div>`;
      r.on.forEach((on, i) => { h += `<div class="v ${o.snaps[i].on ? 'acol' : ''} cat-${r.cat}"><span class="sled ${on ? '' : 'off'}"></span></div>`; });
    }
    if (o.values && o.values.length) {
      h += `<div class="sec2">${o.valuesTitle || 'Values that change'}</div>`;
      for (const r of o.values) {
        h += `<div class="rl cat-${r.cat}"><span class="g">${TP.G(r.cat)}</span><span class="ell">${r.block}</span><span class="pn">${r.param}</span></div>`;
        r.v.forEach((v, i) => { h += `<div class="v num ${o.snaps[i].on ? 'acol' : ''} ${v === '' ? 'dim' : ''}">${v}</div>`; });
      }
    }
    if (o.tempo) {
      h += `<div class="sec2">Tempo</div><div class="rl">${TP.I('timer', 's14')}<span>Follows the preset</span></div>`;
      o.tempo.forEach((v, i) => { h += `<div class="v num ${o.snaps[i].on ? 'acol' : ''}">${v}</div>`; });
    }
    return h + `</div>`;
  };

  /* ---------- sidebar ---------- */
  TP.sidebar = function (o) {
    const dev = o.device;
    let h = `<aside class="side">`;
    h += `<div class="devcard ${dev.online ? '' : 'off'}"><div class="well">${TP.I(dev.icon || 'tp-pedal', 's18')}</div>
      <div class="grow"><div class="nm ell">${dev.name}</div><div class="st"><span class="dot"></span>${dev.status}</div></div>
      <span class="chev">${TP.I('chevrons-up-down', 's14')}</span></div>`;
    const nav = o.nav || 'edit';
    h += `<div class="navseg"><div class="seg fill">${[['edit', 'Edit'], ['library', 'Library'], ['device', o.deviceTab || 'Pedal']]
      .map(([k, l]) => `<span class="s ${nav === k ? 'on' : ''}">${l}</span>`).join('')}</div></div>`;
    if (o.picking) h += `<div class="picking">${o.picking}</div>`;
    h += `<div class="sec"><span class="t">${o.listTitle || 'Presets'}</span><span class="aside">${o.listAside || ''}</span>
      <span class="iconbtn sm" title="favourites">${TP.I('star', 's14')}</span><span class="iconbtn sm" title="keep the whole pedal as a setlist">${TP.I('tp-computer', 's14')}</span></div>`;
    h += `<div class="plist">${(o.presets || []).map(p => TP.prow(p, o)).join('')}<div class="fade"></div></div>`;
    if (o.foot !== false) h += `<div class="sfoot">${o.foot || ''}</div>`;
    return h + `</aside>` + (o.menu || '');
  };
  TP.prow = function (p, o) {
    const cls = ['prow'];
    if (p.bank) cls.push('bank');
    if (p.sel) cls.push('sel');
    if (p.empty) cls.push('empty');
    if (p.hov) cls.push('hov');
    if (p.target) cls.push('target', p.empty ? 'free' : 'used');
    if (p.drop) cls.push('drop');
    let marks = '';
    if (p.dirty) marks += `<span class="edited" title="edited"></span>`;
    if (p.fav) marks += `<span style="color:var(--accent);display:flex">${TP.I('star', 's12')}</span>`;
    if (p.lib === 'same') marks += `<span title="in your library" style="display:flex;color:var(--faint)">${TP.I('check', 's13')}</span>`;
    if (p.lib === 'differs') marks += `<span title="differs from your library" style="display:flex;color:var(--hot)">${TP.I('git-compare', 's13')}</span>`;
    if (p.hint) marks += `<span class="hint">${p.hint}</span>`;
    return `<div class="${cls.join(' ')}"><span class="sl num">${p.slot}</span><span class="nm">${p.empty ? (p.target ? 'Empty' : 'New Preset') : p.name}</span><span class="mk">${marks}</span></div>`;
  };

  /* ---------- deck ---------- */
  TP.deck = function (o) {
    let h = `<header class="deck">`;
    if (o.sideToggle) h += `<span class="iconbtn" style="margin-left:-8px">${TP.I('panel-left')}</span>`;
    h += `<span class="slotchip num">${o.slot}</span>`;
    h += `<div class="ptitlewrap"><div class="ptitle">${o.name}</div><div class="pmeta">${o.meta || ''}</div></div>`;
    h += `<div class="grow"></div>`;
    if (o.snaps) h += `<div class="seg snapseg">${o.snaps.map((s, i) => `<span class="s ${s.on ? 'on' : ''}"><span class="n num">${i + 1}</span>${TP.size() === 's' && !s.on ? '' : s.name}</span>`).join('')}</div>`;
    if (o.tempo) h += `<div class="vsep"></div><div class="tempo"><span class="v num">${o.tempo}</span><span class="u">BPM</span></div><span class="btn sm">Tap</span>`;
    h += `<div class="vsep"></div>`;
    h += `<span class="iconbtn ${o.undo ? '' : 'dis'}">${TP.I('undo-2')}</span><span class="iconbtn ${o.redo ? '' : 'dis'}">${TP.I('redo-2')}</span>`;
    h += o.save || (o.dirty ? `<span class="btn primary">Save${TP.size() === 's' ? '' : '<span class="kbd">Ctrl S</span>'}</span>` : `<span class="btn disabled">${TP.I('check')}Saved</span>`);
    return h + `</header>`;
  };
  TP.miniChain = function (seq) {
    return `<div class="minichain">${seq.map((x, i) => {
      const w = i ? '<span class="w"></span>' : '';
      if (Array.isArray(x)) return w + `<span class="stack">${x.map(c => `<span class="m cat-${c.replace('!', '')} ${c.endsWith('!') ? 'off' : ''}"></span>`).join('')}</span>`;
      return w + `<span class="m cat-${x.replace('!', '')} ${x.endsWith('!') ? 'off' : ''}"></span>`;
    }).join('')}</div>`;
  };
  TP.minideck = function (o) {
    return `<header class="minideck"><span class="slotchip num">${o.slot}</span><span class="nm">${o.name}</span>
      ${o.dirty ? `<span class="chip hot"><span class="dot"></span>Edited</span>` : ''}
      <div style="margin-left:6px">${TP.miniChain(o.chain)}</div>
      ${o.snap ? `<span class="chip ghost" style="margin-left:4px">${TP.I('history', 's12')}${o.snap}</span>` : ''}
      <div class="grow"></div>${o.right || ''}
      ${o.dirty ? `<span class="btn primary sm">Save</span>` : ''}<span class="btn sm ghost">${TP.I('arrow-up-right')}Edit</span></header>`;
  };

  /* ---------- chain ---------- */
  /* Geometry, in points:
     tile width clamps between the size's min and max to fill the board,
     wires are fixed, a fork or merge takes a junction's width, and lanes
     stack under the main line at tile height plus a 10 pt gap. */
  TP.chainGeom = function (model) {
    const pro = model.kind === 'pro';
    return {
      padL: 22, padR: 22, top: z(34, 40, 46), bottom: z(18, 22, 26),
      EW: 34, W: pro ? z(6, 6, 12) : z(18, 24, 30), J: pro ? z(20, 22, 30) : z(26, 30, 34), EMP: pro ? z(16, 16, 24) : 0,
      TH: z(84, 92, 128), LG: z(10, 10, 14), min: pro ? z(64, 68, 96) : z(72, 92, 136), max: pro ? z(88, 112, 150) : z(104, 124, 176),
    };
  };
  TP.renderChain = function (el, model) {
    const g = TP.chainGeom(model);
    const width = el.clientWidth;
    const avail = width - g.padL - g.padR;
    const items = model.items;
    // fixed width and tile units
    let fixed = 2 * g.EW, units = 0;
    const seq = [{ t: 'in' }, ...items, { t: 'out' }];
    for (let i = 0; i < seq.length; i++) {
      const it = seq[i];
      if (it.t === 'block') units += 1;
      if (it.t === 'empty') fixed += g.EMP;
      if (it.t === 'split' || it.t === 'par') {
        const longest = Math.max(...it.lanes.map(l => l.length));
        units += longest; fixed += (longest - 1) * g.W;
      }
      if (i > 0) {
        const a = seq[i - 1], b = it;
        const jn = (x) => x.t === 'split' || x.t === 'par';
        fixed += (jn(a) ? g.J : g.W) * 0 + (jn(a) || jn(b) ? g.J : g.W);
      }
    }
    let TW = (avail - fixed) / units;
    let overflow = false;
    if (TW > g.max) TW = g.max;
    if (TW < g.min) { TW = g.min; overflow = true; }
    TW = Math.floor(TW);
    const total = fixed + units * TW;
    let x = g.padL + Math.max(0, (avail - total) / (model.align === 'left' ? 1e9 : 2));
    const TH = g.TH, y0 = g.top, mainY = y0 + TH / 2;
    const maxLanes = Math.max(1, ...items.filter(i => i.lanes).map(i => i.lanes.length));
    const height = y0 + maxLanes * TH + (maxLanes - 1) * g.LG + g.bottom;
    el.style.height = height + 'px';
    const compact = TW < 90 || TP.size() === 's';
    let html = '', wires = [], dots = [], tags = [], notch = null;
    let stereo = !!(model.input && model.input.stereo);
    const wire = (pts, st) => wires.push({ pts, st });
    const tileHtml = (b, x, y) => {
      const cls = ['tile', 'cat-' + b.cat];
      if (b.sel) cls.push('sel');
      if (b.on === false) cls.push('off');
      if (b.hl) cls.push('hl');
      if (b.swap) cls.push('swap');
      if (compact) cls.push('compact');
      if (b.sel) notch = x + TW / 2;
      let inner = '';
      const tl = (b.tl || []).slice();
      if (b.lock) inner += `<span class="lock" title="fixed position">${TP.I('lock')}</span>`;
      else if (tl.length) inner += `<span class="tl">${tl.map(TP.tag).join('')}</span>`;
      const tr = (b.tr || []).slice();
      if (b.on === false) inner += `<span class="tr"><span class="offtag">OFF</span>${compact ? '' : tr.map(TP.tag).join('')}</span>`;
      else if (tr.length) inner += `<span class="tr">${(compact ? tr.map(c => c.fs ? `<span class="tag" style="padding:0 4px"><span class="led" style="--led:${TP.LED[c.led]}"></span></span>` : TP.tag(c)) : tr.map(TP.tag)).join('')}</span>`;
      inner += `<span class="ic">${TP.G(b.glyph || b.cat)}</span>`;
      inner += `<span class="nm">${TP.size() === "s" && b.short ? b.short : b.name}</span>`;
      if (b.sub) inner += `<span class="sub">${b.sub}</span>`;
      else inner += `<span class="ct">${b.ct || TP.CATSHORT[b.cat]}</span>`;
      const style = `left:${x}px;top:${y}px;width:${TW}px;height:${TH}px;${b.hlc ? '--hl:' + b.hlc : ''}`;
      return `<div class="${cls.join(' ')}" style="${style}">${inner}</div>`;
    };
    const stereoAfter = (b, s) => (b.stereo === true ? true : b.stereo === false ? false : s);
    // walk
    let prevRight = null, prevKind = null;
    for (let i = 0; i < seq.length; i++) {
      const it = seq[i];
      const jnHere = it.t === 'split' || it.t === 'par';
      if (i > 0) {
        const gap = (jnHere || prevKind === 'jn') ? g.J : g.W;
        if (!jnHere && prevKind !== 'jn') wire([[prevRight, mainY], [prevRight + gap, mainY]], stereo);
        x = prevRight + gap;
      }
      if (it.t === 'in' || it.t === 'out') {
        const ep = it.t === 'in' ? model.input : model.output;
        html += `<div class="ep" style="left:${x}px;top:${mainY - 15}px;width:${g.EW}px"><span class="jack">${TP.G(it.t === 'in' ? 'in' : 'out')}</span><span class="lb">${ep.label}</span>${ep.sub ? `<span class="lb2">${ep.sub}</span>` : ''}</div>`;
        if (it.t === 'in') { /* wire starts at the jack's edge */ }
        prevRight = x + g.EW / 2 + 15; if (it.t === 'in') prevRight = x + g.EW / 2 + 15;
        if (it.t === 'out') { /* fix the incoming wire to stop at the jack */ wires[wires.length - 1].pts[1][0] = x + g.EW / 2 - 15; }
        prevKind = 'ep';
        continue;
      }
      if (it.t === 'empty') {
        html += `<div class="slot-empty ${it.new ? 'new' : ''}" style="left:${x}px;top:${y0 + TH * 0.2}px;width:${g.EMP}px;height:${TH * 0.6}px" title="free slot ${it.slot}">${TP.I('plus')}</div>`;
        prevRight = x + g.EMP; prevKind = 'empty';
        continue;
      }
      if (it.t === 'block') {
        html += tileHtml(it.b, x, y0);
        stereo = stereoAfter(it.b, stereo);
        prevRight = x + TW; prevKind = 'block';
        continue;
      }
      if (jnHere) {
        // fork: the main line runs into a dot, curves drop to each lane
        const xa = x - g.J, forkX = xa + 5;
        wire([[xa, mainY], [forkX, mainY]], stereo);
        dots.push([forkX, mainY]);
        const longest = Math.max(...it.lanes.map(l => l.length));
        const itemW = longest * TW + (longest - 1) * g.W;
        let laneStereoOut = [];
        it.lanes.forEach((lane, k) => {
          const ly = y0 + k * (TH + g.LG) + TH / 2;
          const sIn = stereo;
          if (k === 0) wire([[forkX, mainY], [x, mainY]], sIn);
          else wire([[forkX, mainY], [forkX + (x - forkX) * 0.6, mainY], [x - (x - forkX) * 0.6, ly], [x, ly]], sIn, true);
          let lx = x, s = sIn;
          lane.forEach((b, j) => {
            if (j > 0) { wire([[lx, ly], [lx + g.W, ly]], s); lx += g.W; }
            html += tileHtml(b, lx, y0 + k * (TH + g.LG));
            s = stereoAfter(b, s); lx += TW;
          });
          if (lx < x + itemW) wire([[lx, ly], [x + itemW, ly]], s);
          laneStereoOut.push(s);
        });
        const xb = x + itemW, mergeX = xb + g.J - 5;
        const outStereo = it.mergeStereo != null ? it.mergeStereo : laneStereoOut.some(Boolean);
        it.lanes.forEach((lane, k) => {
          const ly = y0 + k * (TH + g.LG) + TH / 2;
          if (k === 0) wire([[xb, mainY], [mergeX, mainY]], laneStereoOut[0]);
          else wire([[xb, ly], [xb + (mergeX - xb) * 0.6, ly], [mergeX - (mergeX - xb) * 0.6, mainY], [mergeX, mainY]], laneStereoOut[k], true);
        });
        dots.push([mergeX, mainY]);
        wire([[mergeX, mainY], [xb + g.J, mainY]], outStereo);
        if (it.tag) tags.push([forkX, mainY + 12, it.tag]);
        if (it.mtag) tags.push([mergeX, mainY + 12, it.mtag]);
        stereo = outStereo;
        prevRight = xb + g.J; prevKind = 'jn';
        // the next gap is already consumed by the merge junction
        seq[i + 1] && (seq[i + 1]._afterJn = true);
        prevRight = xb; // gap J added at the next step
        continue;
      }
    }
    // wires to SVG
    const wireColor = 'var(--wire)';
    let svg = `<svg class="wires" width="${width}" height="${height}">`;
    for (const w of wires) {
      const d = w.pts.length === 4
        ? (dy) => `M${w.pts[0][0]} ${w.pts[0][1] + dy} C${w.pts[1][0]} ${w.pts[1][1] + dy} ${w.pts[2][0]} ${w.pts[2][1] + dy} ${w.pts[3][0]} ${w.pts[3][1] + dy}`
        : (dy) => `M${w.pts[0][0]} ${w.pts[0][1] + dy} L${w.pts[1][0]} ${w.pts[1][1] + dy}`;
      if (w.st) svg += `<path d="${d(-1.8)}" stroke="${wireColor}" stroke-width="1.5" fill="none"/><path d="${d(1.8)}" stroke="${wireColor}" stroke-width="1.5" fill="none"/>`;
      else svg += `<path d="${d(0)}" stroke="${wireColor}" stroke-width="2" fill="none" stroke-linecap="round"/>`;
    }
    for (const [dx, dy] of dots) svg += `<circle cx="${dx}" cy="${dy}" r="4.5" fill="var(--bg-deep)" stroke="${wireColor}" stroke-width="2"/>`;
    svg += `</svg>`;
    let extra = '';
    for (const [tx, ty, t] of tags) extra += `<span class="jtag" style="left:${tx}px;top:${ty}px">${t}</span>`;
    if (model.zones) for (const zz of model.zones) {
      // zones are given as fractions of slot positions resolved by the screen
      extra += zz.html || '';
    }
    let headHtml = model.head ? `<div class="bhead">${model.head}</div>` : '';
    if (notch != null && model.notch !== false) {
      extra += `<svg class="notch" style="left:${notch - 11}px" viewBox="0 0 22 11"><path d="M0 11 L11 0 L22 11 Z" fill="var(--bg)"/><path d="M0 11 L11 0 L22 11" fill="none" stroke="var(--line)" stroke-width="1"/></svg>`;
    }
    if (overflow) extra += `<div class="fadeR">${TP.I('chevron-right')}</div>`;
    el.innerHTML = headHtml + svg + html + extra;
    el._geom = { TW, TH, y0, mainY, g };
    return { TW, TH, overflow };
  };
  TP.legend = () => `<span class="legend"><svg width="20" height="8"><path d="M1 4 H19" stroke="var(--wire)" stroke-width="2" stroke-linecap="round"/></svg>mono</span>
    <span class="legend"><svg width="20" height="8"><path d="M1 2.2 H19 M1 5.8 H19" stroke="var(--wire)" stroke-width="1.5"/></svg>stereo</span>`;

  /* ---------- footswitch board ---------- */
  TP.fsCard = function (f) {
    const cls = ['fscard'];
    if (f.kind === 'exp') cls.push('exp');
    if (f.kind === 'empty') cls.push('empty');
    if (f.sel) cls.push('sel');
    if (f.on === false) cls.push('off');
    const sw = f.kind === 'exp' ? `<span class="sw">${TP.G('wah', 's18')}</span>` : `<span class="sw"></span>`;
    let h = `<div class="${cls.join(' ')}" style="--led:${TP.LED[f.led] || f.led || 'transparent'}"><div class="top">${sw}<div style="min-width:0;flex:1"><div class="t1">${f.t1}</div><div class="t2">${f.t2}</div></div>${f.mode ? `<span class="chip">${f.mode}</span>` : ''}</div>`;
    if (f.carries && f.carries.length) h += `<div>${f.carries.map(c => `<div class="cr cat-${c.cat}"><span class="g">${TP.G(c.cat)}</span><span class="ell">${c.block}</span><span class="muted">${c.what}</span><span class="rng num">${c.range || ''}</span></div>`).join('')}</div>`;
    else h += `<div class="cr" style="color:var(--faint)">${f.emptyText || 'Drag a block or a knob here'}</div>`;
    if (f.foot) h += `<div class="ft">${f.foot}</div>`;
    return h + `</div>`;
  };
  TP.fsBoard = (cards, cols) => `<div class="fsboard" style="grid-template-columns:repeat(${cols || cards.length}, minmax(0,1fr))">${cards.map(TP.fsCard).join('')}</div>`;

  TP.swRow = function (f) {
    const cls = ['swrow'];
    if (f.kind === 'exp') cls.push('exp');
    if (f.kind === 'empty') cls.push('empty');
    if (f.sel) cls.push('sel');
    if (f.on === false) cls.push('off');
    const sw = f.kind === 'exp' ? `<span class="sw">${TP.G('wah', 's14')}</span>` : f.kind === 'midi' ? `<span class="sw" style="border-radius:8px;background:var(--raised);display:flex;align-items:center;justify-content:center;color:var(--text-soft)">${TP.I('cable', 's14')}</span>` : `<span class="sw"></span>`;
    const carry = (f.carries || []).map(c => `<span class="c cat-${c.cat}"><span class="g">${TP.G(c.cat)}</span><b>${c.block}</b>${c.what}${c.range ? ` <span class="num">${c.range}</span>` : ''}</span>`).join('') || `<span class="none">${f.emptyText || ''}</span>`;
    return `<div class="${cls.join(' ')}" style="--led:${TP.LED[f.led] || f.led || 'transparent'}">${sw}<div class="id"><div class="t1">${f.t1}</div><div class="t2">${f.t2}</div></div><div class="carry">${carry}</div>${f.mode ? `<span class="chip">${f.mode}</span>` : ''}</div>`;
  };

  /* ---------- floor ---------- */
  TP.fsw = function (f) {
    if (f.kind === 'exp') return `<div class="fsw exp ${f.cls || ''} ${f.cat ? 'cat-' + f.cat : ''}"><span class="sw">${TP.G('wah', 's14')}</span><div class="tx"><div class="t1">${f.t1}</div><div class="t2">${f.t2}</div></div></div>`;
    if (f.kind === 'knob') return `<div class="fsw knobf ${f.cls || ''}"><span class="sw">${TP.knobSvg(f.frac, 26, null)}</span><div class="tx"><div class="t1">${f.t1}</div><div class="t2">${f.t2}</div></div></div>`;
    if (f.kind === 'empty') return `<div class="fsw empty ${f.cls || ''}"><span class="sw" style="--led:transparent"></span><div class="tx"><div class="t1">${f.t1}</div><div class="t2">${f.t2}</div></div></div>`;
    return `<div class="fsw ${f.on === false ? 'off' : ''} ${f.cls || ''}" style="--led:${TP.LED[f.led] || f.led}"><span class="sw"></span><div class="tx"><div class="t1">${f.t1}</div><div class="t2">${f.t2}</div></div></div>`;
  };
  TP.floor = (o) => `<footer class="floor">${o.label ? `<span class="lab">${o.label}</span>` : ''}${o.items.map(TP.fsw).join('')}<div class="grow"></div>${o.right || ''}</footer>`;

  /* ---------- menus and dialogs ---------- */
  TP.menu = function (o) {
    const rows = o.rows.map(r => {
      if (r === '-') return `<div class="msep"></div>`;
      if (r.head) return `<div class="mhead"><span class="ell">${r.head}</span><span class="h">${r.h || ''}</span></div>`;
      return `<div class="mrow ${r.cls || ''}">${r.icon ? TP.I(r.icon) : '<span style="width:15px"></span>'}<span>${r.t}</span>${r.h ? `<span class="h">${r.h}</span>` : ''}${r.sub ? `<span class="h">${TP.I('chevron-right', 's14')}</span>` : ''}</div>`;
    }).join('');
    return `<div class="menu" style="left:${o.x}px;top:${o.y}px;width:${o.w || 248}px">${rows}</div>`;
  };

  /* The TonePush mark, packaging/icons/tonepush.svg. */
  TP.logo = (size = 40) => `<svg width="${size}" height="${size}" viewBox="0 0 128 128"><rect x="0" y="0" width="128" height="128" rx="28" fill="#121418"/><path d="M 4 64 H 24 M 104 64 H 124" stroke="#4a505c" stroke-width="7" stroke-linecap="round"/><rect x="24" y="20" width="80" height="88" rx="12" fill="#2a2e36" stroke="#d8a83b" stroke-width="5"/><g stroke="#d8a83b" stroke-width="4" fill="#121418"><circle cx="46" cy="44" r="9"/><circle cx="82" cy="44" r="9"/></g><g stroke="#d6d9df" stroke-width="3.5" stroke-linecap="round"><path d="M 46 44 L 41 37"/><path d="M 82 44 L 87 37"/></g><circle cx="64" cy="84" r="13" fill="#d8a83b"/><circle cx="64" cy="84" r="6" fill="#121418"/></svg>`;
  TP.wordmark = (size = 17) => `<span style="font-size:${size}px;font-weight:700;letter-spacing:-0.03em">Tone<span style="color:var(--accent)">Push</span></span>`;
  /* A generic HX-style floor unit, ours: screen, six knobs, three switches. */
  TP.hxArt = (w = 210) => {
    const ln = 'var(--line-strong)', soft = 'var(--raised)';
    let s = `<svg width="${w}" height="${w * 0.62}" viewBox="0 0 210 130"><rect x="4" y="4" width="202" height="122" rx="14" fill="var(--panel)" stroke="${ln}"/>`;
    s += `<rect x="18" y="16" width="62" height="34" rx="4" fill="var(--bg-deep)" stroke="${ln}"/>`;
    [96, 114, 132, 150, 168, 186].forEach(x => { s += `<circle cx="${x}" cy="33" r="6.5" fill="${soft}" stroke="${ln}"/>`; });
    [42, 105, 168].forEach(x => { s += `<circle cx="${x}" cy="92" r="17" fill="none" stroke="${ln}" stroke-width="2"/><circle cx="${x}" cy="92" r="11" fill="${soft}" stroke="${ln}"/>`; });
    return s + `</svg>`;
  };
  TP.proArt = (w = 210) => {
    const ln = 'var(--line-strong)', soft = 'var(--raised)';
    let s = `<svg width="${w}" height="${w * 0.62}" viewBox="0 0 210 130"><rect x="4" y="4" width="202" height="122" rx="14" fill="var(--panel)" stroke="${ln}"/>`;
    s += `<rect x="18" y="16" width="84" height="44" rx="4" fill="var(--bg-deep)" stroke="${ln}"/>`;
    s += `<circle cx="134" cy="38" r="15" fill="${soft}" stroke="${ln}"/><circle cx="134" cy="38" r="9" fill="var(--bg-deep)" stroke="${ln}"/>`;
    [168, 188].forEach(x => { [26, 50].forEach(y => { s += `<circle cx="${x}" cy="${y}" r="6" fill="${soft}" stroke="${ln}"/>`; }); });
    [42, 105, 168].forEach(x => { s += `<circle cx="${x}" cy="98" r="15" fill="none" stroke="${ln}" stroke-width="2"/><circle cx="${x}" cy="98" r="10" fill="${soft}" stroke="${ln}"/>`; });
    return s + `</svg>`;
  };

  /* ---------- mount ---------- */
  const after = [];
  TP.later = (fn) => after.push(fn);
  TP.mount = function (html) {
    document.body.innerHTML = html;
    for (const fn of after) fn();
    document.fonts.ready.then(() => { document.body.dataset.ready = '1'; });
  };
  window.TP = TP;
})();
