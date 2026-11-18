//! Files that the demo (`runtime::demo`) generates at its opening: images (BMP), sound (WAV),
//! documents (PDF, Markdown, long text), data (CSV, JSON, log), source code, opaque binary and
//! a ZIP archive stored without compression. Nothing is bundled with the apps: every byte is
//! computed here, deterministically (fixed seeds), with the standard library only.

use std::borrow::Cow;
use std::fmt::Write as _;

use crate::item::Thumbnail;

const DAY_MS: i64 = 86_400_000;

/// Small deterministic pseudo-random generator (SplitMix64): the same seed gives the same demo.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (0 when `n` is 0).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// Uniform in `lo..hi` (`lo` when the range is empty).
    pub(crate) fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = u64::try_from(hi.saturating_sub(lo)).unwrap_or(0);
        lo.saturating_add(i64::try_from(self.below(span)).unwrap_or(0))
    }

    pub(crate) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// One element of a non-empty slice.
    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// A file computed at the opening of the demo.
#[derive(Debug, Clone)]
pub(crate) enum Generated {
    /// Landscape picture, 24-bit BMP, with a small BMP thumbnail.
    Bmp { width: u32, height: u32, seed: u64 },
    /// Melody, 16-bit mono PCM at 44.1 kHz.
    Wav { millis: u32, seed: u64 },
    /// Text PDF of several pages (ASCII).
    Pdf {
        title: Cow<'static, str>,
        pages: u32,
        seed: u64,
    },
    /// Bank-statement-like CSV (semicolons, French headers).
    Csv { rows: u32, seed: u64 },
    /// Sensor export, pretty-printed JSON.
    Json { records: u32, seed: u64 },
    /// Application log, one event per line.
    Log { lines: u32, seed: u64 },
    /// Long source file.
    Code { lang: CodeLang, functions: u32 },
    /// Rich Markdown document of about `bytes`.
    Markdown { bytes: u32, seed: u64 },
    /// Opaque bytes.
    Binary { bytes: u32, seed: u64 },
    /// ZIP archive, stored (no compression), of a few generated files.
    Zip {
        image_side: u32,
        csv_rows: u32,
        seed: u64,
    },
}

/// Language of a generated source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeLang {
    Rust,
    Kotlin,
    Python,
    TypeScript,
    Sql,
}

impl CodeLang {
    pub(crate) const fn mime(self) -> &'static str {
        match self {
            Self::Rust => "text/x-rust",
            Self::Kotlin => "text/x-kotlin",
            Self::Python => "text/x-python",
            Self::TypeScript => "text/typescript",
            Self::Sql => "application/sql",
        }
    }
}

/// Bytes, type and thumbnail of a generated file.
pub(crate) struct GeneratedFile {
    pub mime: &'static str,
    pub data: Vec<u8>,
    pub thumbnail: Option<Thumbnail>,
}

/// Width of the thumbnails of generated pictures (well under the 128 KiB of a demo thumbnail).
const THUMB_WIDTH: u32 = 160;

