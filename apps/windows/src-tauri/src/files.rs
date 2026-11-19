//! File helpers: names, MIME types, thumbnails, unique destination paths.

use std::path::{Path, PathBuf};

use base64::Engine;

use crate::model::Attachment;

const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
const MAX_NAME_CHARS: usize = 120;

/// Thumbnails fit in 600 x 800 px: the Android card shows one across its whole width (about
/// 760 px on a phone), and the former 160 px one was visibly blurred there. Same bounds as
/// `ThumbnailSize` / `DraftThumbnails` on Android.
pub const THUMB_MAX_WIDTH: u32 = 600;
pub const THUMB_MAX_HEIGHT: u32 = 800;
const THUMB_MAX_TALL_RATIO: f64 = 4.0 / 3.0;
/// One thumbnail travels inside the item (in base64), which the relay caps at about 1 MB.
pub const THUMB_MAX_BYTES: usize = 120 * 1024;
/// A lossless PNG is kept only when it is this small; heavier, JPEG does better.
const THUMB_PNG_MAX_BYTES: usize = 64 * 1024;
const THUMB_JPEG_QUALITIES: [u8; 3] = [80, 65, 50];
const THUMB_SHRINK_STEPS: [f64; 3] = [1.0, 0.75, 0.55];

/// Makes an untrusted file name safe to create on Windows: no directory parts, no
/// reserved characters or device names, bounded length. Never returns an empty name.
pub fn sanitize_file_name(name: &str) -> String {
    // Keep only the last path component, whatever the separator.
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.chars().count() > MAX_NAME_CHARS {
        // Keep the extension when truncating.
        let ext = Path::new(&cleaned)
            .extension()
            .and_then(|e| e.to_str())
            .filter(|e| e.chars().count() <= 10)
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let keep = MAX_NAME_CHARS - ext.chars().count();
        cleaned = cleaned.chars().take(keep).collect::<String>() + &ext;
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return "fichier".to_string();
    }
    if RESERVED_NAMES.contains(&stem.as_str()) {
        return format!("_{cleaned}");
    }
    cleaned
}

/// `dir/name`, or `dir/name (1).ext`, `(2)`... when it already exists.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name = sanitize_file_name(name);
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(&name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("fichier");
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    (1..10_000)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or_else(|| dir.join(format!("{stem} ({}){ext}", uuid::Uuid::new_v4())))
}

pub fn guess_mime(name: &str) -> String {
    mime_guess::from_path(name)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

/// Framing of a thumbnail: the rows `top .. top + crop_height` of the image (all its width),
/// scaled by `scale` (never above 1). A tall image is cut to its centre at
/// [`THUMB_MAX_TALL_RATIO`] (height / width), as the cards crop it anyway. Same rule as
/// `ThumbnailSize` on Android, so both clients send the same kind of thumbnail.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ThumbPlan {
    top: u32,
    crop_height: u32,
    scale: f64,
}

fn thumb_plan(width: u32, height: u32) -> ThumbPlan {
    let width = width.max(1);
    let tall = (f64::from(width) * THUMB_MAX_TALL_RATIO).ceil() as u32;
    let crop_height = height.min(tall).max(1);
    let top = height.saturating_sub(crop_height) / 2;
    let scale = 1f64
        .min(f64::from(THUMB_MAX_WIDTH) / f64::from(width))
        .min(f64::from(THUMB_MAX_HEIGHT) / f64::from(crop_height));
    ThumbPlan {
        top,
        crop_height,
        scale,
    }
}

/// A transparent image drawn on white (JPEG has no alpha: it would turn black).
fn flatten_on_white(img: &image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p[3]);
        let over = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
        out.put_pixel(x, y, image::Rgb([over(p[0]), over(p[1]), over(p[2])]));
    }
    out
}

