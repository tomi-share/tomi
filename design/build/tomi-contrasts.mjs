// Tomi: WCAG 2.x contrast ratios of every documented colour pair, computed from design/tomi/tokens.json.
// Prints Markdown tables (luminances, light pairs, dark pairs, identity pairs) to paste into the guidelines,
// and exits with code 1 if a pair is below its threshold.
// Exported for tomi-build-pdf.mjs: luminance(), contrast(), PAIRS.
// Usage: cd design/build && node tomi-contrasts.mjs

import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

export const channel = (c8) => {
  const s = c8 / 255;
  return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
};
export const linear = (hex) => [1, 3, 5].map((i) => channel(parseInt(hex.slice(i, i + 2), 16)));
export const luminance = (hex) => {
  const [r, g, b] = linear(hex);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
export const contrast = (a, b) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

// [foreground suffix, background suffix, minimum] per mode; 4.5 = text, 3 = UI component or graphic, 0 = decorative.
const MODE_PAIRS = [
  ['text', 'bg', 4.5], ['text', 'surface', 4.5], ['text', 'surface-alt', 4.5],
  ['text-muted', 'bg', 4.5], ['text-muted', 'surface', 4.5], ['text-muted', 'surface-alt', 4.5],
  ['brand', 'bg', 4.5], ['brand', 'surface', 4.5], ['brand', 'surface-alt', 4.5],
  ['on-brand', 'brand', 4.5], ['brand', 'brand-container', 4.5], ['text', 'brand-container', 4.5],
  ['secondary', 'bg', 4.5], ['secondary', 'surface', 4.5], ['secondary', 'surface-alt', 4.5],
  ['secret', 'bg', 4.5], ['secret', 'surface', 4.5], ['secret', 'surface-alt', 4.5],
  ['secret', 'secret-container', 4.5], ['text', 'secret-container', 4.5],
  ['success', 'bg', 4.5], ['success', 'surface', 4.5], ['success', 'surface-alt', 4.5],
  ['error', 'bg', 4.5], ['error', 'surface', 4.5], ['error', 'surface-alt', 4.5],
  ['border-strong', 'bg', 3], ['border-strong', 'surface', 3],
  ['border', 'bg', 0], ['border', 'surface', 0],
];
export const PAIRS = {
  light: MODE_PAIRS.map(([a, b, m]) => [`color.light.${a}`, `color.light.${b}`, m]),
  dark: MODE_PAIRS.map(([a, b, m]) => [`color.dark.${a}`, `color.dark.${b}`, m]),
  // Identity: the symbol is a graphic (3:1, WCAG 1.4.11); the white wordmark on night is large text (4.5 kept).
  identity: [
    ['color.identity.sun', 'color.identity.night', 3],
    ['#FFFFFF', 'color.identity.night', 4.5],
    ['color.identity.night', 'color.light.bg', 3],
    ['color.identity.night', 'color.light.surface', 3],
    ['color.identity.night', 'color.dark.bg', 0],
    ['color.identity.sun', 'color.light.bg', 0],
    ['color.dark.brand', 'color.dark.bg', 3],
  ],
};

export async function loadTokens() {
  return JSON.parse(await readFile(path.resolve(here, '..', 'tomi', 'tokens.json'), 'utf8'));
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const tokens = await loadTokens();
  const val = (n) => {
    if (n.startsWith('#')) return n;
    if (!tokens[n]) throw new Error(`Unknown token: ${n}`);
    return tokens[n].value;
  };
  const short = (n) => (n.startsWith('#') ? 'white' : n.replace(/^color\./, ''));
  const d4 = (x) => x.toFixed(4);
  let failed = 0;

  const seen = new Set();
  const lum = [];
  for (const list of Object.values(PAIRS)) for (const p of list) for (const n of [p[0], p[1]]) if (!seen.has(n) && !n.startsWith('#')) {
    seen.add(n);
    const hex = val(n);
    const [r, g, b] = linear(hex);
    lum.push(`| ${short(n)} | ${hex} | ${d4(r)} | ${d4(g)} | ${d4(b)} | ${d4(luminance(hex))} |`);
  }
  console.log('**Relative luminances**\n\n| Token | Hex | R_lin | G_lin | B_lin | L |\n|---|---|---|---|---|---|');
  console.log(lum.join('\n'));

  const titles = { light: 'Contrast, light mode', dark: 'Contrast, dark mode', identity: 'Contrast, identity' };
  for (const [key, list] of Object.entries(PAIRS)) {
    console.log(`\n**${titles[key]}**\n\n| Foreground | Background | Calculation | Ratio | Threshold | Verdict |\n|---|---|---|---|---|---|`);
    for (const [fgN, bgN, min] of list) {
      const fg = val(fgN);
      const bg = val(bgN);
      const [hi, lo] = [luminance(fg), luminance(bg)].sort((x, y) => y - x);
      const r = (hi + 0.05) / (lo + 0.05);
      let verdict;
      if (min === 0) verdict = key === 'identity' ? 'forbidden' : 'decorative';
      else if (r >= min) verdict = r < min + 0.3 ? 'OK (thin margin)' : 'OK';
      else {
        verdict = 'FAIL';
        failed += 1;
      }
      const label = (n) => (key === 'identity' ? short(n) : short(n).replace(/^(light|dark)\./, ''));
      console.log(`| ${label(fgN)} ${fg} | ${label(bgN)} ${bg} | (${d4(hi)}+0.05)/(${d4(lo)}+0.05) | ${r.toFixed(2)}:1 | ${min || '—'} | ${verdict} |`);
    }
  }
  if (failed) {
    console.error(`FAILED: ${failed} pair(s) below threshold`);
    process.exit(1);
  }
  console.error('OK: all pairs meet their threshold');
}
