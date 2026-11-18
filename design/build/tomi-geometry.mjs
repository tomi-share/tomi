// Tomi identity: single source of the geometry (the "loop" symbol, pixel-grid variants, wordmark,
// lockups, app tile). Imported by tomi-export.mjs (files), tomi-proofs.mjs (renders) and
// tomi-build-pdf.mjs (guidelines). No side effects.

const f = (n) => Number(n.toFixed(2));
const pt = (cx, cy, r, deg) => [f(cx + r * Math.cos((deg * Math.PI) / 180)), f(cy + r * Math.sin((deg * Math.PI) / 180))];

// ---------------- identity colours (mirrored in design/tomi/tokens.json, checked there) ----------------
export const NIGHT = '#24324A';
export const SUN = '#FFC85C';

// ---------------- master symbol, viewBox 0 0 256 256 ----------------
// An open ring and a bead sitting in its opening, on the ring's centreline: the bead goes round and comes
// back to its place. The bead is wider than the ring (64 vs 24) so it reads as an object, not as the end of
// an arc; the cut ends are square (no arrow head, no round cap) so it does not read as a "sync" icon or a spinner.
export const M = { cx: 136, cy: 128, R: 82, w: 24, br: 32, clear: 38 };
const Ro = M.R + M.w / 2; // 94
const Ri = M.R - M.w / 2; // 70
export const BEAD = { x: M.cx - M.R, y: M.cy }; // (54, 128)
const a0 = 180 + M.clear; // start of the ring, above the bead
const a1 = 180 - M.clear; // end of the ring, below the bead
const o0 = pt(M.cx, M.cy, Ro, a0);
const o1 = pt(M.cx, M.cy, Ro, a1);
const i0 = pt(M.cx, M.cy, Ri, a0);
const i1 = pt(M.cx, M.cy, Ri, a1);
export const RING_D = `M${o0[0]} ${o0[1]}A${Ro} ${Ro} 0 1 1 ${o1[0]} ${o1[1]}L${i1[0]} ${i1[1]}A${Ri} ${Ri} 0 1 0 ${i0[0]} ${i0[1]}Z`;
export const beadD = (x = BEAD.x, y = BEAD.y, r = M.br) => `M${f(x - r)} ${f(y)}A${r} ${r} 0 1 0 ${f(x + r)} ${f(y)}A${r} ${r} 0 1 0 ${f(x - r)} ${f(y)}Z`;
export const BEAD_D = beadD();
// Bounding box of the symbol in its 256 grid.
export const SYM_BOX = { x0: BEAD.x - M.br, y0: M.cy - Ro, x1: M.cx + Ro, y1: M.cy + Ro }; // 22 34 230 222
// Clearance between the bead and the ring ends (closest point = inner corner of a cut end).
export const GAP_UNITS = f(Math.hypot(i0[0] - BEAD.x, i0[1] - BEAD.y) - M.br);
export const geometryNotes = { Ro, Ri, a0, a1, o0, o1, i0, i1 };

// Symbol paths in user units, optionally recoloured. rot rotates the bead around the ring centre (animation).
export const symbolInner = (color = 'currentColor', rot = 0) =>
  `<path fill="${color}" d="${RING_D}"/><path fill="${color}"${rot ? ` transform="rotate(${rot} ${M.cx} ${M.cy})"` : ''} d="${BEAD_D}"/>`;

