// Builds the guidelines PDF, design/tomi/brand-guidelines.pdf. The output is generated locally and not versioned.
// Inputs: ../tomi/brand-guidelines.md, ../tomi/tokens.json, tomi-geometry.mjs (shapes), ../icons/*.svg, ../fonts/*.ttf,
// ../tomi/proofs/*.png (run tomi-export.mjs and tomi-proofs.mjs first).
// Output: ./tomi-brand-guidelines.html (print template, kept for inspection) and ../tomi/brand-guidelines.pdf (Edge headless).
// Usage: cd design/build && npm ci && node tomi-build-pdf.mjs && node verify-pdf.mjs ../tomi/brand-guidelines.pdf

import { readFile, writeFile, mkdtemp, rm, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import MarkdownIt from 'markdown-it';
import * as G from './tomi-geometry.mjs';
import { contrast } from './tomi-contrasts.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const designDir = path.resolve(here, '..');
const tomiDir = path.join(designDir, 'tomi');
const htmlPath = path.join(here, 'tomi-brand-guidelines.html');
const pdfPath = path.join(tomiDir, 'brand-guidelines.pdf');

const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const strip = (svg) => svg.replace(/<\?xml[^>]*\?>\s*/, '').replace(/<!--[\s\S]*?-->\s*/g, '');
const tokens = JSON.parse(await readFile(path.join(tomiDir, 'tokens.json'), 'utf8'));
const tok = (name) => {
  if (!tokens[name]) throw new Error(`Unknown token: ${name}`);
  return tokens[name].value;
};
const cssFeature = (name) => tok(name).replace(/"/g, "'");
const fmt = (n, d = 2) => n.toFixed(d).replace('.', ',');
const NIGHT = tok('color.identity.night');
const SUN = tok('color.identity.sun');
for (const p of ['proofs/tailles-choix-x8.png', 'export/tomi.ico']) if (!existsSync(path.join(tomiDir, p))) throw new Error(`missing design/tomi/${p}: run tomi-export.mjs and tomi-proofs.mjs first`);

// ---------- SVG helpers ----------
const svg256 = (inner, size, extra = '') => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="${size}" height="${size}" style="display:block;flex:none"${extra}>${inner}</svg>`;
const sym = (color, size, rot = 0) => svg256(G.symbolInner(color, rot), size);
const tile = (size, rot = 0) => svg256(G.tileInner(NIGHT, SUN, rot), size);
const gridSvg = (rows, n, color, size = n) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}" width="${size}" height="${size}" shape-rendering="crispEdges" style="display:block;flex:none"><path fill="${color}" d="${G.gridD(rows)}"/></svg>`;
const symAt = (color, size) => (size === 16 ? gridSvg(G.GRID16, 16, color) : size === 20 ? gridSvg(G.GRID20, 20, color) : size === 24 ? gridSvg(G.GRID24, 24, color) : sym(color, size));
const lockupH = (h, symC, wordC = symC) => {
  const [, y0, w, hh] = G.lockupHViewBox.split(' ').map(Number);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${G.lockupHViewBox}" height="${h}" width="${((h * w) / hh).toFixed(1)}" style="display:block">${G.lockupHInner(symC, wordC)}</svg>`;
};
const lockupV = (h, symC, wordC = symC) => {
  const [, , w, hh] = G.lockupVViewBox.split(' ').map(Number);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${G.lockupVViewBox}" height="${h}" width="${((h * w) / hh).toFixed(1)}" style="display:block">${G.lockupVInner(symC, wordC)}</svg>`;
};
const word = (h, color) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 ${G.WORD_BOX.y0} ${G.WORD_BOX.x1} ${G.WORD_BOX.y1 - G.WORD_BOX.y0}" height="${h}" width="${((h * G.WORD_BOX.x1) / (G.WORD_BOX.y1 - G.WORD_BOX.y0)).toFixed(1)}" style="display:block">${G.wordInner(color)}</svg>`;

// ---------- figures ----------
function figSymbole() {
  const L = tok('color.light.bg');
  const D = tok('color.dark.bg');
  const sizes = (fg, bg) => `<div class="sizes" style="background:${bg}">${[64, 32, 24, 20, 16].map((s) => `<span class="sz">${symAt(fg, s)}<small style="color:${fg}">${s}</small></span>`).join('')}</div>`;
  return `<div class="figure sym-fig">
  <div class="sym-row">
    <div class="sym-cell" style="background:${L}">${sym(NIGHT, 170)}</div>
    <div class="sym-cell" style="background:${D}">${sym('#FFFFFF', 170)}</div>
    <div class="sym-cell" style="background:#fff">${tile(170)}</div>
  </div>
  <div class="sym-row">${sizes(NIGHT, L)}${sizes('#FFFFFF', D)}</div>
  <p class="figcaption">Night on a light background, white on a dark background, app icon. Below: 64 to 16 px, each size with its own drawing (16, 20 and 24 px drawn pixel by pixel).</p>
</div>`;
}

function figConstruction() {
  const { cx, cy, R, w, br, clear } = G.M;
  const { Ro, Ri, i0, i1 } = G.geometryNotes;
  const bx = G.BEAD.x;
  const by = G.BEAD.y;
  const blue = '#2E4C80';
  const red = '#B3261E';
  const ray = (deg, r) => [cx + r * Math.cos((deg * Math.PI) / 180), cy + r * Math.sin((deg * Math.PI) / 180)];
  const [ax0, ay0] = ray(180 + clear, 112);
  const [ax1, ay1] = ray(180 - clear, 112);
  const t = (x, y, s, anchor = 'start', c = '#1B2130') => `<text x="${x}" y="${y}" font-size="7.5" font-family="Inter" fill="${c}" text-anchor="${anchor}">${s}</text>`;
  const inner = `<rect x="-50" y="0" width="380" height="256" fill="#fff"/>
<g fill="#D9DEE8">${G.symbolInner('#D9DEE8')}</g>
<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="${blue}" stroke-width="0.8" stroke-dasharray="3 2"/>
<circle cx="${cx}" cy="${cy}" r="${Ro}" fill="none" stroke="${blue}" stroke-width="0.4"/>
<circle cx="${cx}" cy="${cy}" r="${Ri}" fill="none" stroke="${blue}" stroke-width="0.4"/>
<path d="M${cx - 5} ${cy}H${cx + 5}M${cx} ${cy - 5}V${cy + 5}M${bx - 5} ${by}H${bx + 5}M${bx} ${by - 5}V${by + 5}" stroke="${blue}" stroke-width="0.8"/>
<path d="M${cx} ${cy}L${ax0} ${ay0}M${cx} ${cy}L${ax1} ${ay1}" stroke="${red}" stroke-width="0.6" stroke-dasharray="2 2"/>
<path d="M${cx} ${cy}L${cx + Ro} ${cy}" stroke="${blue}" stroke-width="0.6"/>
<circle cx="${bx}" cy="${by}" r="${br}" fill="none" stroke="${red}" stroke-width="0.6"/>
<path d="M${i0[0]} ${i0[1]}L${bx + (i0[0] - bx) * (br / Math.hypot(i0[0] - bx, i0[1] - by))} ${by + (i0[1] - by) * (br / Math.hypot(i0[0] - bx, i0[1] - by))}" stroke="${red}" stroke-width="1.2"/>
${t(cx + 6, cy - 4, `centre ${cx}, ${cy}`, 'start', blue)}
${t(cx + Ro + 4, cy + 3, `R ${R} · stroke ${w} (${Ri}–${Ro})`, 'start', blue)}
${t(bx - br - 3, by + 3, `bead r ${br}`, 'end', red)}
${t(ax0 - 2, ay0 - 3, `${180 + clear}°`, 'end', red)}
${t(ax1 - 2, ay1 + 8, `${180 - clear}°`, 'end', red)}
${t(i0[0] + 4, i0[1] - 8, `gap ${fmt(G.GAP_UNITS, 1)}`, 'start', red)}
<rect x="${G.SYM_BOX.x0}" y="${G.SYM_BOX.y0}" width="${G.SYM_BOX.x1 - G.SYM_BOX.x0}" height="${G.SYM_BOX.y1 - G.SYM_BOX.y0}" fill="none" stroke="#858996" stroke-width="0.4" stroke-dasharray="1 2"/>`;
  return `<div class="figure construction"><svg xmlns="http://www.w3.org/2000/svg" viewBox="-50 0 380 256" width="600" height="${Math.round((600 * 256) / 380)}">${inner}</svg>
<p class="figcaption">Centreline dashed, inner and outer radii of the ring, opening of ±${clear}° around the bead, shortest gap between the bead and a cut, bounding box.</p></div>`;
}

function figGrilles() {
  const cell = (rows, n, mm) => {
    const cells = rows.map((l) => [...l].map((ch) => `<i class="${ch === '#' ? 'on' : ''}"></i>`).join('')).join('');
    return `<div class="pixel-grid" style="grid-template-columns:repeat(${n},${mm}mm);grid-template-rows:repeat(${n},${mm}mm)">${cells}</div>`;
  };
  const block = (rows, n, mm, label) => `<div class="grid-block">${cell(rows, n, mm)}<div class="grid-real">
  <span style="background:${tok('color.light.bg')}">${gridSvg(rows, n, NIGHT)}</span>
  <span style="background:#000">${gridSvg(rows, n, '#FFFFFF')}</span>
  <span style="background:${tok('color.dark.bg')}">${gridSvg(rows, n, tok('color.dark.brand'))}</span></div><p class="figcaption">${label}</p></div>`;
  return `<div class="figure grilles">${block(G.GRID16, 16, 2.6, 'logo-16.svg · 16 x 16')}${block(G.GRID20, 20, 2.1, 'logo-20.svg · 20 x 20')}${block(G.GRID24, 24, 1.75, 'logo-24.svg · 24 x 24')}</div>
<p class="figcaption">Pixel-drawing grids (each cell = 1 px) and real-size rendering on three backgrounds.</p>`;
}

function figAnimation() {
  const frames = [0, 45, 90, 135, 180, 225, 270, 315, 360];
  return `<div class="figure anim">${frames.map((d, i) => `<div class="frame">${tile(62, d)}<small>${i === 0 ? 'rest' : i === frames.length - 1 ? 'back' : `${d}°`}</small></div>`).join('')}</div>
<p class="figcaption">The bead's lap, frame by frame (${tok('motion.logo.lap')} ms in total).</p>`;
}

function figAssemblages() {
  const L = tok('color.light.bg');
  const D = tok('color.dark.bg');
  const [, , w, hh] = G.lockupHViewBox.split(' ').map(Number);
  const H = 70;
  const k = H / hh;
  const pad = 64 * G.LOCKUP_H.s * k; // clear space = bead diameter at lockup scale
  const clear = `<div class="clear" style="padding:${pad.toFixed(1)}px;outline:1px dashed #B3261E;outline-offset:0;background:${L}"><div style="outline:1px solid #85899655">${lockupH(H, NIGHT)}</div></div>`;
  return `<div class="figure lockups">
  <div class="lk-row"><div class="lk" style="background:${L}">${lockupH(64, NIGHT)}</div><div class="lk" style="background:${D}">${lockupH(64, '#FFFFFF')}</div></div>
  <div class="lk-row"><div class="lk" style="background:${NIGHT}">${lockupH(64, SUN, '#FFFFFF')}</div><div class="lk" style="background:#fff;border:1px solid ${tok('color.light.border')}">${lockupH(64, '#000')}</div></div>
  <div class="lk-row"><div class="lk" style="background:${L}">${lockupV(150, NIGHT)}</div><div class="lk" style="background:${NIGHT}">${lockupV(150, SUN, '#FFFFFF')}</div><div class="lk" style="background:${L};flex-direction:column;gap:5mm">${word(44, tok('color.light.text'))}<div style="display:flex;gap:5mm;align-items:flex-end">${lockupH(24, NIGHT)}${lockupH(32, NIGHT)}</div><small>lockup at 24 and 32 px high</small></div></div>
  <div class="lk-row">${clear}<p class="figcaption" style="flex:1">Clear space (red dashes): the bead's diameter all around. Here ${Math.round(pad)} px for a lockup ${H} px high.</p></div>
</div>`;
}

function figIdentite() {
  const chip = (bg, fg, name, hex, lines) => `<div class="id-chip"><div class="id-sw" style="background:${bg};color:${fg}">${name}</div><div class="id-meta"><span class="hex">${hex}</span>${lines.map((l) => `<span>${l}</span>`).join('')}</div></div>`;
  const r1 = contrast(SUN, NIGHT);
  const r2 = contrast(SUN, tok('color.light.bg'));
  return `<div class="figure ident">
  ${chip(NIGHT, '#FFFFFF', 'Night', NIGHT, ['App icon tile, splash screen, store', `logo on light background · ${fmt(contrast(NIGHT, tok('color.light.bg')))}:1`])}
  ${chip(SUN, NIGHT, 'Sun', SUN, ['Symbol on night background only', `on night ${fmt(r1)}:1 · on light ${fmt(r2)}:1 (forbidden)`])}
  <div class="id-demo"><div style="background:${NIGHT}">${sym(SUN, 70)}</div><div style="background:${tok('color.light.bg')};position:relative">${sym(SUN, 70)}<b class="no">×</b></div></div>
</div>`;
}

function swatches(mode) {
  const t = (k) => tok(`color.${mode}.${k}`);
  const bgs = ['bg', 'surface', 'surface-alt'];
  const label = mode === 'light' ? 'Light mode' : 'Dark mode';
  const check = (ratio, min) => (ratio >= min ? '<b class="ok">AA</b>' : '<b class="ko">FAIL</b>');
  const backgrounds = bgs.map((k) => {
    const r1 = contrast(t('text'), t(k));
    const r2 = contrast(t('text-muted'), t(k));
    return `<div class="swatch"><div class="chip" style="background:${t(k)};color:${t('text')}"><span>Text</span><span style="color:${t('text-muted')}">secondary</span></div>
<div class="meta"><code>${k}</code><span class="hex">${t(k)}</span><span>text ${fmt(r1)}:1 ${check(r1, 4.5)}</span><span>text-muted ${fmt(r2)}:1 ${check(r2, 4.5)}</span></div></div>`;
  }).join('');
  const foregrounds = ['text', 'text-muted', 'brand', 'secondary', 'secret', 'success', 'error'].map((k) => `<div class="swatch"><div class="chip" style="background:${t(k)}"></div>
<div class="meta"><code>${k}</code><span class="hex">${t(k)}</span>${bgs.map((b) => {
    const r = contrast(t(k), t(b));
    return `<span>on ${b} ${fmt(r)}:1 ${check(r, 4.5)}</span>`;
  }).join('')}</div></div>`).join('');
  const pair = (fgK, bgK, sample) => {
    const r = contrast(t(fgK), t(bgK));
    return `<div class="swatch"><div class="chip chip-text" style="background:${t(bgK)};color:${t(fgK)}">${sample}</div><div class="meta"><code>${fgK}</code> on <code>${bgK}</code><span class="hex">${t(fgK)} / ${t(bgK)}</span><span>${fmt(r)}:1 ${check(r, 4.5)}</span></div></div>`;
  };
  const containers = [pair('on-brand', 'brand', 'Envoyer'), pair('brand', 'brand-container', 'Liens'), pair('secret', 'secret-container', '•••••• Afficher'), pair('text', 'secret-container', 'Contenu masqué')].join('');
  const borders = ['border-strong', 'border'].map((k) => `<div class="swatch"><div class="chip" style="background:${t('surface')};box-shadow:inset 0 0 0 3px ${t(k)}"></div><div class="meta"><code>${k}</code><span class="hex">${t(k)}</span>${['bg', 'surface'].map((b) => {
    const r = contrast(t(k), t(b));
    return k === 'border' ? `<span>on ${b} ${fmt(r)}:1 (decorative)</span>` : `<span>on ${b} ${fmt(r)}:1 (min 3) ${check(r, 3)}</span>`;
  }).join('')}</div></div>`).join('');
  return `<section class="swatch-block" style="background:${t('bg')};color:${t('text')}">
  <h4>${label} · backgrounds</h4><div class="swatch-grid">${backgrounds}</div>
  <h4>${label} · text and state colours</h4><div class="swatch-grid">${foregrounds}</div>
  <h4>${label} · pairs on containers</h4><div class="swatch-grid">${containers}</div>
  <h4>${label} · outlines</h4><div class="swatch-grid">${borders}</div>
</section>`;
}

function typeSpecimen() {
  const scale = [
    ['title', 'semibold', 'ui', 'Title · Feed'],
    ['subtitle', 'semibold', 'ui', 'Subtitle · PC-Bureau'],
    ['body', 'regular', 'ui', 'Body · Le lien reçu s’ouvre d’un tap ; l’aperçu reste en mémoire sept jours.'],
    ['label', 'medium', 'ui', 'Label · Envoyer · Coller · Supprimer partout'],
    ['caption', 'regular', 'ui', 'Caption · Lien · Téléphone · 14:02 · expire dans 7 j'],
    ['mono', 'regular', 'mono', 'https://exemple.org/partage · rapport-2026.pdf'],
  ];
  const rows = scale.map(([size, weight, fam, text]) => `<div class="type-row"><span class="type-spec">${size} · ${tok(`font.size.${size}`)}/${tok(`font.line.${size}`)} · ${tok(`font.weight.${weight}`)}</span><span style="font-family:var(--font-${fam});font-size:${tok(`font.size.${size}`)}px;line-height:${tok(`font.line.${size}`)}px;font-weight:${tok(`font.weight.${weight}`)}">${esc(text)}</span></div>`).join('');
  return `<div class="figure type-specimen">
  <div class="type-hero"><div style="font-weight:600;font-size:40px;line-height:46px">Inter · Aa Bb Œœ</div>
  <div class="accents">àâæçéèêëîïôœùûüÿ ÀÂÆÇÉÈÊËÎÏÔŒÙÛÜŸ « Guillemets » 0123456789</div>
  <div class="weights"><span style="font-weight:400">Regular 400</span><span style="font-weight:500">Medium 500</span><span style="font-weight:600">SemiBold 600</span></div></div>
  ${rows}
  <div class="mono-demo"><div class="mono-title">JetBrains Mono · revealed password and pairing code</div>
  <div class="password" style="font-family:var(--font-mono)"><span class="pw-label">Default</span> Kp7!xQ0o-Il1O0-5S$s</div>
  <div class="password" style="font-family:var(--font-mono);font-feature-settings:${cssFeature('font.feature.mono-password')}"><span class="pw-label">With "zero"</span> Kp7!xQ0o-Il1O0-5S$s</div>
  <div class="pairing" style="font-family:var(--font-mono);font-size:${tok('font.size.pairing-code')}px;font-weight:600;font-feature-settings:${cssFeature('font.feature.ui-digits')}">482 913</div></div>
</div>`;
}

const iconRows = [
  ['Link', 'link', 'text-muted'], ['Text', 'notes', 'text-muted'], ['Markdown', 'markdown', 'text-muted'], ['File', 'draft', 'text-muted'],
  ['Image', 'image', 'text-muted'], ['Hidden', 'visibility_off', 'secret'], ['Show', 'visibility', 'text'], ['Unread', 'fiber_manual_record_fill1', 'brand'],
  ['Sent', 'check', 'text-muted'], ['Delivered', 'done_all', 'success'], ['Waiting', 'schedule', 'secondary'], ['Expired', 'timer_off', 'text-muted'], ['Unreadable', 'error', 'error'],
];
async function icons() {
  const svgs = Object.fromEntries(await Promise.all(iconRows.map(async ([, n]) => [n, strip(await readFile(path.join(designDir, 'icons', `${n}.svg`), 'utf8'))])));
  const sized = (n, s) => svgs[n].replace(/<svg /, '<svg fill="currentColor" ').replace(/ height="24"/, ` height="${s}"`).replace(/ width="24"/, ` width="${s}"`);
  const block = (mode) => {
    const t = (k) => tok(`color.${mode}.${k}`);
    return `<div class="icon-grid" style="background:${t('bg')}">${iconRows.map(([label, n, c]) => `<div class="icon-cell" style="color:${t(c)}"><span>${sized(n, 24)}</span><span>${sized(n, 16)}</span><span class="icon-label" style="color:${t('text')}">${esc(label)}</span><code style="color:${t('text-muted')}">${n}</code></div>`).join('')}</div>`;
  };
  const card = (mode) => {
    const t = (k) => tok(`color.${mode}.${k}`);
    const ic = (n, c, s = 20) => `<span style="color:${t(c)};display:inline-flex">${sized(n, s)}</span>`;
    const logo = mode === 'light' ? lockupH(20, NIGHT) : lockupH(20, '#FFFFFF');
    return `<div class="mock" style="background:${t('bg')};color:${t('text')}">
  <div class="mock-top">${logo}<span style="color:${t('text-muted')};font-size:8pt">3 appareils</span></div>
  <div class="mock-card" style="background:${t('surface')};border:1px solid ${t('border')}"><div class="mock-head">${ic('link', 'text-muted')}<span class="mock-title">github.com/rsms/inter</span>${ic('fiber_manual_record_fill1', 'brand', 12)}</div><div class="mock-meta" style="color:${t('text-muted')}">Lien · Téléphone · 14:02 ${ic('done_all', 'success', 14)}</div></div>
  <div class="mock-card" style="background:${t('surface')};border:1px solid ${t('border')};border-left:${tok('border.width.secret-accent')}px solid ${t('secret')}"><div class="mock-head">${ic('visibility_off', 'secret')}<span class="mock-title" style="color:${t('secret')}">Masqué</span></div>
  <div class="mock-mask" style="background:${t('secret-container')}"><span style="font-family:var(--font-mono)">••••••••••</span><span class="mock-btn" style="color:${t('secret')}">Afficher</span></div><div class="mock-meta" style="color:${t('text-muted')}">PC-Bureau · 14:05 · expire dans 1 h</div></div>
  <div class="mock-banner" style="background:${t('surface-alt')}"><span>Envoyé</span><span style="color:${t('secondary')};font-weight:500">Annuler</span></div>
  <div class="mock-send" style="background:${t('brand')};color:${t('on-brand')}">Envoyer</div>
</div>`;
  };
  return `<div class="figure icons-figure"><p class="figcaption">Material Symbols Outlined, FILL 0, wght 400, at 24 then 16 px, in their usage colour.</p>${block('light')}${block('dark')}
<p class="figcaption">In context: header with the lockup at 20 px, feed card, hidden item, deferred-send banner, primary button.</p><div class="mocks">${card('light')}${card('dark')}</div></div>`;
}

function figInterdits() {
  const { cx, cy, R, w } = G.M;
  const L = tok('color.light.bg');
  const closed = `<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="${NIGHT}" stroke-width="${w}"/><path fill="${NIGHT}" d="${G.BEAD_D}"/>`;
  const round = `<path fill="none" stroke="${NIGHT}" stroke-width="${w}" stroke-linecap="round" d="M${G.geometryNotes.o0[0] + 12} ${G.geometryNotes.o0[1] + 9}A${R} ${R} 0 1 1 ${G.geometryNotes.o1[0] + 12} ${G.geometryNotes.o1[1] - 9}"/><path fill="${NIGHT}" d="${G.beadD(G.BEAD.x, G.BEAD.y, 14)}"/>`;
  const arrow = `${G.symbolInner(NIGHT).replace(/<path fill="[^"]*" d="M[^"]*"\/>$/, '')}<path fill="${NIGHT}" d="M44 66L104 50L92 110Z"/>`;
  const thick = `<circle cx="${cx}" cy="${cy}" r="${R}" fill="none" stroke="${NIGHT}" stroke-width="44" stroke-dasharray="${2 * Math.PI * R * 0.79} 999" transform="rotate(218 ${cx} ${cy})"/><path fill="${NIGHT}" d="${G.beadD(G.BEAD.x, G.BEAD.y, 20)}"/>`;
  const items = [
    ['Closed ring', svg256(closed, 74)],
    ['Rounded ends, smaller bead', svg256(round, 74)],
    ['Arrow head', svg256(arrow, 74)],
    ['Thicker ring', svg256(thick, 74)],
    ['Rotated', `<div style="transform:rotate(90deg)">${sym(NIGHT, 74)}</div>`],
    ['Gradient and shadow', svg256(`<defs><linearGradient id="gx" x1="0" x2="1"><stop offset="0" stop-color="#FFC85C"/><stop offset="1" stop-color="#2E4C80"/></linearGradient></defs>${G.symbolInner('url(#gx)')}`, 74, ' style="display:block;filter:drop-shadow(3px 4px 2px #0006)"')],
    ['Sun on a light background', sym(SUN, 74)],
    ['Re-set wordmark', `<span style="font:600 34px/1 Inter;color:${NIGHT};letter-spacing:-0.5px">Tomi</span>`],
  ];
  return `<div class="figure donts">${items.map(([l, s]) => `<div class="dont"><div class="dont-art" style="background:${L}">${s}<b class="no">×</b></div><small>${l}</small></div>`).join('')}</div>`;
}

// ---------- markdown ----------
const md = new MarkdownIt({ html: true, linkify: false, typographer: false });
let body = md.render(await readFile(path.join(tomiDir, 'brand-guidelines.md'), 'utf8'));
// Images are relative to design/tomi/; the HTML lives in design/build/.
body = body.replace(/src="proofs\//g, 'src="../tomi/proofs/');
const replacements = {
  symbole: figSymbole(),
  construction: figConstruction(),
  grilles: figGrilles(),
  animation: figAnimation(),
  assemblages: figAssemblages(),
  identite: figIdentite(),
  swatches: swatches('light') + swatches('dark'),
  'type-specimen': typeSpecimen(),
  icons: await icons(),
  interdits: figInterdits(),
};
for (const [key, html] of Object.entries(replacements)) {
  const marker = `<!-- pdf:${key} -->`;
  if (!body.includes(marker)) throw new Error(`Marker ${marker} missing from brand-guidelines.md`);
  body = body.replace(marker, html);
}
// Sections 2 to 9 start on a new page; 1 follows the intro note, 10 follows 9.
body = body.replace(/<h2>(\d+)\./g, (m, n) => (Number(n) >= 2 && Number(n) <= 9 ? `<h2 class="section">${n}.` : m));

const today = new Date().toISOString().slice(0, 10);
const css = `
@font-face { font-family: "Inter"; src: url("../fonts/Inter-Variable.ttf") format("truetype"); font-weight: 100 900; }
@font-face { font-family: "JetBrains Mono"; src: url("../fonts/JetBrainsMono-Variable.ttf") format("truetype"); font-weight: 100 800; }
:root { --font-ui: "Inter", sans-serif; --font-mono: "JetBrains Mono", monospace;
  --text: ${tok('color.light.text')}; --muted: ${tok('color.light.text-muted')}; --brand: ${tok('color.light.brand')};
  --border: ${tok('color.light.border')}; --alt: ${tok('color.light.surface-alt')}; --bg: ${tok('color.light.bg')}; }
@page { size: A4; margin: 20mm 16mm 18mm 16mm;
  @top-left { content: "Tomi · Brand guidelines"; font: 500 8pt "Inter"; color: ${tok('color.light.text-muted')}; }
  @top-right { content: "${today}"; font: 400 8pt "Inter"; color: ${tok('color.light.text-muted')}; }
  @bottom-right { content: counter(page) " / " counter(pages); font: 400 8pt "Inter"; color: ${tok('color.light.text-muted')}; } }
@page :first { margin: 0; @top-left { content: none; } @top-right { content: none; } @bottom-right { content: none; } }
* { box-sizing: border-box; }
html { -webkit-print-color-adjust: exact; print-color-adjust: exact; }
body { margin: 0; font-family: var(--font-ui); font-size: 9.5pt; line-height: 1.5; color: var(--text); }
h1 { display: none; }
h2, h3, h4 { font-weight: 600; line-height: 1.25; break-after: avoid; }
h2 { font-size: 18pt; margin: 0 0 6mm; padding-bottom: 2mm; border-bottom: 1px solid var(--border); color: ${NIGHT}; }
h2.section { break-before: page; }
h3 { font-size: 12pt; margin: 7mm 0 2mm; }
h4 { font-size: 9.5pt; margin: 4mm 0 2mm; }
p, li { orphans: 3; widows: 3; }
ul { padding-left: 5mm; }
a { color: var(--brand); text-decoration: none; }
code { font-family: var(--font-mono); font-size: 0.9em; }
blockquote { margin: 0 0 5mm; padding: 2mm 4mm; border-left: 3px solid ${NIGHT}; background: ${tok('color.light.brand-container')}; }
blockquote p { margin: 1mm 0; }
table { width: 100%; border-collapse: collapse; margin: 2mm 0 4mm; font-size: 8.5pt; }
th, td { text-align: left; padding: 1.2mm 2mm; border-bottom: 1px solid var(--border); vertical-align: top; }
th { font-weight: 600; background: var(--alt); }
tr { break-inside: avoid; }
td code { white-space: nowrap; }
.figure, .swatch-block, .swatch, .icon-grid, .mock, .type-row, .mono-demo, .lk-row, .dont, .grid-block { break-inside: avoid; }
.figcaption { font-size: 8pt; color: var(--muted); margin: 2mm 0; }
img { display: block; max-width: 100%; max-height: 230mm; margin: 2mm 0; border: 1px solid var(--border); border-radius: 1mm; image-rendering: pixelated; break-inside: avoid; }
small { font-size: 7pt; color: var(--muted); }
.no { position: absolute; right: 2mm; top: 1mm; color: #B3261E; font-size: 12pt; }

/* cover */
.cover { width: 210mm; height: 296mm; background: ${NIGHT}; color: #fff; display: flex; flex-direction: column; justify-content: center; padding: 0 24mm; gap: 12mm; break-after: page; }
.cover .t { font-size: 24pt; font-weight: 600; }
.cover .s { font-size: 11pt; color: #C9D2E3; max-width: 130mm; }
.cover .m { font-size: 8.5pt; color: #9AA6BD; }

/* symbol */
.sym-row { display: flex; gap: 3mm; margin-bottom: 3mm; }
.sym-cell { flex: 1; display: flex; justify-content: center; padding: 6mm 0; border-radius: 2mm; border: 1px solid var(--border); }
.sizes { flex: 1; display: flex; align-items: flex-end; justify-content: space-around; padding: 5mm 4mm 3mm; border-radius: 2mm; border: 1px solid var(--border); }
.sz { display: inline-flex; flex-direction: column; align-items: center; gap: 1.5mm; }
.construction svg { display: block; margin: 0 auto; }
.grilles { display: flex; gap: 6mm; align-items: flex-start; }
.grid-block { display: flex; flex-direction: column; gap: 2mm; }
.pixel-grid { display: grid; gap: 0.3mm; background: var(--border); padding: 0.3mm; width: max-content; }
.pixel-grid i { background: #fff; }
.pixel-grid i.on { background: ${NIGHT}; }
.grid-real { display: flex; gap: 2mm; }
.grid-real span { display: inline-flex; padding: 2mm; border-radius: 1mm; border: 1px solid var(--border); }
.anim { display: flex; gap: 2.5mm; justify-content: space-between; }
.frame { display: flex; flex-direction: column; align-items: center; gap: 1mm; }

/* lockups */
.lk-row { display: flex; gap: 3mm; margin-bottom: 3mm; align-items: center; }
.lk { flex: 1; display: flex; align-items: center; justify-content: center; padding: 7mm 5mm; border-radius: 2mm; }
.clear { display: inline-block; }

/* identity */
.ident { display: flex; gap: 4mm; align-items: stretch; }
.id-chip { flex: 1; display: flex; flex-direction: column; }
.id-sw { height: 26mm; border-radius: 2mm; display: flex; align-items: flex-end; padding: 3mm; font-weight: 600; font-size: 12pt; }
.id-meta { display: flex; flex-direction: column; font-size: 7.5pt; padding-top: 1.5mm; }
.hex { font-family: var(--font-mono); font-size: 8pt; font-weight: 600; }
.id-demo { display: flex; flex-direction: column; gap: 2mm; }
.id-demo > div { padding: 2mm; border-radius: 2mm; border: 1px solid var(--border); }

/* swatches */
.swatch-block { padding: 4mm 5mm 5mm; border-radius: 3mm; margin: 0 0 4mm; }
.swatch-block h4:first-child { margin-top: 0; }
.swatch-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 3mm; }
.swatch .chip { height: 15mm; border-radius: 2mm; display: flex; flex-direction: column; justify-content: center; padding: 0 3mm; font-size: 9pt; font-weight: 500; }
.swatch .chip-text { font-size: 10pt; }
.swatch .meta { display: flex; flex-direction: column; font-size: 7.5pt; line-height: 1.45; padding-top: 1.5mm; }
.swatch .meta code { font-size: 7.5pt; font-weight: 500; }
.ok { color: inherit; } .ko { color: #B3261E; }

/* type */
.type-specimen { border: 1px solid var(--border); border-radius: 3mm; padding: 5mm 6mm; margin: 3mm 0 5mm; }
.type-hero { padding-bottom: 3mm; border-bottom: 1px solid var(--border); margin-bottom: 3mm; }
.accents { font-size: 11pt; margin: 2mm 0; }
.weights { display: flex; gap: 8mm; font-size: 11pt; }
.type-row { display: flex; align-items: baseline; gap: 5mm; padding: 1.6mm 0; border-bottom: 1px dashed var(--border); }
.type-spec { width: 36mm; flex: none; font-size: 7.5pt; color: var(--muted); font-family: var(--font-mono); }
.mono-demo { margin-top: 4mm; padding: 4mm; background: var(--alt); border-radius: 2mm; }
.mono-title { font-weight: 600; margin-bottom: 2mm; }
.password { font-size: 18px; line-height: 28px; display: flex; align-items: baseline; gap: 4mm; }
.pw-label { font-family: var(--font-ui); font-size: 7.5pt; color: var(--muted); width: 30mm; flex: none; }
.pairing { margin-top: 3mm; letter-spacing: 0.08em; }

/* icons */
.icon-grid { display: grid; grid-template-columns: repeat(7, 1fr); gap: 2mm; padding: 4mm; border-radius: 3mm; margin-bottom: 3mm; }
.icon-cell { display: flex; flex-direction: column; align-items: center; gap: 1mm; text-align: center; }
.icon-cell span { display: inline-flex; }
.icon-label { font-size: 8pt; font-weight: 500; }
.icon-cell code { font-size: 6.5pt; word-break: break-all; }
.mocks { display: flex; gap: 3mm; }
.mock { flex: 1; padding: 4mm; border-radius: 3mm; display: flex; flex-direction: column; gap: 2.5mm; font-size: 10pt; }
.mock-top { display: flex; justify-content: space-between; align-items: center; padding-bottom: 1mm; }
.mock-card { border-radius: 2mm; padding: 3mm; display: flex; flex-direction: column; gap: 1.5mm; }
.mock-head { display: flex; align-items: center; gap: 2mm; }
.mock-title { flex: 1; font-weight: 500; }
.mock-meta { font-size: 8pt; display: flex; align-items: center; gap: 1.5mm; }
.mock-mask { display: flex; justify-content: space-between; align-items: center; padding: 2mm 3mm; border-radius: 1.5mm; }
.mock-btn { font-weight: 500; font-size: 9pt; }
.mock-banner { display: flex; justify-content: space-between; padding: 2mm 3mm; border-radius: 1.5mm; font-size: 9pt; }
.mock-send { align-self: flex-end; padding: 2mm 6mm; border-radius: 1.5mm; font-weight: 500; font-size: 9.5pt; }

/* don'ts */
.donts { display: grid; grid-template-columns: repeat(4, 1fr); gap: 3mm; }
.dont { display: flex; flex-direction: column; gap: 1mm; }
.dont-art { position: relative; height: 30mm; display: flex; align-items: center; justify-content: center; border-radius: 2mm; border: 1px solid var(--border); }
`;

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Tomi · Brand guidelines</title><style>${css}</style></head>
<body>
<section class="cover">
  ${lockupH(150, SUN, '#FFFFFF')}
  <div class="t">Brand guidelines</div>
  <div class="s">Symbol, wordmark, colours, typography, icons and tone of Tomi, the app for sending things to yourself between Windows and Android.</div>
  <div class="m">Final version · generated on ${today} from design/tomi/brand-guidelines.md, tokens.json and design/build/tomi-geometry.mjs</div>
</section>
${body}
</body></html>`;
await writeFile(htmlPath, html, 'utf8');
console.log(`HTML written: ${htmlPath}`);

const edge = [process.env.EDGE_PATH, 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe', 'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe'].filter(Boolean).find((p) => existsSync(p));
if (!edge) throw new Error('Microsoft Edge not found; set EDGE_PATH');
const profile = await mkdtemp(path.join(os.tmpdir(), 'tomi-edge-'));
try {
  const r = spawnSync(edge, ['--headless', '--disable-gpu', '--no-first-run', '--no-pdf-header-footer', '--virtual-time-budget=5000', `--user-data-dir=${profile}`, `--print-to-pdf=${pdfPath}`, pathToFileURL(htmlPath).href], { encoding: 'utf8', timeout: 300000 });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`Edge exited with ${r.status}: ${r.stderr}`);
} finally {
  await rm(profile, { recursive: true, force: true }).catch(() => {});
}
const { size } = await stat(pdfPath);
console.log(`PDF written: ${pdfPath} (${Math.round(size / 1024)} KB)`);