/// The encoded thumbnail and its MIME type, or `None` when it does not fit in
/// [`THUMB_MAX_BYTES`]. Lossless PNG when the image stays small that way (screenshots, flat
/// graphics, transparency), JPEG otherwise (photos).
fn encode_thumbnail(img: &image::DynamicImage) -> Option<(Vec<u8>, &'static str)> {
    let has_alpha = img.color().has_alpha();
    let mut png = Vec::new();
    if img
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .is_ok()
        && png.len()
            <= if has_alpha {
                THUMB_MAX_BYTES
            } else {
                THUMB_PNG_MAX_BYTES
            }
    {
        return Some((png, "image/png"));
    }
    let rgb = image::DynamicImage::ImageRgb8(if has_alpha {
        flatten_on_white(img)
    } else {
        img.to_rgb8()
    });
    for quality in THUMB_JPEG_QUALITIES {
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality);
        if rgb.write_with_encoder(encoder).is_ok() && jpeg.len() <= THUMB_MAX_BYTES {
            return Some((jpeg, "image/jpeg"));
        }
    }
    None
}

/// Thumbnail of an image file: upright (EXIF), framed by [`thumb_plan`], at most
/// [`THUMB_MAX_BYTES`]. If the first try is too heavy, the image is shrunk a little and
/// encoded again before giving up (no thumbnail is better than a refused item).
fn thumbnail_bytes(path: &Path) -> Option<(Vec<u8>, &'static str)> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);

    let plan = thumb_plan(img.width(), img.height());
    let framed = img.crop_imm(0, plan.top, img.width(), plan.crop_height);
    for shrink in THUMB_SHRINK_STEPS {
        let scale = plan.scale * shrink;
        let small = if scale < 1.0 {
            let w = (f64::from(framed.width()) * scale).round().max(1.0) as u32;
            let h = (f64::from(framed.height()) * scale).round().max(1.0) as u32;
            framed.resize_exact(w, h, image::imageops::FilterType::Lanczos3)
        } else {
            framed.clone()
        };
        if let Some(found) = encode_thumbnail(&small) {
            return Some(found);
        }
    }
    None
}

/// Thumbnail as a `data:` URL (PNG or JPEG), for image files only.
pub fn thumbnail_data_url(path: &Path) -> Option<String> {
    let (bytes, mime) = thumbnail_bytes(path)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Metadata of a local regular file as an attachment (thumbnail for images). Folders,
/// devices and anything else that is not a file are refused.
pub fn describe(path: &Path) -> std::io::Result<Attachment> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(sanitize_file_name)
        .unwrap_or_else(|| "fichier".to_string());
    let mime = guess_mime(&name);
    let thumbnail = if mime.starts_with("image/") {
        thumbnail_data_url(path)
    } else {
        None
    };
    Ok(Attachment {
        name,
        mime,
        size: meta.len(),
        path: Some(path.to_string_lossy().into_owned()),
        thumbnail,
        available: true,
        slot: None,
    })
}