// ---------------- pixel-grid variants ('#' = on) ----------------
// 16 px: 12 px ring, 2 px stroke, 4 px bead, 1 px gap around the bead. Used at 16 px only.
export const GRID16 = [
  '................',
  '................',
  '......######....',
  '.....########...',
  '.....##....###..',
  '............###.',
  '...##........##.',
  '..####.......##.',
  '..####.......##.',
  '...##........##.',
  '............###.',
  '.....##....###..',
  '.....########...',
  '......######....',
  '................',
  '................',
];
// 20 px: 15 px ring, 2 px stroke, 5 px bead, 1 px gap around the bead. Used at 20 px (tray at 125 %).
export const GRID20 = [
  '....................',
  '....................',
  '....................',
  '........#####.......',
  '......#########.....',
  '.....###.....###....',
  '.....##.......###...',
  '...............##...',
  '...###..........##..',
  '..#####.........##..',
  '..#####.........##..',
  '..#####.........##..',
  '...###..........##..',
  '...............##...',
  '.....##.......###...',
  '.....###.....###....',
  '......#########.....',
  '........#####.......',
  '....................',
  '....................',
];
// 24 px: 18 px ring, 2 px stroke, 6 px bead, 2 px gap around the bead. Used at 24 px.
export const GRID24 = [
  '........................',
  '........................',
  '........................',
  '..........######........',
  '........##########......',
  '.......###......###.....',
  '......###........###....',
  '..................###...',
  '...................##...',
  '...####.............##..',
  '..######............##..',
  '..######............##..',
  '..######............##..',
  '..######............##..',
  '...####.............##..',
  '...................##...',
  '..................###...',
  '......###........###....',
  '.......###......###.....',
  '........##########......',
  '..........######........',
  '........................',
  '........................',
  '........................',
];
// Symbol for the 16 px app tile: 10 px ring, 2 px stroke, 4 px bead (fits inside a 16 px tile).
export const TILE16_SYM = [
  '................',
  '................',
  '................',
  '.......####.....',
  '.....########...',
  '...........##...',
  '....##......##..',
  '...####.....##..',
  '...####.....##..',
  '....##......##..',
  '...........##...',
  '.....########...',
  '.......####.....',
  '................',
  '................',
  '................',
];

