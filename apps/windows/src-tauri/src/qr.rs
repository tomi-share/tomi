//! QR code of the pairing link (`spec/pairing-v1.md` §4.3).
//!
//! The QR code carries **exactly** the link `https://<relay>/l/pair#<code>` that the core
//! built, error correction level M. The code is turned into the modules of a matrix and then
//! into one SVG path that the webview draws (`QrCode.svelte`): nothing is rendered to an
//! image file, nothing is stored. The path is the secret in another form (it decodes to the
//! link): `Debug` does not print it.

use std::fmt::Write as _;

use qrcode::{Color, EcLevel, QrCode};
use serde::Serialize;

/// Width of the empty border, in modules. The QR standard asks for at least 4.
const QUIET_ZONE: u32 = 4;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Qr {
    /// Modules on a side.
    pub size: u32,
    /// Empty border around the matrix, in modules.
    pub quiet: u32,
    /// SVG path data in module units: one `M x y h n v 1 h -n z` rectangle per horizontal run
    /// of dark modules. Fill it dark on a light background that covers the quiet zone.
    pub path: String,
}

impl std::fmt::Debug for Qr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Qr")
            .field("size", &self.size)
            .field("path", &"<redacted>")
            .finish()
    }
}

/// The QR code of `link`, or `None` if it cannot be encoded (it would not fit any version).
pub fn encode(link: &str) -> Option<Qr> {
    let code = QrCode::with_error_correction_level(link.as_bytes(), EcLevel::M).ok()?;
    let width = code.width();
    let modules = code.to_colors();
    let mut path = String::new();
    for (y, row) in modules.chunks(width).enumerate() {
        let mut x = 0;
        while x < width {
            if row[x] != Color::Dark {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && row[x] == Color::Dark {
                x += 1;
            }
            let run = x - start;
            let _ = write!(path, "M{start} {y}h{run}v1h-{run}z");
        }
    }
    Some(Qr {
        size: u32::try_from(width).ok()?,
        quiet: QUIET_ZONE,
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://relay.example.org/l/pair#4821-crayon-renard";

    /// The modules of a drawn code, read back from its path.
    fn modules_of(qr: &Qr) -> Vec<Vec<bool>> {
        let n = qr.size as usize;
        let mut grid = vec![vec![false; n]; n];
        let runs = regex::Regex::new(r"M(\d+) (\d+)h(\d+)v1h-(\d+)z").unwrap();
        let mut seen = 0;
        for c in runs.captures_iter(&qr.path) {
            let num = |i: usize| c[i].parse::<usize>().unwrap();
            let (x, y, w, back) = (num(1), num(2), num(3), num(4));
            assert_eq!(w, back, "a run is closed with the width it opened");
            for cell in &mut grid[y][x..x + w] {
                assert!(!*cell, "no module is drawn twice");
                *cell = true;
            }
            seen += c[0].len();
        }
        assert_eq!(seen, qr.path.len(), "the path holds nothing but runs");
        grid
    }

    /// A greyscale picture of the code at `scale` pixels a module, with its quiet zone.
    fn picture(qr: &Qr, scale: usize) -> (usize, usize, Vec<u8>) {
        let grid = modules_of(qr);
        let side = (qr.size + 2 * qr.quiet) as usize * scale;
        let quiet = qr.quiet as usize;
        let mut pixels = vec![255u8; side * side];
        for (y, row) in grid.iter().enumerate() {
            for (x, dark) in row.iter().enumerate() {
                if !dark {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = ((x + quiet) * scale + dx, (y + quiet) * scale + dy);
                        pixels[py * side + px] = 0;
                    }
                }
            }
        }
        (side, side, pixels)
    }

    fn decode(qr: &Qr) -> String {
        let (w, h, pixels) = picture(qr, 6);
        let mut image = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| pixels[y * w + x]);
        let grids = image.detect_grids();
        assert_eq!(grids.len(), 1, "exactly one code in the picture");
        let (_, content) = grids[0].decode().expect("the code decodes");
        content
    }

    #[test]
    fn the_code_carries_exactly_the_link() {
        let qr = encode(LINK).unwrap();
        assert_eq!(decode(&qr), LINK);
    }

    #[test]
    fn the_code_has_a_standard_size_and_a_quiet_zone() {
        let qr = encode(LINK).unwrap();
        // 48 bytes at level M: version 4, 33 modules a side.
        assert_eq!(qr.size, 33);
        assert_eq!(qr.quiet, 4);
        // The three finder patterns (7x7: dark ring, light ring, dark centre) sit in three
        // corners; the fourth corner has none.
        let grid = modules_of(&qr);
        let n = qr.size as usize;
        let finder_at = |x0: usize, y0: usize| {
            (0..7).all(|dy| {
                (0..7).all(|dx| {
                    let edge = dx == 0 || dx == 6 || dy == 0 || dy == 6;
                    let centre = (2..=4).contains(&dx) && (2..=4).contains(&dy);
                    grid[y0 + dy][x0 + dx] == (edge || centre)
                })
            })
        };
        assert!(finder_at(0, 0) && finder_at(n - 7, 0) && finder_at(0, n - 7));
        assert!(!finder_at(n - 7, n - 7));
    }

    #[test]
    fn the_same_link_always_gives_the_same_code() {
        assert_eq!(encode(LINK), encode(LINK));
        assert_ne!(
            encode(LINK).unwrap().path,
            encode("https://relay.example.org/l/pair#4822-crayon-renard")
                .unwrap()
                .path
        );
    }

    #[test]
    fn a_local_relay_link_is_drawn_too() {
        let link = "https://127.0.0.1:8080/l/pair#9999-zoologie-zoologie";
        assert_eq!(decode(&encode(link).unwrap()), link);
    }

    #[test]
    fn what_cannot_be_encoded_gives_nothing() {
        assert!(encode(&"x".repeat(4000)).is_none());
    }

    #[test]
    fn debug_hides_the_path() {
        let shown = format!("{:?}", encode(LINK).unwrap());
        assert!(
            shown.contains("redacted") && !shown.contains('M'),
            "{shown}"
        );
    }
}