/// Extensions that run code when opened from Explorer. A file received from the network
/// with one of these is never launched by the app: its folder is shown instead, so the
/// user makes the decision (and SmartScreen gets a say when they double-click it).
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    // The set the owner asked for.
    "exe",
    "lnk",
    "js",
    "hta",
    "bat",
    "cmd",
    "ps1",
    "scr",
    "msi",
    "com",
    "vbs",
    "wsf",
    // Same risk, same handling.
    "vbe",
    "jse",
    "wsh",
    "pif",
    "cpl",
    "reg",
    "jar",
    "url",
    "msp",
    // Windows shortcuts, installers, scripts and components that run or load code.
    "chm",
    "msc",
    "appref-ms",
    "application",
    "settingcontent-ms",
    "library-ms",
    "search-ms",
    "psm1",
    "psc1",
    "inf",
    "sct",
    "wsc",
    "ws",
    "scf",
    "gadget",
    "mst",
    "xll",
    "wll",
    "diagcab",
    "msix",
    "appx",
    "msixbundle",
    "appxbundle",
    "appinstaller",
    // Disk images: mounted by a double-click, whose content then runs without the
    // Mark-of-the-Web (a classic way round SmartScreen).
    "iso",
    "img",
    "vhd",
    "vhdx",
    // Remote Desktop / Citrix connection files: opening one connects to the sender's server with
    // drive and clipboard redirection (a real phishing technique).
    "rdp",
    "rdg",
    "ica",
    // Interpreters that run a script directly when one is installed.
    "py",
    "pyw",
    "pyc",
    "pyo",
    "pyz",
    "pyzw",
    "pl",
    "vb",
    "vbp",
    "csh",
    "ksh",
    "ps2",
    "ps2xml",
    "ps1xml",
    "psc2",
    "psd1",
    "psdm1",
    "msh",
    "msh1",
    "msh2",
    "mshxml",
    "msh1xml",
    "msh2xml",
    // Themes and connectors that reach out to a remote path (NTLM hash leak) or load code.
    "theme",
    "themepack",
    "deskthemepack",
    "searchconnector-ms",
    "website",
    "webpnp",
    "xbap",
    "xnk",
    "vsto",
    "vsix",
    "vsmacros",
    "vsw",
    // Help and archive formats that run content, and web archives that open in a script-capable
    // browser context.
    "hlp",
    "cab",
    "jnlp",
    "mht",
    "mhtml",
    "msu",
    "printerexport",
    "ops",
    "osd",
    "shs",
    "shb",
    "bgi",
    // Certificates and Office / Access macro carriers from the "dangerous file types" list of
    // the Windows Attachment Manager.
    "cer",
    "crt",
    "der",
    "ade",
    "adp",
    "mad",
    "maf",
    "mag",
    "mam",
    "maq",
    "mar",
    "mas",
    "mat",
    "mau",
    "mav",
    "maw",
    "mcf",
    "mda",
    "mdb",
    "mde",
    "mdt",
    "mdw",
    "mdz",
    "pst",
];

/// True for files that execute code when opened. Case-insensitive, last extension only.
pub fn is_executable_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            EXECUTABLE_EXTENSIONS
                .iter()
                .any(|x| x.eq_ignore_ascii_case(e))
        })
}

/// Sets or clears the read-only attribute (on Windows, `FILE_ATTRIBUTE_READONLY`; the names of a
/// hard-linked file share it).
pub fn set_readonly(path: &Path, readonly: bool) -> std::io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    if perms.readonly() == readonly {
        return Ok(());
    }
    // On Windows this only toggles the attribute (no mode bits are involved).
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(readonly);
    std::fs::set_permissions(path, perms)
}

/// Content written in the `Zone.Identifier` stream: zone 3 = Internet.
const ZONE_IDENTIFIER: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";

/// Marks a file received from the network with the Mark-of-the-Web, an NTFS alternate
/// data stream named `Zone.Identifier` (`ZoneId=3`), so that SmartScreen, Office
/// Protected View and the "downloaded from the Internet" prompt apply to it.
/// Fails (harmlessly) on file systems without streams.
#[cfg(windows)]
pub fn mark_of_the_web(path: &Path) -> std::io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    // A stream cannot be written to a read-only file (the opened copies are: see
    // `real_core::named_copy`): lift the attribute for the write, then put it back.
    let was_readonly = std::fs::metadata(path)
        .map(|m| m.permissions().readonly())
        .unwrap_or(false);
    if was_readonly {
        set_readonly(path, false)?;
    }
    let written = std::fs::write(stream, ZONE_IDENTIFIER);
    if was_readonly {
        let _ = set_readonly(path, true);
    }
    written
}

