// Checks the guidelines PDF (default: design/tomi/brand-guidelines.pdf, generated locally by tomi-build-pdf.mjs):
// size, page count, embedded fonts (Inter and JetBrains Mono must be embedded, i.e. no silent system fallback),
// then renders every page to design/build/apercu/page-NN.png so that a human or an agent can look at them.
// Usage: cd design/build && node verify-pdf.mjs [path/to/file.pdf]
//   default: ../tomi/brand-guidelines.pdf, pages in apercu/ ; with a path: pages in apercu/<name of the PDF's folder>/

import { readFile, writeFile, mkdir, rm } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { pdf } from 'pdf-to-img';

const here = path.dirname(fileURLToPath(import.meta.url));
const pdfPath = process.argv[2] ? path.resolve(process.argv[2]) : path.resolve(here, '..', 'tomi', 'brand-guidelines.pdf');
const outDir = process.argv[2] ? path.join(here, 'apercu', path.basename(path.dirname(pdfPath))) : path.join(here, 'apercu');

const bytes = await readFile(pdfPath);
const sizeKb = Math.round(bytes.length / 1024);
const failures = [];
if (bytes.length < 20 * 1024) failures.push(`PDF too small: ${sizeKb} KB`);

// Font names appear in clear in FontDescriptor / BaseFont entries (subset prefix ABCDEF+).
const latin1 = bytes.toString('latin1');
const fonts = [...new Set([...latin1.matchAll(/\/(?:BaseFont|FontName)\s*\/([A-Za-z0-9+_-]+)/g)].map((m) => m[1]))];
console.log(`PDF: ${pdfPath} (${sizeKb} KB)`);
console.log(`Embedded fonts: ${fonts.join(', ') || '(none found)'}`);
if (!fonts.some((f) => /Inter/i.test(f))) failures.push('Inter is not embedded');
if (!fonts.some((f) => /JetBrains/i.test(f))) failures.push('JetBrains Mono is not embedded');
const fallbacks = fonts.filter((f) => /Segoe|Arial|Times|Calibri|Consolas|Roboto|DejaVu/i.test(f));
if (fallbacks.length) failures.push(`System fallback fonts embedded: ${fallbacks.join(', ')}`);

await rm(outDir, { recursive: true, force: true });
await mkdir(outDir, { recursive: true });
const doc = await pdf(pdfPath, { scale: 1.5 });
let page = 0;
for await (const image of doc) {
  page += 1;
  const file = path.join(outDir, `page-${String(page).padStart(2, '0')}.png`);
  await writeFile(file, image);
}
console.log(`Pages: ${page} rendered to ${outDir}`);
if (page === 0) failures.push('No page rendered');

if (failures.length) {
  console.error(`FAILED:\n- ${failures.join('\n- ')}`);
  process.exit(1);
}
console.log('OK');