impl Generated {
    /// Type of the file.
    pub(crate) const fn mime(&self) -> &'static str {
        match self {
            Self::Bmp { .. } => "image/bmp",
            Self::Wav { .. } => "audio/wav",
            Self::Pdf { .. } => "application/pdf",
            Self::Csv { .. } => "text/csv",
            Self::Json { .. } => "application/json",
            Self::Log { .. } => "text/plain",
            Self::Code { lang, .. } => lang.mime(),
            Self::Markdown { .. } => "text/markdown",
            Self::Binary { .. } => "application/octet-stream",
            Self::Zip { .. } => "application/zip",
        }
    }

    /// Computes the file; `at_ms` is the time of the item (dates inside the file end there).
    pub(crate) fn generate(&self, at_ms: i64) -> GeneratedFile {
        let mime = self.mime();
        let plain = |data| GeneratedFile {
            mime,
            data,
            thumbnail: None,
        };
        match self {
            Self::Bmp {
                width,
                height,
                seed,
            } => {
                let tw = THUMB_WIDTH.min(*width).max(1);
                let th = (u64::from(*height) * u64::from(tw) / u64::from((*width).max(1))).max(1);
                let th = u32::try_from(th).unwrap_or(1);
                GeneratedFile {
                    mime,
                    data: bmp(*width, *height, *seed),
                    thumbnail: Some(Thumbnail {
                        mime: "image/bmp".into(),
                        data: bmp(tw, th, *seed),
                    }),
                }
            }
            Self::Wav { millis, seed } => plain(wav(*millis, *seed)),
            Self::Pdf { title, pages, seed } => plain(pdf_document(title, *pages, *seed)),
            Self::Csv { rows, seed } => plain(csv(*rows, *seed, at_ms).into_bytes()),
            Self::Json { records, seed } => plain(json(*records, *seed, at_ms).into_bytes()),
            Self::Log { lines, seed } => plain(log(*lines, *seed, at_ms).into_bytes()),
            Self::Code { lang, functions } => plain(code(*lang, *functions).into_bytes()),
            Self::Markdown { bytes, seed } => {
                plain(markdown_document(*bytes as usize, *seed).0.into_bytes())
            }
            Self::Binary { bytes, seed } => plain(binary(*bytes, *seed)),
            Self::Zip {
                image_side,
                csv_rows,
                seed,
            } => {
                let side = *image_side;
                let mut files = vec![(
                    "LISEZMOI.txt".to_owned(),
                    "Archive de démonstration Tomi : photos, relevés et notes générés.\n"
                        .as_bytes()
                        .to_vec(),
                )];
                for i in 0..3u64 {
                    files.push((
                        format!("photos/vue-{:02}.bmp", i + 1),
                        bmp(side, side * 3 / 4, seed.wrapping_add(i)),
                    ));
                }
                files.push((
                    "releves.csv".to_owned(),
                    csv(*csv_rows, *seed, at_ms).into_bytes(),
                ));
                files.push((
                    "notes/carnet.md".to_owned(),
                    markdown_document(8_000, *seed).0.into_bytes(),
                ));
                plain(zip_stored(&files, at_ms))
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------------------------

/// (year, month, day, hour, minute, second) of a Unix time in milliseconds (UTC).
fn civil(ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = ms.div_euclid(DAY_MS);
    let secs = ms.rem_euclid(DAY_MS) / 1000;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let small = |v: i64| u32::try_from(v).unwrap_or(0);
    (
        y,
        small(m),
        small(d),
        small(secs / 3600),
        small(secs / 60 % 60),
        small(secs % 60),
    )
}

fn iso(ms: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(ms);
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:03}Z",
        ms.rem_euclid(1000)
    )
}

fn date_fr(ms: i64) -> String {
    let (y, mo, d, ..) = civil(ms);
    format!("{d:02}/{mo:02}/{y:04}")
}

// ---------------------------------------------------------------------------------------------
// BMP
// ---------------------------------------------------------------------------------------------

/// Colours of a generated landscape.
struct Palette {
    sky_top: [i32; 3],
    sky_bottom: [i32; 3],
    sun: [i32; 3],
    far: [i32; 3],
    near: [i32; 3],
    water: [i32; 3],
}

const PALETTES: [Palette; 4] = [
    // Morning.
    Palette {
        sky_top: [86, 140, 214],
        sky_bottom: [250, 214, 165],
        sun: [255, 236, 170],
        far: [120, 132, 170],
        near: [52, 92, 74],
        water: [70, 120, 170],
    },
    // Noon.
    Palette {
        sky_top: [40, 110, 220],
        sky_bottom: [170, 210, 245],
        sun: [255, 252, 220],
        far: [110, 150, 120],
        near: [46, 110, 60],
        water: [40, 100, 160],
    },
    // Dusk.
    Palette {
        sky_top: [36, 50, 74],
        sky_bottom: [255, 200, 92],
        sun: [255, 170, 80],
        far: [90, 70, 110],
        near: [40, 36, 60],
        water: [60, 60, 100],
    },
    // Winter.
    Palette {
        sky_top: [150, 175, 200],
        sky_bottom: [225, 232, 240],
        sun: [255, 255, 250],
        far: [200, 210, 225],
        near: [235, 240, 245],
        water: [120, 140, 165],
    },
];

fn mix(a: [i32; 3], b: [i32; 3], num: i32, den: i32) -> [i32; 3] {
    let den = den.max(1);
    [
        a[0] + (b[0] - a[0]) * num / den,
        a[1] + (b[1] - a[1]) * num / den,
        a[2] + (b[2] - a[2]) * num / den,
    ]
}

/// Ridge line (row from the top) of a mountain layer, for each column.
fn ridge(w: usize, h: usize, base: f64, amp: f64, rng: &mut Rng) -> Vec<i32> {
    let f1 = 1.0 + rng.below(3) as f64;
    let f2 = 3.0 + rng.below(5) as f64;
    let p1 = rng.below(628) as f64 / 100.0;
    let p2 = rng.below(628) as f64 / 100.0;
    (0..w)
        .map(|x| {
            let t = x as f64 / w.max(1) as f64 * std::f64::consts::TAU;
            let v = base + amp * (f1 * t + p1).sin() + amp * 0.4 * (f2 * t + p2).sin();
            (v * h as f64) as i32
        })
        .collect()
}

/// A landscape picture (sky, sun, two mountain ranges, a lake), 24-bit uncompressed BMP.
pub(crate) fn bmp(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let w = width.max(1) as usize;
    let h = height.max(1) as usize;
    let row = (w * 3 + 3) & !3;
    let pixels = row * h;
    let mut out = Vec::with_capacity(54 + pixels);
    let u32le = |out: &mut Vec<u8>, v: usize| {
        out.extend_from_slice(&u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes());
    };
    out.extend_from_slice(b"BM");
    u32le(&mut out, 54 + pixels);
    u32le(&mut out, 0);
    u32le(&mut out, 54);
    u32le(&mut out, 40);
    u32le(&mut out, w);
    u32le(&mut out, h);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    u32le(&mut out, 0);
    u32le(&mut out, pixels);
    u32le(&mut out, 2835);
    u32le(&mut out, 2835);
    u32le(&mut out, 0);
    u32le(&mut out, 0);

    let mut rng = Rng::new(seed);
    let p = &PALETTES[(seed % PALETTES.len() as u64) as usize];
    let far = ridge(w, h, 0.45, 0.06, &mut rng);
    let near = ridge(w, h, 0.62, 0.05, &mut rng);
    let shore = (h as f64 * 0.78) as i32;
    let sun_x = (w as u64 * (20 + rng.below(60)) / 100) as i32;
    let sun_y = (h as u64 * (15 + rng.below(20)) / 100) as i32;
    let sun_r = (h as i32 / 12).max(1);
    let (wi, hi) = (w as i32, h as i32);
    for y in (0..hi).rev() {
        for x in 0..wi {
            let xi = x as usize;
            let (dx, dy) = (x - sun_x, y - sun_y);
            let c = if y >= shore {
                // The lake reflects the sky, darker towards the bottom.
                let sky = mix(p.sky_bottom, p.sky_top, y - shore, (hi - shore).max(1));
                let ripple = if (y + x / 7) % 9 == 0 { 18 } else { 0 };
                let m = mix(sky, p.water, 2, 3);
                [m[0] + ripple, m[1] + ripple, m[2] + ripple]
            } else if y >= near[xi] {
                mix(p.near, [0, 0, 0], y - near[xi], hi * 2)
            } else if y >= far[xi] {
                mix(p.far, p.near, y - far[xi], hi)
            } else if dx * dx + dy * dy <= sun_r * sun_r {
                p.sun
            } else {
                mix(p.sky_top, p.sky_bottom, y, shore.max(1))
            };
            // A little grain, so that the picture is not flat.
            let grain = (((x as u32).wrapping_mul(73_856_093)
                ^ (y as u32).wrapping_mul(19_349_663))
                % 7) as i32
                - 3;
            for ch in [2, 1, 0] {
                out.push((c[ch] + grain).clamp(0, 255) as u8);
            }
        }
        out.resize(out.len() + (row - w * 3), 0);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// WAV
// ---------------------------------------------------------------------------------------------

/// A calm melody (pentatonic notes with a soft envelope), 16-bit mono PCM at 44.1 kHz.
pub(crate) fn wav(millis: u32, seed: u64) -> Vec<u8> {
    const RATE: u32 = 44_100;
    const NOTES: [f64; 8] = [261.63, 293.66, 329.63, 392.0, 440.0, 523.25, 587.33, 659.25];
    let samples = usize::try_from(u64::from(RATE) * u64::from(millis) / 1000).unwrap_or(0);
    let data_len = samples * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    let u32le = |out: &mut Vec<u8>, v: usize| {
        out.extend_from_slice(&u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes());
    };
    out.extend_from_slice(b"RIFF");
    u32le(&mut out, 36 + data_len);
    out.extend_from_slice(b"WAVEfmt ");
    u32le(&mut out, 16);
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    u32le(&mut out, RATE as usize);
    u32le(&mut out, RATE as usize * 2);
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    u32le(&mut out, data_len);

    let mut rng = Rng::new(seed);
    let mut written = 0;
    while written < samples {
        let freq = *rng.pick(&NOTES);
        let len = (RATE as usize * (200 + rng.below(400) as usize) / 1000).min(samples - written);
        let attack = (RATE as usize / 100).max(1);
        for i in 0..len {
            let t = i as f64 / f64::from(RATE);
            let env = if i < attack {
                i as f64 / attack as f64
            } else {
                (1.0 - (i - attack) as f64 / (len.max(attack + 1) - attack) as f64).max(0.0)
            };
            let wave = (std::f64::consts::TAU * freq * t).sin()
                + 0.3 * (std::f64::consts::TAU * freq * 2.0 * t).sin();
            let v = (wave * env * 0.25 * f64::from(i16::MAX)) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        written += len;
    }
    out
}

// ---------------------------------------------------------------------------------------------
// PDF
// ---------------------------------------------------------------------------------------------

/// ASCII text for a PDF string: other characters become `?`, delimiters are dropped.
fn pdf_safe(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '(' | ')' | '\\' => ' ',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '?',
        })
        .collect()
}

/// A PDF file of `objects` (numbered from 1, the first one the catalog), with its
/// cross-reference table.
pub(crate) fn pdf_objects(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A valid one-page PDF printing `title` (ASCII; other characters become `?`).
pub(crate) fn pdf(title: &str) -> Vec<u8> {
    let content = format!("BT /F1 20 Tf 72 760 Td ({}) Tj ET", pdf_safe(title));
    pdf_objects(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ])
}

const PDF_WORDS: [&str; 24] = [
    "contrat",
    "parties",
    "article",
    "prestation",
    "annexe",
    "delai",
    "paiement",
    "livraison",
    "garantie",
    "conditions",
    "duree",
    "resiliation",
    "facture",
    "montant",
    "echeance",
    "responsabilite",
    "service",
    "client",
    "prestataire",
    "obligations",
    "document",
    "version",
    "signature",
    "accord",
];

/// A text PDF of `pages` pages (`title` on top of each one), Helvetica, ASCII.
pub(crate) fn pdf_document(title: &str, pages: u32, seed: u64) -> Vec<u8> {
    let pages = pages.max(1) as usize;
    let title = pdf_safe(title);
    let mut rng = Rng::new(seed);
    // 1 catalog, 2 page tree, 3 font, then a page and its content per page.
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    for page in 0..pages {
        let mut content = format!(
            "BT /F1 16 Tf 72 790 Td ({title}) Tj ET\nBT /F1 9 Tf 72 40 Td (Page {} / {pages}) Tj ET\n\
             BT /F1 10 Tf 72 760 Td 13 TL\n",
            page + 1
        );
        let mut article = page * 3 + 1;
        for line in 0..52 {
            if line % 13 == 0 {
                let _ = writeln!(content, "(Article {article}) Tj T*");
                article += 1;
                continue;
            }
            let mut text = String::new();
            while text.len() < 80 {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(rng.pick(&PDF_WORDS));
            }
            let _ = writeln!(content, "({text}) Tj T*");
        }
        content.push_str("ET");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents {} 0 R \
             /Resources << /Font << /F1 3 0 R >> >> >>",
            5 + 2 * page
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    pdf_objects(&objects)
}

// ---------------------------------------------------------------------------------------------
// ZIP (stored)
// ---------------------------------------------------------------------------------------------

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// CRC-32 (IEEE 802.3), as ZIP wants it.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// A ZIP archive of `files` (path, bytes), stored without compression, dated `at_ms`.
pub(crate) fn zip_stored(files: &[(String, Vec<u8>)], at_ms: i64) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = civil(at_ms);
    let year = u16::try_from((y - 1980).clamp(0, 127)).unwrap_or(0);
    let date = (year << 9) | ((mo as u16) << 5) | d as u16;
    let time = ((h as u16) << 11) | ((mi as u16) << 5) | (s as u16 / 2);
    let u16le = |out: &mut Vec<u8>, v: usize| {
        out.extend_from_slice(&u16::try_from(v).unwrap_or(u16::MAX).to_le_bytes());
    };
    let u32le = |out: &mut Vec<u8>, v: usize| {
        out.extend_from_slice(&u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes());
    };
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32(data) as usize;
        let offset = out.len();
        // Local header: version 2.0, UTF-8 names (bit 11), method 0 (stored).
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        u16le(&mut out, 20);
        u16le(&mut out, 0x0800);
        u16le(&mut out, 0);
        u16le(&mut out, time as usize);
        u16le(&mut out, date as usize);
        u32le(&mut out, crc);
        u32le(&mut out, data.len());
        u32le(&mut out, data.len());
        u16le(&mut out, name.len());
        u16le(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        u16le(&mut central, 20);
        u16le(&mut central, 20);
        u16le(&mut central, 0x0800);
        u16le(&mut central, 0);
        u16le(&mut central, time as usize);
        u16le(&mut central, date as usize);
        u32le(&mut central, crc);
        u32le(&mut central, data.len());
        u32le(&mut central, data.len());
        u16le(&mut central, name.len());
        u16le(&mut central, 0); // extra
        u16le(&mut central, 0); // comment
        u16le(&mut central, 0); // disk
        u16le(&mut central, 0); // internal attributes
        u32le(&mut central, 0); // external attributes
        u32le(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len();
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    u16le(&mut out, 0);
    u16le(&mut out, 0);
    u16le(&mut out, files.len());
    u16le(&mut out, files.len());
    u32le(&mut out, central.len());
    u32le(&mut out, central_offset);
    u16le(&mut out, 0);
    out
}

// ---------------------------------------------------------------------------------------------
// Data files
// ---------------------------------------------------------------------------------------------

const ACCOUNTS: [&str; 3] = ["Compte courant", "Livret", "Compte joint"];
const LABELS: [(&str, &str); 12] = [
    ("Boulangerie du Centre", "Alimentation"),
    ("Supermarché Exemple", "Alimentation"),
    ("Abonnement téléphone", "Télécom"),
    ("Électricité", "Logement"),
    ("Loyer", "Logement"),
    ("Pharmacie des Tilleuls", "Santé"),
    ("Librairie Exemple", "Loisirs"),
    ("Cinéma", "Loisirs"),
    ("Station-service", "Transport"),
    ("Billet de train", "Transport"),
    ("Virement reçu", "Revenus"),
    ("Assurance habitation", "Logement"),
];

/// Bank-statement-like CSV, newest line first, ending at `at_ms`.
pub(crate) fn csv(rows: u32, seed: u64, at_ms: i64) -> String {
    let mut rng = Rng::new(seed);
    let mut out = String::with_capacity(rows as usize * 72 + 64);
    out.push_str("date;compte;libellé;catégorie;montant;solde\n");
    let mut balance: i64 = 254_321;
    let mut t = at_ms;
    for _ in 0..rows {
        t -= rng.range(600_000, 8 * 3_600_000);
        let (label, category) = *rng.pick(&LABELS);
        let cents = if category == "Revenus" {
            rng.range(50_000, 250_000)
        } else {
            -rng.range(150, 12_000)
        };
        balance -= cents;
        let _ = writeln!(
            out,
            "{};{};{label};{category};{}{}.{:02};{}.{:02}",
            date_fr(t),
            rng.pick(&ACCOUNTS),
            if cents < 0 { "-" } else { "" },
            cents.abs() / 100,
            cents.abs() % 100,
            balance / 100,
            balance.rem_euclid(100)
        );
    }
    out
}

const SENSORS: [(&str, &str); 6] = [
    ("salon-01", "interieur"),
    ("chambre-02", "interieur"),
    ("cuisine-03", "interieur"),
    ("garage-04", "annexe"),
    ("jardin-05", "exterieur"),
    ("cave-06", "annexe"),
];

/// Sensor export, pretty-printed JSON, the last record at `at_ms`.
pub(crate) fn json(records: u32, seed: u64, at_ms: i64) -> String {
    let mut rng = Rng::new(seed);
    let mut out = String::with_capacity(records as usize * 260 + 256);
    let _ = write!(
        out,
        "{{\n  \"export\": \"Relevés de la maison (démo Tomi)\",\n  \"version\": 3,\n  \
         \"genere_le\": \"{}\",\n  \"mesures\": [\n",
        iso(at_ms)
    );
    let start = at_ms - i64::from(records) * 300_000;
    for i in 0..records {
        let (sensor, zone) = *rng.pick(&SENSORS);
        let temp = rng.range(150, 260);
        let _ = write!(
            out,
            "    {{\n      \"id\": {},\n      \"capteur\": \"{sensor}\",\n      \
             \"horodatage\": \"{}\",\n      \"temperature\": {}.{},\n      \"humidite\": {},\n      \
             \"batterie\": {},\n      \"etat\": \"{}\",\n      \"tags\": [\"{zone}\", \"demo\"]\n    }}{}\n",
            i + 1,
            iso(start + i64::from(i) * 300_000),
            temp / 10,
            temp % 10,
            rng.range(30, 70),
            rng.range(5, 101),
            if rng.chance(3) { "alerte" } else { "ok" },
            if i + 1 == records { "" } else { "," }
        );
    }
    out.push_str("  ]\n}\n");
    out
}

const LOG_EVENTS: [(&str, &str); 10] = [
    ("INFO ", "sync: {n} événements appliqués"),
    ("INFO ", "outbox: {n} envois en attente"),
    ("DEBUG", "http: GET /v1/events -> 200 ({n} ms)"),
    ("DEBUG", "http: POST /v1/items -> 201 ({n} ms)"),
    ("INFO ", "doorbell: réveil reçu"),
    ("WARN ", "réseau indisponible, nouvel essai dans {n} s"),
    ("INFO ", "pièce jointe déchiffrée ({n} Kio)"),
    ("DEBUG", "cache: {n} vignettes en mémoire"),
    ("INFO ", "maintenance: {n} éléments expirés purgés"),
    ("ERROR", "upload interrompu, reprise à la partie {n}"),
];

/// Application log, one event per line, the last one at `at_ms`.
pub(crate) fn log(lines: u32, seed: u64, at_ms: i64) -> String {
    let mut rng = Rng::new(seed);
    let mut out = String::with_capacity(lines as usize * 90);
    let mut t = at_ms - i64::from(lines) * 2_000;
    for i in 0..lines {
        t += rng.range(10, 4_000);
        let (level, template) = *rng.pick(&LOG_EVENTS);
        let n = rng.range(1, 500).to_string();
        let _ = writeln!(
            out,
            "{} {level} [tomi-demo#{}] {}",
            iso(t),
            i % 8 + 1,
            template.replace("{n}", &n)
        );
    }
    out
}

/// Pseudo-random opaque bytes.
pub(crate) fn binary(bytes: u32, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let len = bytes as usize;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

// ---------------------------------------------------------------------------------------------
// Source code
// ---------------------------------------------------------------------------------------------

/// A long source file of `functions` small functions (comments in French).
pub(crate) fn code(lang: CodeLang, functions: u32) -> String {
    let mut out = String::new();
    match lang {
        CodeLang::Rust => {
            out.push_str("//! Module de démonstration Tomi : calculs sur des relevés.\n\nuse std::collections::HashMap;\n\n#[derive(Debug, Clone, PartialEq)]\npub struct Releve {\n    pub capteur: String,\n    pub valeur: f64,\n}\n\n");
            for i in 0..functions {
                let _ = write!(
                    out,
                    "/// Moyenne des relevés du capteur n° {i}, ignorant les valeurs hors bornes.\n\
                     pub fn moyenne_{i}(releves: &[Releve], min: f64, max: f64) -> Option<f64> {{\n    \
                     let valides: Vec<f64> = releves\n        .iter()\n        \
                     .filter(|r| r.capteur == \"capteur-{i}\" && (min..=max).contains(&r.valeur))\n        \
                     .map(|r| r.valeur)\n        .collect();\n    \
                     if valides.is_empty() {{\n        return None;\n    }}\n    \
                     Some(valides.iter().sum::<f64>() / valides.len() as f64)\n}}\n\n\
                     pub fn compter_{i}(releves: &[Releve]) -> HashMap<String, usize> {{\n    \
                     let mut parts = HashMap::new();\n    for r in releves {{\n        \
                     *parts.entry(r.capteur.clone()).or_insert(0) += {};\n    }}\n    parts\n}}\n\n",
                    i % 3 + 1
                );
            }
        }
        CodeLang::Kotlin => {
            out.push_str("// Démo Tomi : liste de tâches\npackage eu.example.demo\n\ndata class Tache(val id: Int, val titre: String, val faite: Boolean = false)\n\n");
            for i in 0..functions {
                let _ = write!(
                    out,
                    "/** Filtre n° {i} : tâches restantes dont le titre contient le mot-clé. */\n\
                     fun restantes{i}(taches: List<Tache>, motCle: String): List<Tache> =\n    \
                     taches.filter {{ !it.faite && it.titre.contains(motCle, ignoreCase = true) }}\n        \
                     .sortedBy {{ it.id }}\n        .take({})\n\n\
                     fun resume{i}(taches: List<Tache>): String {{\n    \
                     val faites = taches.count {{ it.faite }}\n    \
                     return \"$faites / ${{taches.size}} tâches faites (lot {i})\"\n}}\n\n",
                    10 + i % 20
                );
            }
        }
        CodeLang::Python => {
            out.push_str("\"\"\"Démo Tomi : analyse d'un journal.\"\"\"\n\nfrom collections import Counter\nfrom dataclasses import dataclass\n\n\n@dataclass\nclass Ligne:\n    niveau: str\n    message: str\n\n\n");
            for i in 0..functions {
                let _ = write!(
                    out,
                    "def niveaux_{i}(lignes: list[Ligne]) -> Counter:\n    \
                     \"\"\"Compte les niveaux du lot {i}.\"\"\"\n    \
                     return Counter(l.niveau for l in lignes if l.message.startswith(\"lot-{i}\"))\n\n\n\
                     def erreurs_{i}(lignes: list[Ligne], seuil: int = {}) -> bool:\n    \
                     total = sum(1 for l in lignes if l.niveau == \"ERROR\")\n    \
                     return total >= seuil\n\n\n",
                    i % 7 + 1
                );
            }
        }
        CodeLang::TypeScript => {
            out.push_str("// Démo Tomi : petit client d'API fictive\n\nexport interface Article {\n  id: number;\n  titre: string;\n  lien: string;\n}\n\nconst BASE = 'https://api.example.org/v2';\n\n");
            for i in 0..functions {
                let _ = write!(
                    out,
                    "/** Page {i} des articles, triés par titre. */\n\
                     export async function articles{i}(page = {i}): Promise<Article[]> {{\n  \
                     const reponse = await fetch(`${{BASE}}/articles?page=${{page}}&taille={}`);\n  \
                     if (!reponse.ok) {{\n    throw new Error(`HTTP ${{reponse.status}}`);\n  }}\n  \
                     const liste: Article[] = await reponse.json();\n  \
                     return liste.sort((a, b) => a.titre.localeCompare(b.titre));\n}}\n\n",
                    20 + i % 30
                );
            }
        }
        CodeLang::Sql => {
            out.push_str("-- Démo Tomi : schéma et requêtes d'une bibliothèque fictive\n\nCREATE TABLE livre (\n  id INTEGER PRIMARY KEY,\n  titre TEXT NOT NULL,\n  auteur TEXT NOT NULL,\n  annee INTEGER\n);\n\n");
            for i in 0..functions {
                let _ = write!(
                    out,
                    "-- Requête {i} : livres parus après {}\n\
                     SELECT titre, auteur, annee\nFROM livre\nWHERE annee > {}\n  \
                     AND titre LIKE '%{}%'\nORDER BY annee DESC, titre\nLIMIT 50;\n\n",
                    1950 + i % 70,
                    1950 + i % 70,
                    ["mer", "nuit", "jardin", "ville", "temps"][i as usize % 5]
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Texts
// ---------------------------------------------------------------------------------------------

const SENTENCES: [&str; 16] = [
    "Le rendez-vous est confirmé pour la fin de la matinée.",
    "Pense à vérifier la liste avant de partir.",
    "Les documents sont rangés dans le dossier partagé.",
    "La réunion a été déplacée à la semaine prochaine.",
    "Il reste encore quelques points à éclaircir sur le budget.",
    "Le colis devrait arriver entre mardi et jeudi.",
    "On garde la même organisation que l'an dernier.",
    "Les horaires changent pendant les vacances scolaires.",
    "Le devis comprend la pose et le déplacement.",
    "Tout est prêt, il ne manque que la validation finale.",
    "La version imprimée est plus lisible que l'écran.",
    "Une copie de sauvegarde est faite chaque semaine.",
    "Le parcours fait une douzaine de kilomètres, sans difficulté.",
    "Les chiffres du trimestre sont meilleurs que prévu.",
    "La salle est réservée de quatorze à seize heures.",
    "On fera le point ensemble à la fin du mois.",
];

/// A paragraph of `n` sentences.
pub(crate) fn paragraph(rng: &mut Rng, n: usize) -> String {
    let mut p = String::new();
    for i in 0..n {
        if i > 0 {
            p.push(' ');
        }
        p.push_str(rng.pick(&SENTENCES));
    }
    p
}

/// Plain text of about `bytes`, in paragraphs.
pub(crate) fn long_text(bytes: usize, seed: u64) -> String {
    let mut rng = Rng::new(seed);
    let mut out = String::with_capacity(bytes + 512);
    let mut n = 1;
    while out.len() < bytes {
        let _ = write!(out, "{n}. ");
        let count = 3 + rng.below(5) as usize;
        out.push_str(&paragraph(&mut rng, count));
        out.push_str("\n\n");
        n += 1;
    }
    out.truncate(out.trim_end().len());
    out
}

const TOPICS: [&str; 12] = [
    "Installation",
    "Configuration",
    "Sauvegardes",
    "Réseau",
    "Sécurité",
    "Synchronisation",
    "Performances",
    "Dépannage",
    "Migration",
    "Accessibilité",
    "Notifications",
    "Archives",
];

/// A long fictitious guide in Markdown (headings, lists, tables, code, quotes, links) of about
/// `bytes`, and the same text without markup (the plain form of the item).
pub(crate) fn markdown_document(bytes: usize, seed: u64) -> (String, String) {
    let mut rng = Rng::new(seed);
    let mut md = String::with_capacity(bytes + 4096);
    let mut plain = String::with_capacity(bytes);
    md.push_str("# Guide complet de la maison (démo Tomi)\n\n> Document fictif généré pour la démonstration : rien n'y est réel.\n\n");
    plain.push_str("Guide complet de la maison (démo Tomi)\n\nDocument fictif généré pour la démonstration : rien n'y est réel.\n\n");
    let mut chapter = 1;
    while md.len() < bytes {
        let topic = TOPICS[(chapter - 1) % TOPICS.len()];
        let _ = write!(md, "## {chapter}. {topic}\n\n");
        let _ = write!(plain, "{chapter}. {topic}\n\n");
        let para = paragraph(&mut rng, 4);
        let _ = write!(
            md,
            "{para} Voir aussi la [page {topic}](https://docs.example.org/guide/{chapter}) et la **note** associée.\n\n"
        );
        let _ = write!(
            plain,
            "{para} Voir aussi la page {topic} et la note associée.\n\n"
        );
        let _ = write!(md, "### {chapter}.1 Étapes\n\n");
        let _ = write!(plain, "{chapter}.1 Étapes\n\n");
        for step in 1..=5 {
            let s = paragraph(&mut rng, 1);
            let _ = writeln!(md, "{step}. {s}");
            let _ = writeln!(plain, "{step}. {s}");
        }
        md.push_str("\n- [x] Prérequis vérifiés\n- [ ] Validation finale\n  - relire les *réglages*\n  - noter la `version`\n\n");
        plain.push_str(
            "\nPrérequis vérifiés\nValidation finale\nrelire les réglages\nnoter la version\n\n",
        );
        let _ = write!(md, "### {chapter}.2 Récapitulatif\n\n| Élément | Valeur | État |\n|:--------|-------:|:----:|\n");
        let _ = write!(plain, "{chapter}.2 Récapitulatif\n\n");
        for row in 1..=6 {
            let v = rng.range(1, 1000);
            let state = if rng.chance(80) { "ok" } else { "à revoir" };
            let _ = writeln!(md, "| {topic} {row} | {v} | {state} |");
            let _ = writeln!(plain, "{topic} {row} : {v} ({state})");
        }
        let _ = write!(
            md,
            "\n```bash\n# Étape {chapter} : vérifier l'état\ntomi-demo statut --chapitre {chapter} --format json\necho \"ok\"\n```\n\n"
        );
        let _ = write!(
            plain,
            "\ntomi-demo statut --chapitre {chapter} --format json\n\n"
        );
        let quote = paragraph(&mut rng, 2);
        let _ = write!(md, "> **À retenir :** {quote}\n\n---\n\n");
        let _ = write!(plain, "À retenir : {quote}\n\n");
        chapter += 1;
    }
    (md, plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic() {
        let a: Vec<u64> = (0..4)
            .scan(Rng::new(7), |r, _| Some(r.next_u64()))
            .collect();
        let b: Vec<u64> = (0..4)
            .scan(Rng::new(7), |r, _| Some(r.next_u64()))
            .collect();
        assert_eq!(a, b);
        assert_ne!(a[0], a[1]);
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29T12:34:56.789Z
        assert_eq!(iso(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
        assert_eq!(date_fr(1_709_210_096_789), "29/02/2024");
    }

    #[test]
    fn crc32_matches_the_reference_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_bmp_has_the_announced_size() {
        let b = bmp(5, 3, 1);
        assert_eq!(&b[..2], b"BM");
        // Rows of 15 bytes padded to 16.
        assert_eq!(b.len(), 54 + 16 * 3);
        assert_eq!(
            u32::from_le_bytes([b[2], b[3], b[4], b[5]]) as usize,
            b.len()
        );
        assert_eq!(u32::from_le_bytes([b[18], b[19], b[20], b[21]]), 5);
    }

    #[test]
    fn a_wav_header_matches_its_data() {
        let w = wav(100, 1);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        let data = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
        assert_eq!(data, 4410 * 2);
        assert_eq!(w.len(), 44 + data);
    }

    #[test]
    fn a_stored_zip_lists_its_files() {
        let files = vec![
            ("a.txt".to_owned(), b"bonjour".to_vec()),
            ("dossier/b.bin".to_owned(), vec![1, 2, 3]),
        ];
        let z = zip_stored(&files, 1_709_210_096_789);
        assert_eq!(&z[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let end = z.len() - 22;
        assert_eq!(&z[end..end + 4], &[0x50, 0x4b, 0x05, 0x06]);
        assert_eq!(u16::from_le_bytes([z[end + 10], z[end + 11]]), 2);
        let cd = u32::from_le_bytes([z[end + 16], z[end + 17], z[end + 18], z[end + 19]]) as usize;
        assert_eq!(&z[cd..cd + 4], &[0x50, 0x4b, 0x01, 0x02]);
        // The CRC of the first entry, in its local header.
        assert_eq!(
            u32::from_le_bytes([z[14], z[15], z[16], z[17]]),
            crc32(b"bonjour")
        );
        assert_eq!(&z[30..35], b"a.txt");
        assert_eq!(&z[35..42], b"bonjour");
    }

    #[test]
    fn a_long_pdf_has_one_page_object_per_page() {
        let p = pdf_document("Rapport (été)", 12, 3);
        let text = String::from_utf8(p).unwrap();
        assert!(text.contains("/Count 12 >>"));
        assert_eq!(text.matches("/Type /Page /Parent").count(), 12);
        assert!(text.contains("(Rapport  ?t? ) Tj"));
        assert!(text.ends_with("%%EOF\n"));
    }

    #[test]
    fn the_data_files_have_the_expected_shape() {
        let c = csv(10, 1, 1_709_210_096_789);
        assert_eq!(c.lines().count(), 11);
        let j = json(3, 1, 1_709_210_096_789);
        assert!(j.trim_end().ends_with("]\n}") && j.matches("\"id\":").count() == 3);
        assert_eq!(log(5, 1, 1_709_210_096_789).lines().count(), 5);
        assert_eq!(binary(13, 1).len(), 13);
        let (md, plain) = markdown_document(20_000, 1);
        assert!(md.len() >= 20_000 && md.contains("| Élément |") && md.contains("```bash"));
        assert!(!plain.contains("```") && !plain.contains("**"));
        for lang in [
            CodeLang::Rust,
            CodeLang::Kotlin,
            CodeLang::Python,
            CodeLang::TypeScript,
            CodeLang::Sql,
        ] {
            assert!(code(lang, 3).lines().count() > 20);
        }
    }
}
