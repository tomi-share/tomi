//! Content of the demo space (`runtime::demo`), item for item the demo feed of the Android
//! app (`apps/android/.../core/FakeTomiCore.kt`), so that every client shows the same demo.
//!
//! Binary demo files (images, code files) are read from the `assets_dir` given by the app
//! (Android keeps them in `app/src/debug/assets/demo/`); when one is missing a small
//! built-in stand-in is used instead, so the demo never fails for a missing asset.

use crate::item::SourceFormat;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Device that writes a demo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Author {
    /// "PC-Bureau" (Windows), also the founder of the demo space.
    Office,
    /// "PC-Maison" (Windows).
    Home,
    /// This device, in the past (items "sent from this phone").
    This,
}

/// A file attached to a demo item.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DemoFile {
    /// Bundled demo file of the app, by name (image or code file).
    Asset {
        name: &'static str,
        mime: &'static str,
    },
    /// One-page PDF generated on the fly (ASCII `title` printed on it).
    Pdf {
        name: &'static str,
        title: &'static str,
    },
    /// Small text file generated on the fly.
    Text {
        name: &'static str,
        mime: &'static str,
        content: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DemoText {
    pub plain: &'static str,
    pub markdown: Option<&'static str>,
    pub html: Option<&'static str>,
    pub source_format: SourceFormat,
}

const fn plain(text: &'static str) -> Option<DemoText> {
    Some(DemoText {
        plain: text,
        markdown: None,
        html: None,
        source_format: SourceFormat::Plain,
    })
}

/// One item of the demo feed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DemoEntry {
    /// Short stable name (tests, debugging); never shown.
    pub key: &'static str,
    /// How long ago it was written.
    pub age_ms: i64,
    pub from: Author,
    pub title: Option<&'static str>,
    pub text: Option<DemoText>,
    pub url: Option<&'static str>,
    pub files: &'static [DemoFile],
    pub secret: bool,
    pub after_first_read: bool,
    /// Expiration chosen by the sender, counted from the creation.
    pub expires_after_ms: Option<i64>,
    /// Already read on this device.
    pub read: bool,
    pub pinned: bool,
    pub archived: bool,
    /// Sealed in a format this app does not know yet ("update required").
    pub unreadable: bool,
    /// PC-Maison catches up right after this item (its receipts make the item "delivered
    /// to all" while later items stay "sent" to it).
    pub home_catches_up: bool,
}

const BASE: DemoEntry = DemoEntry {
    key: "",
    age_ms: 0,
    from: Author::Office,
    title: None,
    text: None,
    url: None,
    files: &[],
    secret: false,
    after_first_read: false,
    expires_after_ms: None,
    read: true,
    pinned: false,
    archived: false,
    unreadable: false,
    home_catches_up: false,
};

pub(crate) const PORTRAIT: &str = "demo_portrait_exif.jpg";
pub(crate) const LANDSCAPE: &str = "demo_paysage.jpg";
const CODE_RUST: &str = "demo_main.rs";
const CODE_JS: &str = "demo_app.js";
const CODE_KOTLIN: &str = "demo_notes.kt";
const CODE_PHP: &str = "demo_index.php";
const CODE_PYTHON: &str = "demo_secret.py";

const fn asset(name: &'static str, mime: &'static str) -> DemoFile {
    DemoFile::Asset { name, mime }
}

const IMG_PORTRAIT: DemoFile = asset(PORTRAIT, "image/jpeg");
const IMG_LANDSCAPE: DemoFile = asset(LANDSCAPE, "image/jpeg");
const RS: DemoFile = asset(CODE_RUST, "text/x-rust");
const JS: DemoFile = asset(CODE_JS, "text/javascript");
const KT: DemoFile = asset(CODE_KOTLIN, "text/x-kotlin");
const PHP: DemoFile = asset(CODE_PHP, "application/x-httpd-php");
const PY: DemoFile = asset(CODE_PYTHON, "text/x-python");

