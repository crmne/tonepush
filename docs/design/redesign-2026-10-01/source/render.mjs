// Renders the mockups to PNG with playwright-core and the system Chromium.
// Run it under gpu-lock (one GPU job at a time on this machine):
//   gpu-lock node source/render.mjs            every scene
//   gpu-lock node source/render.mjs 01 09      scenes whose name starts with these
// playwright-core is not a dependency of this repository; install it anywhere
// outside the tree and point NODE_PATH at it.
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// CommonJS resolution, so NODE_PATH can point at an install outside the tree.
const { chromium } = createRequire(import.meta.url)('playwright-core');
const src = path.dirname(fileURLToPath(import.meta.url));
const out = path.dirname(src);
const S = [1280, 760], L = [2560, 1440], XS = [1024, 640];
const scenes = [
  ['01-hx-editor', [S, L, XS]],
  ['02-hx-model-browser', [S]],
  ['03-hx-footswitches', [S]],
  ['04-hx-snapshots', [S]],
  ['05-presets-setlists', [S]],
  ['06-setlist-confirm', [S], '05-presets-setlists.html?dialog=confirm'],
  ['07-library-send', [S]],
  ['08-hx-backups', [S]],
  ['09-pro-editor', [S, L, XS]],
  ['10-pro-nam-library', [S]],
  ['11-pro-ir-library', [S]],
  ['12-pro-firmware-backup', [S]],
  ['13-pro-firmware-update-mode', [S]],
  ['14-pro-firmware-confirm', [S], '13-pro-firmware-update-mode.html?found=1'],
  ['15-pro-firmware-writing', [S]],
  ['16-pro-firmware-power-cycle', [S]],
  ['17-pro-firmware-failed', [S]],
  ['18-connect', [S, XS]],
  ['19-hx-editor-light', [S, L], '01-hx-editor.html?theme=light'],
  ['20-pro-editor-light', [S], '09-pro-editor.html?theme=light'],
  ['21-pro-not-protected', [S], '09-pro-editor.html?state=unprotected'],
];
const wanted = process.argv.slice(2);
const browser = await chromium.launch({
  executablePath: '/usr/bin/chromium',
  args: ['--allow-file-access-from-files', '--use-angle=gl-egl', '--use-gl=angle', '--force-color-profile=srgb', '--font-render-hinting=none'],
});
{
  const probe = await browser.newPage();
  const gl = await probe.evaluate(() => {
    const c = document.createElement('canvas').getContext('webgl');
    const e = c && c.getExtension('WEBGL_debug_renderer_info');
    return e ? c.getParameter(e.UNMASKED_RENDERER_WEBGL) : 'no webgl';
  });
  console.log('renderer:', gl);
  await probe.close();
}
for (const [name, sizes, file] of scenes) {
  if (wanted.length && !wanted.some(w => name.startsWith(w))) continue;
  for (const [w, h] of sizes) {
    const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: 1 });
    page.on('console', m => { if (m.type() === 'error') console.log(name, 'console:', m.text()); });
    page.on('pageerror', e => console.log(name, 'error:', e.message));
    const url = 'file://' + path.join(src, file || name + '.html');
    await page.goto(url);
    await page.waitForFunction(() => document.body.dataset.ready === '1', null, { timeout: 15000 });
    await page.waitForTimeout(120);
    const target = path.join(out, `${name}-${w}x${h}.png`);
    await page.screenshot({ path: target });
    console.log('rendered', path.basename(target));
    await page.close();
  }
}
await browser.close();
