/* The StompStation PRO's own pages: the frame shared by its libraries,
   backups and firmware screens. */
(function () {
  const { I, G } = TP;
  const PRO = {};
  PRO.side = (o = {}) => {
    const presets = D.proPresets().slice(0, 22);
    presets[7].sel = true;
    if (o.update) presets.forEach(p => { p.lib = null; });
    return TP.sidebar(Object.assign({
      device: { name: 'StompStation PRO', status: o.status || 'Connected · 2.2.6', online: o.online !== false, icon: 'tp-pedal' },
      nav: 'device', listAside: '24 of 60', presets,
      foot: o.foot || `<span class="ok row">${I('shield-check', 's14')}</span><span class="grow ell">Protected · backup 14:02</span><span class="iconbtn sm">${I('settings', 's14')}</span>`,
    }, o.sidebar || {}));
  };
  PRO.mini = (o = {}) => o.none ? `<header class="minideck"><span class="muted" style="font-size:13px">${o.text}</span><div class="grow"></div></header>`
    : TP.minideck({ slot: '03B', name: 'Velvet Drive', chain: ['dyn', 'dyn', 'wah!', 'dist', 'amp', 'ir', 'eq', ['mod', 'mod!'], 'delay', 'reverb'] });
  PRO.head = (tab, o = {}) => `<div class="phead" style="padding-bottom:14px"><span class="bwell" style="--cat:var(--text-soft);background:var(--raised);border-color:var(--line-strong)">${I('tp-pedal', 's20')}</span>
    <div><div class="ph1">StompStation PRO</div><div class="phsub">${o.sub || 'Firmware 2.2.6 · 24 of 60 presets · NAM player with dual amp · stereo IR loader'}</div></div><span class="grow"></span>
    ${o.right || `<span class="chip ok" style="height:24px">${I('shield-check', 's12')}Protected by the 14:02 backup</span>`}</div>
    <div class="tabs">${[['backups', 'Backups'], ['amps', 'NAM amps', 14], ['drives', 'NAM drives', 9], ['irs', 'Impulse responses', 11], ['settings', 'Settings'], ['firmware', 'Firmware']]
      .map(([k, l, n]) => `<span class="t ${tab === k ? 'on' : ''}">${l}${n ? ` <span class="n">${n}</span>` : ''}${k === 'firmware' && o.fwDot ? '<span class="dot" style="color:var(--accent);margin-left:2px"></span>' : ''}</span>`).join('')}</div>`;
  window.PRO = PRO;
})();