/// There is no such thing as the Mark-of-the-Web outside Windows.
#[cfg(not(windows))]
pub fn mark_of_the_web(_path: &Path) -> std::io::Result<()> {
    let _ = ZONE_IDENTIFIER;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_extensions_are_detected_case_insensitively() {
        for n in [
            "a.exe",
            "a.EXE",
            "a.Lnk",
            "a.js",
            "a.hta",
            "a.bat",
            "a.cmd",
            "a.ps1",
            "a.scr",
            "a.msi",
            "a.com",
            "a.vbs",
            "a.wsf",
            "x.tar.exe",
            "a.url",
            "a.chm",
            "a.msc",
            "a.appref-ms",
            "a.application",
            "a.settingcontent-ms",
            "a.library-ms",
            "a.search-ms",
            "a.psm1",
            "a.inf",
            "a.sct",
            "a.wsc",
            "a.ws",
            "a.xll",
            "a.diagcab",
            "a.ISO",
            "a.img",
            "a.vhd",
            "a.vhdx",
            "a.msix",
            "a.appx",
            "a.appinstaller",
            // Audit W4.
            "a.rdp",
            "a.RDP",
            "a.py",
            "a.pyw",
            "a.pyz",
            "a.theme",
            "a.themepack",
            "a.deskthemepack",
            "a.searchConnector-ms",
            "a.website",
            "a.xbap",
            "a.hlp",
            "a.jnlp",
            "a.ps1xml",
            "a.psd1",
            "a.mht",
            "a.MHTML",
            "a.vb",
            "a.cab",
            "a.msu",
            "a.cer",
            "a.mde",
            "a.ica",
            "a.rdg",
            "a.vsto",
        ] {
            assert!(is_executable_name(n), "{n}");
        }
        for n in [
            "a.pdf",
            "a.txt",
            "a.png",
            "exe",
            "a.exe.txt",
            "a",
            "a.jsx",
            "a.isos",
        ] {
            assert!(!is_executable_name(n), "{n}");
        }
    }

    #[test]
    fn the_executable_list_is_lower_case_and_has_no_duplicate() {
        let mut seen = std::collections::HashSet::new();
        for ext in EXECUTABLE_EXTENSIONS {
            assert_eq!(*ext, ext.to_ascii_lowercase(), "{ext}");
            assert!(seen.insert(*ext), "{ext} is listed twice");
        }
    }

    #[cfg(windows)]
    #[test]
    fn mark_of_the_web_writes_zone_id_3_in_the_alternate_stream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recu.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        mark_of_the_web(&path).unwrap();
        // The file itself is untouched.
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF");
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let zone = std::fs::read_to_string(stream).unwrap();
        assert!(zone.contains("[ZoneTransfer]"));
        assert!(zone.contains("ZoneId=3"));
        // Idempotent.
        mark_of_the_web(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn mark_of_the_web_works_on_a_read_only_file_and_keeps_it_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recu.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        set_readonly(&path, true).unwrap();
        mark_of_the_web(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        assert!(std::fs::read_to_string(stream)
            .unwrap()
            .contains("ZoneId=3"));
        set_readonly(&path, false).unwrap();
    }

    #[test]
    fn describe_refuses_folders() {
        let dir = tempfile::tempdir().unwrap();
        assert!(describe(dir.path()).is_err());
    }

    #[test]
    fn sanitises_names() {
        assert_eq!(sanitize_file_name("a/b\\c.txt"), "c.txt");
        assert_eq!(sanitize_file_name("..\\..\\evil.dll"), "evil.dll");
        assert_eq!(sanitize_file_name("a<b>:c|d?.txt"), "a_b__c_d_.txt");
        assert_eq!(sanitize_file_name("nom. "), "nom");
        assert_eq!(sanitize_file_name(""), "fichier");
        assert_eq!(sanitize_file_name(".."), "fichier");
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_file_name("nul"), "_nul");
        assert_eq!(sanitize_file_name("tab\tname"), "tab_name");
    }

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.pdf", "a".repeat(300));
        let out = sanitize_file_name(&long);
        assert_eq!(out.chars().count(), MAX_NAME_CHARS);
        assert!(out.ends_with(".pdf"));
    }

    #[test]
    fn unique_path_adds_a_counter() {
        let dir = tempfile::tempdir().unwrap();
        let first = unique_path(dir.path(), "a.txt");
        assert_eq!(first, dir.path().join("a.txt"));
        std::fs::write(&first, b"x").unwrap();
        let second = unique_path(dir.path(), "a.txt");
        assert_eq!(second, dir.path().join("a (1).txt"));
        std::fs::write(&second, b"x").unwrap();
        assert_eq!(
            unique_path(dir.path(), "a.txt"),
            dir.path().join("a (2).txt")
        );
    }

    #[test]
    fn guesses_mime() {
        assert_eq!(guess_mime("x.png"), "image/png");
        assert_eq!(guess_mime("x.pdf"), "application/pdf");
        assert_eq!(guess_mime("x.unknownext"), "application/octet-stream");
    }

    #[test]
    fn describes_an_image_with_a_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.png");
        image::RgbImage::from_pixel(400, 200, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let a = describe(&path).unwrap();
        assert_eq!(a.name, "pic.png");
        assert_eq!(a.mime, "image/png");
        assert!(a.size > 0);
        assert!(a.thumbnail.unwrap().starts_with("data:image/png;base64,"));
    }

    fn decode_data_url(url: &str) -> image::DynamicImage {
        let (_, b64) = url.split_once(";base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        image::load_from_memory(&bytes).unwrap()
    }

    #[test]
    fn the_thumbnail_plan_matches_android() {
        // A tall screenshot is cut to its centre and keeps its width.
        let p = thumb_plan(1080, 2340);
        assert_eq!((p.top, p.crop_height), ((2340 - 1440) / 2, 1440));
        assert!((p.scale - 600.0 / 1080.0).abs() < 1e-9);
        // A landscape photo is only scaled down.
        let p = thumb_plan(4000, 3000);
        assert_eq!((p.top, p.crop_height), (0, 3000));
        assert!((p.scale - 0.15).abs() < 1e-9);
        // A small image is left as it is.
        let p = thumb_plan(100, 50);
        assert_eq!((p.top, p.crop_height, p.scale), (0, 50, 1.0));
    }

    #[test]
    fn a_photo_gets_a_sharp_thumbnail_within_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.png");
        // Busy picture, so that PNG is too heavy and JPEG is used.
        image::RgbImage::from_fn(2000, 1500, |x, y| {
            let n = x
                .wrapping_mul(2654435761)
                .wrapping_add(y.wrapping_mul(40503))
                >> 24;
            image::Rgb([
                ((x / 8 + n / 4) % 256) as u8,
                ((y / 6 + n / 4) % 256) as u8,
                ((x + y) / 10 % 256) as u8,
            ])
        })
        .save(&path)
        .unwrap();
        let url = thumbnail_data_url(&path).expect("a thumbnail");
        assert!(url.starts_with("data:image/jpeg;base64,"));
        let (_, b64) = url.split_once(";base64,").unwrap();
        assert!(b64.len() * 3 / 4 <= THUMB_MAX_BYTES);
        let img = decode_data_url(&url);
        // At least 0.55 x 600: far above the former 160 px.
        assert!(img.width() >= 330 && img.width() <= 600, "{}", img.width());
        assert!(img.height() <= 800);
    }

    #[test]
    fn a_tall_screenshot_keeps_its_width_and_is_cut_to_its_centre() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        image::RgbImage::from_pixel(1080, 2340, image::Rgb([240, 240, 240]))
            .save(&path)
            .unwrap();
        let img = decode_data_url(&thumbnail_data_url(&path).unwrap());
        assert_eq!((img.width(), img.height()), (600, 800));
    }

    #[test]
    fn a_small_image_is_not_enlarged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icon.png");
        image::RgbImage::from_pixel(64, 48, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let img = decode_data_url(&thumbnail_data_url(&path).unwrap());
        assert_eq!((img.width(), img.height()), (64, 48));
    }

    #[test]
    fn a_transparent_image_stays_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logo.png");
        image::RgbaImage::from_pixel(300, 300, image::Rgba([200, 0, 0, 0]))
            .save(&path)
            .unwrap();
        let url = thumbnail_data_url(&path).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(decode_data_url(&url).color().has_alpha());
    }

    #[test]
    fn transparency_is_flattened_on_white_for_jpeg() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([0, 0, 0, 0]),
        ));
        assert_eq!(flatten_on_white(&img).get_pixel(0, 0).0, [255, 255, 255]);
    }

    #[test]
    fn describes_a_non_image_without_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let a = describe(&path).unwrap();
        assert_eq!(a.size, 5);
        assert!(a.thumbnail.is_none());
    }
}