export function checkGrid(rows, n, name) {
  if (rows.length !== n || rows.some((l) => l.length !== n || /[^#.]/.test(l))) throw new Error(`${name}: grid must be ${n} lines of ${n} chars among "#" and "."`);
}
checkGrid(GRID16, 16, 'GRID16');
checkGrid(GRID20, 20, 'GRID20');
checkGrid(GRID24, 24, 'GRID24');
checkGrid(TILE16_SYM, 16, 'TILE16_SYM');

// Horizontal runs of '#' as one path of 1-px-high rectangles (integer coordinates).
export const gridD = (rows, dx = 0, dy = 0) => {
  let d = '';
  rows.forEach((line, y) => {
    for (const m of line.matchAll(/#+/g)) d += `M${m.index + dx} ${y + dy}h${m[0].length}v1h-${m[0].length}z`;
  });
  return d;
};

// ---------------- app tile ----------------
export const TILE_RADIUS = 0.22; // corner radius as a fraction of the side (56 on 256)
export const TILE_SCALE = 0.7; // symbol scale inside the 256 tile
// Symbol centred in the tile on its bounding box.
const tileTx = f(128 - ((SYM_BOX.x0 + SYM_BOX.x1) / 2) * TILE_SCALE);
const tileTy = f(128 - ((SYM_BOX.y0 + SYM_BOX.y1) / 2) * TILE_SCALE);
export const TILE_SYM_TRANSFORM = `translate(${tileTx} ${tileTy}) scale(${TILE_SCALE})`;
export const tileInner = (bg = NIGHT, fg = SUN, rot = 0) =>
  `<rect width="256" height="256" rx="${256 * TILE_RADIUS}" fill="${bg}"/><g transform="${TILE_SYM_TRANSFORM}">${symbolInner(fg, rot)}</g>`;

// Pixel tile (n = 16, 20, 24): rounded square mask tested at pixel centres, symbol grid on top.
export function pixelTile(n) {
  const r = TILE_RADIUS * n;
  const inside = (c, y) => {
    const px = c + 0.5;
    const py = y + 0.5;
    const qx = Math.min(Math.max(px, r), n - r);
    const qy = Math.min(Math.max(py, r), n - r);
    return Math.hypot(px - qx, py - qy) <= r;
  };
  const bg = [];
  for (let y = 0; y < n; y += 1) {
    let s = '';
    for (let c = 0; c < n; c += 1) s += inside(c, y) ? '#' : '.';
    bg.push(s);
  }
  // symbol: TILE16_SYM at 16; GRID16 offset to centre at 20 and 24
  let sym;
  if (n === 16) sym = TILE16_SYM;
  else {
    const off = n === 20 ? [2, 2] : [4, 4];
    sym = Array.from({ length: n }, (_, y) => {
      let s = '';
      for (let c = 0; c < n; c += 1) {
        const gy = y - off[1];
        const gx = c - off[0];
        s += gy >= 0 && gy < 16 && gx >= 0 && gx < 16 && GRID16[gy][gx] === '#' ? '#' : '.';
      }
      return s;
    });
  }
  // the symbol must sit fully inside the tile with at least 1 px of tile around it
  sym.forEach((line, y) => [...line].forEach((ch, c) => {
    if (ch !== '#') return;
    for (const [dx, dy] of [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]]) {
      const yy = y + dy;
      const cc = c + dx;
      if (yy < 0 || yy >= n || cc < 0 || cc >= n || bg[yy][cc] !== '#') throw new Error(`pixel tile ${n}: symbol pixel ${c},${y} has no tile margin`);
    }
  }));
  // background minus symbol, so each pixel has exactly one colour
  const bgOnly = bg.map((line, y) => [...line].map((ch, c) => (ch === '#' && sym[y][c] !== '#' ? '#' : '.')).join(''));
  return { bg: bgOnly, sym };
}

// ---------------- wordmark "tomi", geometric, drawn (no font) ----------------
// x-height 100 (y 100 to 200, baseline 200), stroke 22, round letters overshoot 2. Box x 0..412, y 52..202.
// One outline per letter part (no self-overlap, no abutting edges, so no anti-aliasing seam).
export const WORD_BOX = { x0: 0, y0: 52, x1: 412, y1: 202 };
const ringD = (cx, cy, R, r) => `M${cx - R} ${cy}A${R} ${R} 0 1 0 ${cx + R} ${cy}A${R} ${R} 0 1 0 ${cx - R} ${cy}Z M${cx - r} ${cy}A${r} ${r} 0 1 1 ${cx + r} ${cy}A${r} ${r} 0 1 1 ${cx - r} ${cy}Z`;
export const WORD_PATHS = [
  // t: stem with a rounded foot (echo of the ring) and crossbar on the x-height, one outline
  { d: 'M14 62V100H0V122H14V164A36 36 0 0 0 50 200H66V178H50A14 14 0 0 1 36 164V122H62V100H36V62Z' },
  // o: full ring (the symbol's ring is the open one)
  { d: ringD(138, 150, 52, 30), evenOdd: true },
  // m: square-topped first stem, two arches (outer R 44, inner R 22, centres x 256 and 322, y 144), one
  // outline so no seam appears where the arches meet the stems
  { d: 'M212 200V100H234V105.89A44 44 0 0 1 289 114.9A44 44 0 0 1 366 144V200H344V144A22 22 0 0 0 300 144V200H278V144A22 22 0 0 0 234 144V200Z' },
  // i: stem and round dot (same round as the bead)
  { d: 'M390 100H412V200H390Z' },
  { d: beadD(401, 66, 14) },
];
export const wordInner = (color = 'currentColor') =>
  WORD_PATHS.map((p) => `<path fill="${color}"${p.evenOdd ? ' fill-rule="evenodd"' : ''} d="${p.d}"/>`).join('');

// ---------------- lockups ----------------
// Horizontal: symbol at 0.8 (ring 150 high, 1.5 x-height), centred on the middle of the x-height, gap 56.
export const LOCKUP_H = (() => {
  const s = 0.8;
  const gap = 56;
  const tx = f(-SYM_BOX.x0 * s);
  const ty = f(150 - M.cy * s);
  const symW = (SYM_BOX.x1 - SYM_BOX.x0) * s;
  const wx = f(symW + gap);
  const y0 = Math.min(WORD_BOX.y0, ty + SYM_BOX.y0 * s);
  const y1 = Math.max(WORD_BOX.y1, ty + SYM_BOX.y1 * s);
  return { s, gap, tx, ty, wx, w: f(wx + WORD_BOX.x1), y0: f(y0), y1: f(y1) };
})();
export const lockupHInner = (symColor = 'currentColor', wordColor = symColor) =>
  `<g transform="translate(${LOCKUP_H.tx} ${LOCKUP_H.ty}) scale(${LOCKUP_H.s})">${symbolInner(symColor)}</g><g transform="translate(${LOCKUP_H.wx} 0)">${wordInner(wordColor)}</g>`;
export const lockupHViewBox = `0 ${LOCKUP_H.y0} ${LOCKUP_H.w} ${f(LOCKUP_H.y1 - LOCKUP_H.y0)}`;

// Stacked: symbol at 1.0 above, gap 44, wordmark centred under it. Width = wordmark width.
export const LOCKUP_V = (() => {
  const symW = SYM_BOX.x1 - SYM_BOX.x0;
  const W = WORD_BOX.x1 - WORD_BOX.x0;
  const tx = f((W - symW) / 2 - SYM_BOX.x0);
  const ty = 0;
  const gap = 44;
  const wy = f(SYM_BOX.y1 + gap - WORD_BOX.y0);
  return { tx, ty, wy, w: W, y0: SYM_BOX.y0, y1: f(wy + WORD_BOX.y1) };
})();
export const lockupVInner = (symColor = 'currentColor', wordColor = symColor) =>
  `<g transform="translate(${LOCKUP_V.tx} 0)">${symbolInner(symColor)}</g><g transform="translate(0 ${LOCKUP_V.wy})">${wordInner(wordColor)}</g>`;
export const lockupVViewBox = `0 ${LOCKUP_V.y0} ${LOCKUP_V.w} ${f(LOCKUP_V.y1 - LOCKUP_V.y0)}`;

// ---------------- animation (one lap of the bead, then rest) ----------------
export const ANIM = { lapMs: 1200, restMs: 1200, easing: 'cubic-bezier(0.65, 0, 0.35, 1)', spline: '0.65 0 0.35 1' };
// CSS animation (not SMIL) so that prefers-reduced-motion really stops it.
export const animatedSymbolSvg = (color = 'currentColor') => {
  const dur = (ANIM.lapMs + ANIM.restMs) / 1000;
  const k = Math.round((ANIM.lapMs / (ANIM.lapMs + ANIM.restMs)) * 100);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256" role="img" aria-label="Tomi">
<style>
.tomi-bead { transform-box: view-box; transform-origin: ${M.cx}px ${M.cy}px; animation: tomi-lap ${dur}s ${ANIM.easing} infinite; }
@keyframes tomi-lap { 0% { transform: rotate(0deg); } ${k}%, 100% { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .tomi-bead { animation: none; } }
</style>
<path fill="${color}" d="${RING_D}"/>
<path class="tomi-bead" fill="${color}" d="${BEAD_D}"/>
</svg>`;
};

// ---------------- generic look-alikes for confusion tests (drawn here, not copies of an icon set) ----------------
export const LOOKALIKES = {
  sync: (c) => `<g fill="none" stroke="${c}" stroke-width="24"><path d="M196 128A68 68 0 0 1 72 168"/><path d="M60 128A68 68 0 0 1 184 88"/></g><path fill="${c}" d="M196 56V112H140Z"/><path fill="${c}" d="M60 200V144H116Z"/>`,
  refresh: (c) => `<path fill="none" stroke="${c}" stroke-width="26" d="M200 128A72 72 0 1 1 168 68"/><path fill="${c}" d="M140 40H208V108Z"/>`,
  spinner: (c) => `<path fill="none" stroke="${c}" stroke-width="28" stroke-linecap="round" d="M128 46A82 82 0 1 1 46 128"/>`,
  power: (c) => `<g fill="none" stroke="${c}" stroke-width="26" stroke-linecap="round"><path d="M86 70A80 80 0 1 0 170 70"/><path d="M128 40V128"/></g>`,
};