/* ---------- firmware update flow ---------- */
(function () {
  const { I } = TP;
  const STEPS = ['Back up', 'Update Mode', 'Write', 'Restart', 'Check'];
  PRO.stepper = (states) => `<div class="steps">${STEPS.map((s, i) => {
    const st = states[i] || 'wait';
    const b = st === 'done' ? I('check', 's12') : st === 'fail' ? I('x', 's12') : i + 1;
    return `${i ? `<span class="stepline ${states[i - 1] === 'done' ? 'done' : ''}"></span>` : ''}<span class="step ${st}"><span class="b">${b}</span>${s}</span>`;
  }).join('')}</div>`;
  /* A generic drawing of the pedal, ours: the top with its screen, F1-F4,
     the encoder and footswitches A B C, and the rear edge with UPD lit. */
  PRO.pedalArt = (o = {}) => {
    const acc = 'var(--accent)', ln = 'var(--line-strong)', fg = 'var(--muted)', soft = 'var(--raised)';
    const jacks = [['IN 1', 40], ['IN 2', 72], ['OUT 1', 110], ['OUT 2', 142], ['USB', 186], ['9V', 222]];
    let s = `<svg width="320" height="250" viewBox="0 0 320 250" style="overflow:visible">`;
    // rear edge
    s += `<rect x="10" y="8" width="300" height="46" rx="10" fill="var(--panel)" stroke="${ln}"/>`;
    for (const [l, x] of jacks) {
      s += l === 'USB' ? `<rect x="${x - 9}" y="22" width="18" height="9" rx="4" fill="${soft}" stroke="${ln}"/>` : `<circle cx="${x}" cy="27" r="7" fill="${soft}" stroke="${ln}"/>`;
      s += `<text x="${x}" y="47" text-anchor="middle" font-size="8.5" font-weight="600" fill="${fg}" font-family="Inter">${l}</text>`;
    }
    const upd = o.upd !== false;
    s += `<circle cx="270" cy="27" r="${upd ? 12 : 0}" fill="none" stroke="${acc}" stroke-opacity="0.35" stroke-width="6"/>`;
    s += `<circle cx="270" cy="27" r="6" fill="${upd ? acc : soft}" stroke="${upd ? acc : ln}"/>`;
    s += `<text x="270" y="47" text-anchor="middle" font-size="8.5" font-weight="700" fill="${upd ? acc : fg}" font-family="Inter">UPD</text>`;
    // top
    s += `<rect x="10" y="66" width="300" height="176" rx="16" fill="var(--panel)" stroke="${ln}"/>`;
    s += `<rect x="30" y="84" width="128" height="62" rx="6" fill="var(--bg-deep)" stroke="${ln}"/>`;
    if (o.screenText !== '') s += `<text x="94" y="119" text-anchor="middle" font-size="10" font-weight="600" fill="${fg}" font-family="Inter">${o.screenText || 'Update Mode'}</text>`;
    // the BROWSE/VOLUME encoder, and F1 to F4 under the screen
    s += `<circle cx="214" cy="112" r="20" fill="${soft}" stroke="${ln}"/><circle cx="214" cy="112" r="13" fill="var(--bg-deep)" stroke="${ln}"/>`;
    s += `<text x="214" y="148" text-anchor="middle" font-size="8" font-weight="600" fill="${fg}" font-family="Inter">BROWSE</text>`;
    [44, 74, 104, 134].forEach((x) => { s += `<circle cx="${x}" cy="160" r="7" fill="${soft}" stroke="${ln}"/>`; });
    [70, 160, 250].forEach((x, i) => {
      s += `<circle cx="${x}" cy="212" r="15" fill="none" stroke="${o.leds ? o.leds[i] : ln}" stroke-width="2"/>`;
      s += `<circle cx="${x}" cy="212" r="10" fill="${soft}" stroke="${ln}"/>`;
      s += `<text x="${x + 24}" y="216" text-anchor="middle" font-size="8.5" font-weight="700" fill="${fg}" font-family="Inter">${'ABC'[i]}</text>`;
    });
    ['F1', 'F2', 'F3', 'F4'].forEach((l, i) => { s += `<text x="${44 + i * 30}" y="178" text-anchor="middle" font-size="8" font-weight="600" fill="${fg}" font-family="Inter">${l}</text>`; });
    return s + `</svg>`;
  };
  PRO.fwPage = (o) => {
    const side = PRO.side({ status: o.sideStatus || 'Connected · 1.5.12', online: o.online !== false,
      foot: o.foot || `<span class="ok row">${I('shield-check', 's14')}</span><span class="grow ell">Protected · backup 14:02</span><span class="iconbtn sm">${I('settings', 's14')}</span>` });
    const mini = PRO.mini(o.mini || { none: true, text: o.miniText || 'Editing is paused while the firmware updates.' });
    const head = PRO.head('firmware', { sub: o.sub || 'Firmware 1.5.12 · 24 of 60 presets', right: o.right || `<span class="chip" style="height:24px">${I('package-check', 's12')}s_pro_2_2_6.upd · firmware 2.2.6</span>` });
    const flow = `<div style="flex:1;min-height:0;overflow:hidden;padding:18px 20px 0;display:flex;justify-content:center">
      <div style="width:100%;max-width:880px;display:flex;flex-direction:column;gap:14px">
        <div class="card" style="padding:14px 18px">${PRO.stepper(o.states)}</div>
        <div class="card" style="flex:none;display:flex;flex-direction:column;overflow:hidden">${o.body}${o.footer ? `<div class="df" style="padding:14px 20px;border-top:1px solid var(--line);display:flex;align-items:center;gap:10px">${o.footer}</div>` : ''}</div>
      </div></div>`;
    return `<div class="win">${side}<div class="main">${mini}${head}${flow}</div>${o.dialog || ''}</div>`;
  };
})();
