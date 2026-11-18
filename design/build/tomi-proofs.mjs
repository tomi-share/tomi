// Tomi: visual proofs at REAL size rendered by Microsoft Edge headless (1 CSS px = 1 screen px, device
// scale factor forced to 1), then the same capture enlarged with image-rendering: pixelated.
// Output (versioned, referenced by the charter): design/tomi/proofs/*.png
//   tailles-*-actual.png / -x8.png   variants at 16, 20, 24, 32 px on light and dark, colour and monochrome
//   exports-png-x8.png             the exported PNG files (design/tomi/export) shown at 1:1, enlarged
//   confusions-x4.png              next to look-alike system icons (sync, refresh, spinner, power)
//   situ-actual.png / situ-x3.png    Windows tray, title bar, Android status bar and notification, home screen
//   animation.png                  frames of the lap of the bead
// Usage: cd design/build && node tomi-export.mjs && node tomi-proofs.mjs   (EDGE_PATH overrides Edge)

import { readFile, writeFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import * as G from './tomi-geometry.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const tomiDir = path.resolve(here, '..', 'tomi');
const outDir = path.join(tomiDir, 'proofs');
const exportDir = path.join(tomiDir, 'export');
const tokens = JSON.parse(await readFile(path.join(tomiDir, 'tokens.json'), 'utf8'));
const tok = (n) => {
  if (!tokens[n]) throw new Error(`Unknown token: ${n}`);
  return tokens[n].value;
};
const edge = [process.env.EDGE_PATH, 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe', 'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe']
  .filter(Boolean)
  .find((p) => existsSync(p));
if (!edge) {
  console.error('FAILED: Microsoft Edge not found; set EDGE_PATH');
  process.exit(1);
}
if (!existsSync(path.join(exportDir, 'tomi.ico'))) {
  console.error('FAILED: design/tomi/export is missing; run node tomi-export.mjs first');
  process.exit(1);
}

async function runEdge(htmlFile, pngFile, width, height) {
  for (let attempt = 1; ; attempt += 1) {
    const profile = await mkdtemp(path.join(os.tmpdir(), 'tomi-edge-'));
    try {
      await rm(pngFile, { force: true });
      const r = spawnSync(edge, ['--headless', '--disable-gpu', '--no-first-run', '--hide-scrollbars', '--force-device-scale-factor=1', `--user-data-dir=${profile}`, `--window-size=${width},${height}`, `--screenshot=${pngFile}`, pathToFileURL(htmlFile).href], { encoding: 'utf8', timeout: 90000, killSignal: 'SIGKILL' });
      if (r.error) throw r.error;
      if (!existsSync(pngFile)) throw new Error(`Edge produced no screenshot (status ${r.status}): ${r.stderr}`);
      return;
    } catch (e) {
      if (attempt >= 3) throw e;
      console.log(`Edge attempt ${attempt} failed (${e.message.split('\n')[0]}), retrying`);
    } finally {
      await rm(profile, { recursive: true, force: true }).catch(() => {});
    }
  }
}
const shoot = async (name, body, w, h, bg = '#FFFFFF') => {
  const hf = path.join(outDir, `_${name}.html`);
  const pf = path.join(outDir, `${name}.png`);
  await writeFile(hf, `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;padding:0;background:${bg};overflow:hidden;font-family:"Segoe UI",sans-serif;color:#1B2130}</style></head><body>${body}</body></html>`, 'utf8');
  await runEdge(hf, pf, w, h);
  await rm(hf);
  console.log(`proof: ${path.relative(tomiDir, pf)}`);
  return pf;
};
const uri = async (p) => `data:image/png;base64,${(await readFile(p)).toString('base64')}`;
const fileSvg = async (name) =>
  (await readFile(path.join(tomiDir, name), 'utf8')).replace(/<\?xml[^>]*\?>\s*/, '').replace(/<!--[\s\S]*?-->\s*/g, '');
const sized = (svg, size, color) =>
  svg
    .replace(/ width="[\d.]+"/, ` width="${size}"`)
    .replace(/ height="[\d.]+"/, ` height="${size}"`)
    .replace(/ color="[^"]*"/, '')
    .replace(/<svg /, `<svg style="display:block;flex:none${color ? `;color:${color}` : ''}" `);
const sym256 = (inner, size) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="${size}" height="${size}" style="display:block;flex:none">${inner}</svg>`;

await mkdir(outDir, { recursive: true });
const SVG = {};
for (const n of ['logo.svg', 'logo-16.svg', 'logo-20.svg', 'logo-24.svg', 'logo-app.svg', 'logo-app-16.svg', 'logo-app-20.svg', 'logo-app-24.svg']) SVG[n] = await fileSvg(n);

// ---------- 1. size tables (real + x8) ----------
const SIZES = [16, 20, 24, 32];
const CELL = 40;
const ZOOM = 8;
const ROWS = [
  { label: `Light background ${tok('color.light.bg')} · night ${tok('color.identity.night')}`, bg: tok('color.light.bg'), fg: tok('color.identity.night') },
  { label: `Light background ${tok('color.light.bg')} · black #000000`, bg: tok('color.light.bg'), fg: '#000000' },
  { label: `Dark background ${tok('color.dark.bg')} · white #FFFFFF`, bg: tok('color.dark.bg'), fg: '#FFFFFF' },
  { label: `Dark background ${tok('color.dark.bg')} · brand ${tok('color.dark.brand')}`, bg: tok('color.dark.bg'), fg: tok('color.dark.brand') },
];
async function table(name, title, cell, rows = ROWS) {
  const w = SIZES.length * CELL;
  const h = rows.length * CELL;
  const cells = rows.map((row) => SIZES.map((s) => `<div style="width:${CELL}px;height:${CELL}px;background:${row.bg};display:flex;align-items:center;justify-content:center">${cell(row, s)}</div>`).join('')).join('');
  const real = await shoot(`${name}-actual`, `<div style="display:grid;grid-template-columns:repeat(${SIZES.length},${CELL}px)">${cells}</div>`, w, h);
  // Labels sized to stay readable once the image is scaled down to a PDF page (about 0.4x).
  const labelW = 400;
  const titleH = 96;
  const headH = 56;
  const zoomBody = `<div style="height:${titleH}px;display:flex;align-items:center;padding:0 16px;font-weight:600;font-size:28px;line-height:1.25">${title} · Edge capture at real size, enlarged x${ZOOM}</div>
<div style="display:flex;margin-left:${labelW}px;height:${headH}px">${SIZES.map((s) => `<div style="width:${CELL * ZOOM}px;display:flex;align-items:center;justify-content:center;font-weight:600;font-size:30px">${s} px</div>`).join('')}</div>
<div style="display:flex"><div>${rows.map((r) => `<div style="width:${labelW}px;height:${CELL * ZOOM}px;display:flex;align-items:center;padding:0 16px;box-sizing:border-box;font-size:26px;line-height:1.3">${r.label}</div>`).join('')}</div>
<img src="${await uri(real)}" style="width:${w * ZOOM}px;height:${h * ZOOM}px;image-rendering:pixelated;display:block"></div>`;
  await shoot(`${name}-x8`, zoomBody, labelW + w * ZOOM, titleH + headH + h * ZOOM);
}
const pick = (s) => (s <= 24 ? `logo-${s}.svg` : 'logo.svg');
await table('tailles-reduction', 'logo.svg simply reduced (do not do this below 32 px)', (row, s) => sized(SVG['logo.svg'], s, row.fg));
await table('tailles-choix', 'Drawing chosen per size: logo-16 · logo-20 · logo-24 · logo (32)', (row, s) => sized(SVG[pick(s)], s, row.fg));
const APP_ROWS = [
  { label: `Light background ${tok('color.light.bg')}`, bg: tok('color.light.bg') },
  { label: 'White background #FFFFFF', bg: '#FFFFFF' },
  { label: `Dark background ${tok('color.dark.bg')}`, bg: tok('color.dark.bg') },
  { label: 'Dark taskbar #1F1F1F', bg: '#1F1F1F' },
];
await table('tailles-app', 'App icon: logo-app-16 · -20 · -24 drawn pixel by pixel, logo-app at 32', (row, s) => sized(SVG[s <= 24 ? `logo-app-${s}.svg` : 'logo-app.svg'], s), APP_ROWS);

// exported PNG files at 1:1
{
  const map = {};
  for (const s of SIZES) {
    map[`light-${s}`] = await uri(path.join(exportDir, `tray-light-${s}.png`));
    map[`dark-${s}`] = await uri(path.join(exportDir, `tray-dark-${s}.png`));
    map[`app-${s}`] = await uri(path.join(exportDir, `tomi-${s}.png`));
  }
  const rows = [
    { label: 'tray-light-N.png on light taskbar #EEEEEE', bg: '#EEEEEE', k: 'light' },
    { label: 'tray-dark-N.png on dark taskbar #1F1F1F', bg: '#1F1F1F', k: 'dark' },
    { label: 'tomi-N.png (tomi.ico) on #EEEEEE', bg: '#EEEEEE', k: 'app' },
    { label: 'tomi-N.png (tomi.ico) on #1F1F1F', bg: '#1F1F1F', k: 'app' },
  ];
  await table('exports-png', 'Exported PNGs (design/tomi/export), shown at 1:1', (row, s) => `<img src="${map[`${row.k}-${s}`]}" width="${s}" height="${s}" style="display:block">`, rows);
}

// ---------- 2. confusion test ----------
{
  const wifi = (c) => `<g fill="none" stroke="${c}" stroke-width="22" stroke-linecap="round"><path d="M40 108A124 124 0 0 1 216 108"/><path d="M76 144A72 72 0 0 1 180 144"/></g><circle cx="128" cy="190" r="18" fill="${c}"/>`;
  const at = (s, fg, isTomi, g) => (isTomi ? sized(SVG[pick(s)], s, fg) : sym256(g(fg), s));
  const list = [[false, wifi], [false, G.LOOKALIKES.sync], [true], [false, G.LOOKALIKES.refresh], [false, G.LOOKALIKES.spinner], [false, G.LOOKALIKES.power]];
  const row = (bg, fg, s) => `<div style="display:flex;align-items:center;gap:${s / 2 + 6}px;height:${s + 14}px;padding:0 10px;background:${bg}">${list.map(([t, g]) => at(s, fg, t, g)).join('')}</div>`;
  const W = 270;
  const H = 30 + 30 + 38;
  const real = await shoot('confusions-actual', `<div style="width:${W}px">${row('#EEEEEE', '#1A1A1A', 16)}${row('#1F1F1F', '#FFFFFF', 16)}${row('#EEEEEE', '#1A1A1A', 24)}</div>`, W, H);
  await shoot('confusions-x4', `<div style="height:96px;padding:16px 14px 0;box-sizing:border-box;font-size:24px;line-height:1.3;font-weight:600">Look-alikes, real size enlarged x4: wifi, sync, <u>Tomi</u>, refresh, spinner, power · 16 px light, 16 px dark, 24 px light</div><img src="${await uri(real)}" style="width:${W * 4}px;height:${H * 4}px;image-rendering:pixelated;display:block;margin-left:14px">`, W * 4 + 28, H * 4 + 106);
}

// ---------- 3. in context ----------
{
  const NIGHT = tok('color.identity.night');
  const SUN = tok('color.identity.sun');
  const png = async (f) => uri(path.join(exportDir, f));
  const ic = (g, c, s) => sym256(g(c), s);
  const glyph = {
    chevron: (c) => `<path fill="none" stroke="${c}" stroke-width="22" d="M64 160L128 96L192 160"/>`,
    cloud: (c) => `<path fill="${c}" d="M72 192A44 44 0 0 1 76 104A60 60 0 0 1 188 96A48 48 0 0 1 188 192Z"/>`,
    wifi: (c) => `<g fill="none" stroke="${c}" stroke-width="22" stroke-linecap="round"><path d="M40 108A124 124 0 0 1 216 108"/><path d="M76 144A72 72 0 0 1 180 144"/></g><circle cx="128" cy="190" r="18" fill="${c}"/>`,
    sound: (c) => `<path fill="${c}" d="M40 100H84L140 52V204L84 156H40Z"/><path fill="none" stroke="${c}" stroke-width="20" stroke-linecap="round" d="M172 92A50 50 0 0 1 172 164M196 64A90 90 0 0 1 196 192"/>`,
    battery: (c) => `<rect x="28" y="80" width="180" height="96" rx="18" fill="none" stroke="${c}" stroke-width="18"/><rect x="216" y="108" width="16" height="40" fill="${c}"/><rect x="52" y="104" width="110" height="48" fill="${c}"/>`,
    chat: (c) => `<path fill="${c}" d="M48 52H208A20 20 0 0 1 228 72V164A20 20 0 0 1 208 184H104L56 220V184H48A20 20 0 0 1 28 164V72A20 20 0 0 1 48 52Z"/>`,
    mail: (c) => `<rect x="32" y="64" width="192" height="128" rx="14" fill="none" stroke="${c}" stroke-width="20"/><path fill="none" stroke="${c}" stroke-width="20" d="M40 76L128 144L216 76"/>`,
    signal: (c) => `<g fill="${c}"><rect x="40" y="168" width="32" height="48"/><rect x="92" y="128" width="32" height="88"/><rect x="144" y="84" width="32" height="132"/><rect x="196" y="40" width="32" height="176"/></g>`,
    play: (c) => `<path fill="${c}" d="M88 56L208 128L88 200Z"/>`,
    note: (c) => `<rect x="56" y="32" width="144" height="192" rx="16" fill="${c}"/>`,
    map: (c) => `<path fill="${c}" fill-rule="evenodd" d="M128 228C80 168 60 136 60 104A68 68 0 0 1 196 104C196 136 176 168 128 228Z M128 76A28 28 0 1 0 128.1 76Z"/>`,
    sync: G.LOOKALIKES.sync,
  };
  const tray = async (dark, s) => {
    const bg = dark ? '#1F1F1F' : '#EEEEEE';
    const fg = dark ? '#FFFFFF' : '#1A1A1A';
    const tomi = `<img src="${await png(`tray-${dark ? 'dark' : 'light'}-${s}.png`)}" width="${s}" height="${s}" style="display:block">`;
    const others = [glyph.chevron, glyph.cloud].map((g) => ic(g, fg, s)).join('') + tomi + [glyph.wifi, glyph.sound, glyph.battery].map((g) => ic(g, fg, s)).join('');
    return `<div style="display:flex;align-items:center;gap:${Math.round(s * 0.5)}px;height:${s * 3}px;padding:0 ${Math.round(s * 0.5)}px;background:${bg};color:${fg};font-size:${Math.round(s * 0.75)}px;width:360px;box-sizing:border-box">${others}<div style="text-align:right;line-height:1.25;margin-left:auto">14:32<br>04/11/2026</div></div>`;
  };
  const titleBar = async () => `<div style="display:flex;align-items:center;gap:8px;height:32px;padding:0 10px;background:#F3F3F3;border-bottom:1px solid #ddd;font-size:12px;width:360px;box-sizing:border-box"><img src="${await png('tomi-16.png')}" width="16" height="16" style="display:block">Tomi<span style="margin-left:auto;letter-spacing:14px;color:#555">–□✕</span></div>`;
  const statusBar = () => {
    // Android draws the small icon at 24 dp; shown here at 1 dp = 1 px for a compact proof (phones use 2.6 to 3.5 px per dp).
    const ico = sym256(G.symbolInner('#FFFFFF'), 22);
    const i = (g) => ic(g, '#FFFFFF', 18);
    return `<div style="display:flex;align-items:center;gap:8px;height:30px;padding:0 14px;background:#101114;color:#fff;font:600 13px 'Segoe UI',sans-serif;width:360px;box-sizing:border-box"><span>14:32</span>${i(glyph.chat)}${ico}${i(glyph.mail)}<span style="flex:1"></span>${i(glyph.wifi)}${i(glyph.signal)}${i(glyph.battery)}</div>`;
  };
  const notif = () => `<div style="display:flex;gap:10px;align-items:center;width:360px;box-sizing:border-box;padding:10px 12px;background:#2A2C33;border-radius:18px;color:#fff;font-size:12px">
<div style="width:30px;height:30px;border-radius:50%;background:${tok('color.dark.brand')};display:flex;align-items:center;justify-content:center;flex:none">${sym256(G.symbolInner(tok('color.dark.on-brand')), 18)}</div>
<div style="line-height:1.35"><b>Tomi</b> · maintenant<br>Lien reçu depuis PC-Bureau</div></div>`;
  const home = () => {
    const apps = [['#2E7D5B', glyph.chat, 'Messages'], ['#1E5BD8', glyph.mail, 'Mail'], ['#D9412E', glyph.play, 'Vidéo'], [null, null, 'Tomi'], ['#6A4BC9', glyph.note, 'Notes'], ['#11867F', glyph.map, 'Plans'], ['#3C3F47', glyph.sync, 'Sauveg.'], [null, null, 'Tomi']];
    const cell = ([bg, g, label], i) => {
      // adaptive icon: 108 dp canvas, 72 dp visible; first Tomi in a circle mask, second in a squircle
      const shape = i === 3 ? '50%' : '30%';
      const icon = g
        ? `<div style="width:56px;height:56px;border-radius:${shape};background:${bg};display:flex;align-items:center;justify-content:center">${ic(g, '#FFFFFF', 28)}</div>`
        : `<div style="width:56px;height:56px;border-radius:${shape};overflow:hidden;background:${NIGHT};display:flex;align-items:center;justify-content:center">${sym256(G.symbolInner(SUN), Math.round((56 * 256 * G.TILE_SCALE) / 208 * (208 / 256)))}</div>`;
      return `<div style="display:flex;flex-direction:column;align-items:center;gap:6px;width:76px">${icon}<span style="font-size:11px;color:#fff;text-shadow:0 1px 2px #0008">${label}</span></div>`;
    };
    return `<div style="display:grid;grid-template-columns:repeat(4,76px);gap:14px 10px;padding:18px 16px;width:360px;box-sizing:border-box;border-radius:18px;background:linear-gradient(160deg,#6A7C98,#2B3348)">${apps.map(cell).join('')}</div>`;
  };
  const body = `<div style="display:flex;flex-direction:column;gap:6px;width:360px">${await tray(false, 16)}${await tray(true, 16)}${await tray(false, 24)}${await titleBar()}${statusBar()}${notif()}${home()}</div>`;
  const real = await shoot('situ-actual', body, 360, 540);
  await shoot('situ-x3', `<img src="${await uri(real)}" style="width:${360 * 3}px;height:${540 * 3}px;image-rendering:pixelated;display:block">`, 360 * 3, 540 * 3);
}

// ---------- 4. animation frames ----------
{
  const frames = [0, 45, 90, 135, 180, 225, 270, 315, 360];
  const body = `<div style="display:flex;gap:14px;padding:16px;align-items:flex-end">${frames
    .map((d, i) => `<div style="display:flex;flex-direction:column;align-items:center;gap:6px"><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="96" height="96">${G.tileInner(tok('color.identity.night'), tok('color.identity.sun'), d)}</svg><span style="font-size:12px;color:#666">${i === 0 ? 'rest' : i === frames.length - 1 ? 'back' : `${d}°`}</span></div>`)
    .join('')}</div>`;
  await shoot('animation', body, 16 * 2 + frames.length * 96 + (frames.length - 1) * 14, 140);
}
console.log('OK');