const MARKDOWN_CODE: &str = r#"# Notes de code

Un **exemple** de bloc Rust, coloré par tree-sitter :

```rust
// Additionne deux nombres
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    let total = add(40, 2);
    println!("Total = {total}");
}
```

Puis un bloc JavaScript :

```js
const greet = (name) => 'Bonjour ' + name + ' !';
console.log(greet('Tomi'), 42, true, null);
```

PHP a sa propre grammaire tree-sitter (hors Enriched Markdown) :

```php
function add(int $a, int $b): int {
    return $a + $b; // somme
}

echo "Total : " . add(40, 2) . PHP_EOL;
```

![pixel de suivi](https://example.org/tracking.png)

L'image ci-dessus n'est jamais téléchargée."#;

/// The demo feed, in any order (the demo writes them oldest first).
pub(crate) fn entries() -> Vec<DemoEntry> {
    vec![
        DemoEntry {
            key: "link",
            age_ms: 2 * MINUTE,
            title: Some("Material Design 3 dans Compose"),
            url: Some("https://developer.android.com/develop/ui/compose/designsystems/material3"),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: "secret-wifi",
            age_ms: 9 * MINUTE,
            title: Some("Wi-Fi invités"),
            text: plain("Hx7!pQ2-demo-zR9"),
            secret: true,
            read: false,
            pinned: true,
            ..BASE
        },
        DemoEntry {
            key: "markdown-courses",
            age_ms: 35 * MINUTE,
            text: Some(DemoText {
                plain: "Courses\nLait\nPain\nPensez à vérifier la date.",
                markdown: Some(
                    "# Courses\n\n- Lait\n- Pain\n\n**Pensez** à vérifier la [date](https://example.org).",
                ),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            // The only demo item with a time limit: the sender picked "1 day".
            expires_after_ms: Some(DAY),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: "markdown-code",
            age_ms: 12 * MINUTE,
            from: Author::Home,
            text: Some(DemoText {
                plain: "Notes de code\nBloc Rust, bloc JavaScript, bloc PHP.",
                markdown: Some(MARKDOWN_CODE),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: "code-multi",
            age_ms: 18 * MINUTE,
            title: Some("Projet démo (4 fichiers)"),
            files: &[RS, JS, KT, PHP],
            read: false,
            ..BASE
        },
        DemoEntry {
            key: "code-php",
            age_ms: 22 * MINUTE,
            from: Author::Home,
            files: &[PHP],
            read: false,
            ..BASE
        },
        DemoEntry {
            key: "code-rust",
            age_ms: 25 * MINUTE,
            files: &[RS],
            ..BASE
        },
        DemoEntry {
            key: "code-mixed",
            age_ms: 40 * MINUTE,
            from: Author::Home,
            title: Some("Capture et script"),
            files: &[IMG_LANDSCAPE, JS],
            ..BASE
        },
        DemoEntry {
            key: "secret-code",
            age_ms: 45 * MINUTE,
            title: Some("Script de connexion (démo)"),
            files: &[PY],
            secret: true,
            ..BASE
        },
        DemoEntry {
            key: "sent-link",
            age_ms: 2 * HOUR,
            from: Author::This,
            title: Some("Itinéraire du week-end"),
            url: Some("https://example.org/itineraire"),
            pinned: true,
            home_catches_up: true,
            ..BASE
        },
        DemoEntry {
            key: "html",
            age_ms: 5 * HOUR,
            from: Author::Home,
            text: Some(DemoText {
                plain: "Réunion jeudi\nOrdre du jour : budget, planning.",
                markdown: None,
                html: Some(
                    "<h2>Réunion jeudi</h2><p>Ordre du jour : <b>budget</b>, <i>planning</i>.</p>\
                     <script>alert('never runs')</script>",
                ),
                source_format: SourceFormat::Html,
            }),
            ..BASE
        },
        DemoEntry {
            key: "image-landscape",
            age_ms: 8 * HOUR,
            files: &[IMG_LANDSCAPE],
            ..BASE
        },
        // Portrait photo stored sideways with EXIF orientation 6: viewers must show it upright.
        DemoEntry {
            key: "image-portrait",
            age_ms: 20 * MINUTE,
            files: &[IMG_PORTRAIT],
            read: false,
            ..BASE
        },
        DemoEntry {
            key: "image-multi",
            age_ms: 50 * MINUTE,
            from: Author::Home,
            title: Some("Photos du week-end"),
            files: &[IMG_LANDSCAPE, IMG_PORTRAIT, IMG_LANDSCAPE],
            ..BASE
        },
        DemoEntry {
            key: "secret-image",
            age_ms: 15 * MINUTE,
            title: Some("Photo de la carte (démo)"),
            files: &[IMG_PORTRAIT],
            secret: true,
            ..BASE
        },
        DemoEntry {
            key: "files",
            age_ms: DAY,
            from: Author::Home,
            title: Some("Contrat"),
            files: &[
                DemoFile::Pdf {
                    name: "contrat-signe.pdf",
                    title: "Contrat signe (demo Tomi)",
                },
                DemoFile::Text {
                    name: "annexe.txt",
                    mime: "text/plain",
                    content: "Annexe du contrat (document de démonstration Tomi).\n",
                },
            ],
            ..BASE
        },
        // Sent from this device after PC-Maison caught up: delivered to PC-Bureau only.
        DemoEntry {
            key: "sent-secret",
            age_ms: 90 * MINUTE,
            from: Author::This,
            text: plain("Code-demo-4821-Kp"),
            secret: true,
            after_first_read: true,
            ..BASE
        },
        DemoEntry {
            key: "unreadable",
            age_ms: DAY + 6 * HOUR,
            from: Author::Home,
            unreadable: true,
            ..BASE
        },
        DemoEntry {
            key: "text-digicode",
            age_ms: 2 * DAY,
            text: plain("Digicode du portail : voir la note, ne pas répondre à ce message."),
            ..BASE
        },
        // Archived items: the "Archives" filter and the un-archive gesture.
        DemoEntry {
            key: "archived-link",
            age_ms: 9 * DAY,
            from: Author::Home,
            title: Some("Recette de la tarte aux pommes"),
            url: Some("https://example.org/recettes/tarte-aux-pommes"),
            archived: true,
            ..BASE
        },
        DemoEntry {
            key: "archived-text",
            age_ms: 12 * DAY,
            text: plain("Numéro de suivi du colis : 6A12345678901 (livré mardi dernier)."),
            archived: true,
            ..BASE
        },
        // Archived while still unread: archiving does not read, so the secret survives.
        DemoEntry {
            key: "archived-secret",
            age_ms: 14 * DAY,
            title: Some("Code du coffre (démo)"),
            text: plain("4821-Kp-demo"),
            secret: true,
            after_first_read: true,
            read: false,
            archived: true,
            ..BASE
        },
        DemoEntry {
            key: "archived-file",
            age_ms: 20 * DAY,
            from: Author::Home,
            title: Some("Devis plombier"),
            files: &[DemoFile::Pdf {
                name: "devis-plombier.pdf",
                title: "Devis plombier (demo Tomi)",
            }],
            archived: true,
            ..BASE
        },
    ]
}

/// What `simulate_incoming` sends from PC-Bureau (same contents as the Android demo).
pub(crate) fn incoming(kind: super::demo::DemoIncoming) -> DemoEntry {
    use super::demo::DemoIncoming as K;
    match kind {
        K::Link => DemoEntry {
            key: "incoming-link",
            title: Some("Documentation Android"),
            url: Some("https://developer.android.com/"),
            read: false,
            ..BASE
        },
        K::Text => DemoEntry {
            key: "incoming-text",
            text: plain("Rendez-vous à 14 h salle B"),
            read: false,
            ..BASE
        },
        K::RichText => DemoEntry {
            key: "incoming-rich-text",
            text: Some(DemoText {
                plain: "Titre\nUn paragraphe.",
                markdown: Some("# Titre\n\nUn **paragraphe**."),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            read: false,
            ..BASE
        },
        K::Image => DemoEntry {
            key: "incoming-image",
            files: &[IMG_PORTRAIT],
            read: false,
            ..BASE
        },
        K::File => DemoEntry {
            key: "incoming-file",
            files: &[DemoFile::Pdf {
                name: "facture.pdf",
                title: "Facture (demo Tomi)",
            }],
            read: false,
            ..BASE
        },
        K::Secret => DemoEntry {
            key: "incoming-secret",
            text: plain("Tr0ub4dor&3-demo"),
            secret: true,
            after_first_read: true,
            read: false,
            ..BASE
        },
        K::Unreadable => DemoEntry {
            key: "incoming-unreadable",
            unreadable: true,
            read: false,
            ..BASE
        },
    }
}

/// Built-in stand-in for a bundled code file the app did not provide.
pub(crate) fn fallback_code(name: &str) -> Option<&'static str> {
    Some(match name {
        CODE_RUST => "// Demo Tomi\nfn main() {\n    println!(\"Bonjour\");\n}\n",
        CODE_JS => "// Demo Tomi\nconsole.log('Bonjour');\n",
        CODE_KOTLIN => "// Demo Tomi\nfun main() = println(\"Bonjour\")\n",
        CODE_PHP => "<?php\n// Demo Tomi\necho \"Bonjour\" . PHP_EOL;\n",
        CODE_PYTHON => "# Demo Tomi\nprint(\"Bonjour\")\n",
        _ => return None,
    })
}

/// 1 x 1 PNG, stand-in for a bundled image the app did not provide.
pub(crate) const FALLBACK_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x68, 0x58, 0xF8,
    0x1F, 0x00, 0x04, 0x7F, 0x02, 0x7E, 0x6B, 0x43, 0xA5, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A valid one-page PDF printing `title` (ASCII; other characters become `?`).
pub(crate) fn pdf(title: &str) -> Vec<u8> {
    let safe: String = title
        .chars()
        .map(|c| match c {
            '(' | ')' | '\\' => ' ',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '?',
        })
        .collect();
    let content = format!("BT /F1 20 Tf 72 760 Td ({safe}) Tj ET");
    let objects = [
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
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_feed_mirrors_the_android_demo() {
        let all = entries();
        assert_eq!(all.len(), 23);
        assert_eq!(all.iter().filter(|e| e.archived).count(), 4);
        assert_eq!(all.iter().filter(|e| e.pinned).count(), 2);
        assert_eq!(all.iter().filter(|e| e.from == Author::This).count(), 2);
        assert!(
            all.iter().all(|e| e.age_ms < 30 * DAY),
            "within the relay retention"
        );
        let mut keys: Vec<_> = all.iter().map(|e| e.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), all.len(), "unique keys");
        // The item PC-Maison catches up after is older than every other item sent from here.
        let catch_up = all.iter().find(|e| e.home_catches_up).map(|e| e.age_ms);
        assert!(all
            .iter()
            .filter(|e| e.from == Author::This && !e.home_catches_up)
            .all(|e| Some(e.age_ms) < catch_up));
    }

    #[test]
    fn the_generated_pdf_has_a_consistent_cross_reference_table() {
        let p = pdf("Devis (été)");
        let text = String::from_utf8(p.clone()).unwrap();
        assert!(text.starts_with("%PDF-1.4\n") && text.ends_with("%%EOF\n"));
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .and_then(|t| t.lines().next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert!(text[start..].starts_with("xref\n"));
        for (i, line) in text[start..].lines().skip(3).take(5).enumerate() {
            let offset: usize = line[..10].parse().unwrap();
            assert!(text[offset..].starts_with(&format!("{} 0 obj", i + 1)));
            assert_eq!(line.len() + 1, 20, "xref entries are 20 bytes");
        }
        assert!(text.contains("(Devis  ?t? ) Tj"));
    }
}
