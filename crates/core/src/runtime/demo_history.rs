//! The generated parts of the demo feed ([`super::demo_data`]): the "validation week" (every
//! kind of item over the last seven days, with heavy files) and about three years of ordinary
//! history (several items a day, mostly links and texts). Everything is fictitious: French
//! templates, `example.org` / `example.com` domains, no real person, deterministic seeds.

use std::borrow::Cow;

use super::demo_data::{
    plain, s, st, Author, DemoEntry, DemoFile, DemoText, Scale, Str, BASE, DAY, HOUR, MINUTE,
};
use super::demo_files::{self, CodeLang, Generated, Rng};
use crate::item::SourceFormat;

fn owned(text: String) -> Option<Str> {
    Some(Cow::Owned(text))
}

fn generated(name: impl Into<Str>, file: Generated) -> DemoFile {
    DemoFile::Generated {
        name: name.into(),
        file,
    }
}

fn markdown(plain: String, markdown: String) -> Option<DemoText> {
    Some(DemoText {
        plain: Cow::Owned(plain),
        markdown: owned(markdown),
        html: None,
        source_format: SourceFormat::Markdown,
    })
}

fn photo(width: u32, height: u32, seed: u64) -> Generated {
    Generated::Bmp {
        width,
        height,
        seed,
    }
}

/// Size of the file closest to the per-file limit of the apps (25 MiB, `ShareLimits` on iOS,
/// the bundled-file limit of the demo): just under it, so that it is accepted everywhere.
pub(crate) const NEAR_LIMIT_BYTES: u32 = 25 * 1024 * 1024 - 64 * 1024;

const RICH_HTML: &str = "<h1>Lettre d'information (démo)</h1>\
<p>Bonjour, voici les <b>nouvelles</b> du mois, avec un <a href=\"https://example.org/lettre\">lien</a> \
et une image distante jamais chargée : <img src=\"https://example.com/pixel.png\" alt=\"pixel\"></p>\
<h2>Au programme</h2><ul><li>Atelier <i>compost</i> samedi</li><li>Vide-grenier dimanche</li>\
<li>Fermeture exceptionnelle lundi</li></ul>\
<ol><li>S'inscrire</li><li>Venir</li></ol>\
<table><thead><tr><th>Jour</th><th>Horaire</th><th>Lieu</th></tr></thead>\
<tbody><tr><td>Samedi</td><td>10 h</td><td>Jardin partagé</td></tr>\
<tr><td>Dimanche</td><td>8 h</td><td>Place du marché</td></tr></tbody></table>\
<blockquote>« Venez nombreux ! »</blockquote><pre><code>horaires --semaine 46</code></pre>\
<p style=\"color:red\" onclick=\"alert(1)\">Texte stylé, sans script.</p>\
<script>alert('never runs')</script><iframe src=\"https://example.com/\"></iframe>";

const RICH_HTML_PLAIN: &str =
    "Lettre d'information (démo)\nBonjour, voici les nouvelles du mois.\n\
Au programme : atelier compost samedi, vide-grenier dimanche, fermeture exceptionnelle lundi.";

const MARKDOWN_LANGS: &str = "# Extraits de code\n\n\
Plusieurs langages dans un même message :\n\n\
```kotlin\nfun main() {\n    val noms = listOf(\"Ada\", \"Alan\")\n    println(noms.joinToString())\n}\n```\n\n\
```python\ndef carre(x: int) -> int:\n    return x * x\n\nprint([carre(n) for n in range(5)])\n```\n\n\
```sql\nSELECT titre, annee FROM livre WHERE annee > 2000 ORDER BY annee;\n```\n\n\
```json\n{ \"nom\": \"démo\", \"version\": 3, \"actif\": true }\n```\n\n\
```bash\nfor f in *.txt; do wc -l \"$f\"; done\n```\n\n\
Et du code en ligne : `cargo test --features demo`.";

/// Items of the validation week kept by the tests (one out of this many), each with this many
/// files at most: they open the demo many times, unoptimised. The whole week is checked item by
/// item without the engine (`demo_data` tests), and opened whole by the measure in release.
const TEST_WEEK_STEP: usize = 3;
const TEST_WEEK_FILES: usize = 2;

/// The validation week: over the last seven days, every kind of item the apps show, with
/// heavy generated files (small ones, and a sample of the items, in the tests).
pub(crate) fn validation_week(scale: Scale) -> Vec<DemoEntry> {
    let week = whole_week(scale);
    match scale {
        Scale::Full => week,
        Scale::Test => week
            .into_iter()
            .step_by(TEST_WEEK_STEP)
            .map(|mut e| {
                e.files.truncate(TEST_WEEK_FILES);
                e
            })
            .collect(),
    }
}

fn whole_week(scale: Scale) -> Vec<DemoEntry> {
    let n = |full: u32, test: u32| scale.pick(full, test);
    let (big_md, big_md_plain) = demo_files::markdown_document(n(200_000, 4_000) as usize, 11);
    let long_text = demo_files::long_text(n(30_000, 2_000) as usize, 12);
    let many_files: Vec<DemoFile> = (1..=12u32)
        .map(|i| {
            if i % 3 == 0 {
                DemoFile::Pdf {
                    name: Cow::Owned(format!("piece-{i:02}.pdf")),
                    title: Cow::Owned(format!("Piece {i} du dossier (demo Tomi)")),
                }
            } else {
                DemoFile::Text {
                    name: Cow::Owned(format!("piece-{i:02}.txt")),
                    mime: st("text/plain"),
                    content: Cow::Owned(format!(
                        "Pièce n° {i} du dossier de démonstration Tomi.\n"
                    )),
                }
            }
        })
        .collect();
    vec![
        // --- Heavy attachments ------------------------------------------------------------
        DemoEntry {
            key: st("week-photos"),
            age_ms: 3 * HOUR,
            from: Author::Home,
            title: s("Randonnée du dimanche (4 photos)"),
            files: (0..4u64)
                .map(|i| {
                    generated(
                        format!("randonnee-{:02}.bmp", i + 1),
                        photo(n(800, 48), n(600, 36), 100 + i),
                    )
                })
                .collect(),
            ..BASE
        },
        DemoEntry {
            key: st("week-large-photo"),
            age_ms: 5 * HOUR,
            title: s("Panorama (grande image)"),
            files: vec![generated(
                "panorama.bmp",
                photo(n(2000, 64), n(1500, 40), 110),
            )],
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-long-pdf"),
            age_ms: 8 * HOUR,
            from: Author::Home,
            title: s("Rapport annuel"),
            files: vec![generated(
                "rapport-annuel.pdf",
                Generated::Pdf {
                    title: st("Rapport annuel (demo Tomi)"),
                    pages: n(240, 3),
                    seed: 120,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-csv"),
            age_ms: 10 * HOUR,
            title: s("Relevés bancaires sur trois ans"),
            files: vec![generated(
                "releves-bancaires.csv",
                Generated::Csv {
                    rows: n(60_000, 20),
                    seed: 130,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-json"),
            age_ms: 14 * HOUR,
            from: Author::Home,
            title: s("Export des capteurs"),
            files: vec![generated(
                "export-capteurs.json",
                Generated::Json {
                    records: n(12_000, 5),
                    seed: 140,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-log"),
            age_ms: 20 * HOUR,
            from: Author::This,
            title: s("Journal de l'application"),
            files: vec![generated(
                "journal-application.txt",
                Generated::Log {
                    lines: n(60_000, 20),
                    seed: 150,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-code"),
            age_ms: DAY,
            title: s("Sources du projet démo (5 fichiers)"),
            files: vec![
                generated(
                    "releves.rs",
                    Generated::Code {
                        lang: CodeLang::Rust,
                        functions: n(300, 3),
                    },
                ),
                generated(
                    "Taches.kt",
                    Generated::Code {
                        lang: CodeLang::Kotlin,
                        functions: n(300, 3),
                    },
                ),
                generated(
                    "journal.py",
                    Generated::Code {
                        lang: CodeLang::Python,
                        functions: n(400, 3),
                    },
                ),
                generated(
                    "client.ts",
                    Generated::Code {
                        lang: CodeLang::TypeScript,
                        functions: n(300, 3),
                    },
                ),
                generated(
                    "bibliotheque.sql",
                    Generated::Code {
                        lang: CodeLang::Sql,
                        functions: n(400, 3),
                    },
                ),
            ],
            ..BASE
        },
        DemoEntry {
            key: st("week-audio"),
            age_ms: DAY + 3 * HOUR,
            from: Author::Home,
            title: s("Mémo vocal (démo)"),
            files: vec![generated(
                "memo-vocal.wav",
                Generated::Wav {
                    millis: n(45_000, 200),
                    seed: 160,
                },
            )],
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-zip"),
            age_ms: DAY + 8 * HOUR,
            title: s("Archive photos et relevés"),
            files: vec![generated(
                "archive-demo.zip",
                Generated::Zip {
                    image_side: n(640, 16),
                    csv_rows: n(5_000, 5),
                    seed: 170,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-near-limit"),
            age_ms: 2 * DAY,
            from: Author::This,
            title: s("Sauvegarde (presque 25 Mo)"),
            files: vec![generated(
                "sauvegarde.bin",
                Generated::Binary {
                    bytes: n(NEAR_LIMIT_BYTES, 4_096),
                    seed: 180,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-markdown-file"),
            age_ms: 2 * DAY + 2 * HOUR,
            from: Author::Home,
            title: s("Guide de la maison (fichier .md)"),
            files: vec![generated(
                "guide-maison.md",
                Generated::Markdown {
                    bytes: n(220_000, 3_000),
                    seed: 190,
                },
            )],
            ..BASE
        },
        DemoEntry {
            key: st("week-mixed-files"),
            age_ms: 2 * DAY + 5 * HOUR,
            title: s("Dossier sinistre (photo, PDF, tableau)"),
            files: vec![
                generated("degats.bmp", photo(n(1200, 40), n(900, 30), 200)),
                generated(
                    "declaration.pdf",
                    Generated::Pdf {
                        title: st("Declaration de sinistre (demo Tomi)"),
                        pages: n(12, 2),
                        seed: 201,
                    },
                ),
                generated(
                    "inventaire.csv",
                    Generated::Csv {
                        rows: n(2_000, 10),
                        seed: 202,
                    },
                ),
            ],
            ..BASE
        },
        DemoEntry {
            key: st("week-secret-photo"),
            age_ms: 3 * DAY,
            title: s("Carte de mutuelle (démo)"),
            files: vec![generated(
                "carte-mutuelle.bmp",
                photo(n(1000, 40), n(640, 26), 210),
            )],
            secret: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-sent-captures"),
            age_ms: 4 * DAY + 8 * HOUR,
            from: Author::This,
            title: s("Captures (3)"),
            files: (0..3u64)
                .map(|i| {
                    generated(
                        format!("capture-{}.bmp", i + 1),
                        photo(n(480, 24), n(300, 16), 220 + i),
                    )
                })
                .collect(),
            ..BASE
        },
        DemoEntry {
            key: st("week-many-files"),
            age_ms: 5 * DAY + 7 * HOUR,
            title: s("Pièces du dossier (12 fichiers)"),
            files: many_files,
            ..BASE
        },
        // --- Texts ------------------------------------------------------------------------
        DemoEntry {
            key: st("week-big-markdown"),
            age_ms: 4 * HOUR,
            text: markdown(big_md_plain, big_md),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-long-text"),
            age_ms: 6 * HOUR,
            from: Author::Home,
            title: s("Compte rendu détaillé"),
            text: Some(DemoText {
                plain: Cow::Owned(long_text),
                markdown: None,
                html: None,
                source_format: SourceFormat::Plain,
            }),
            ..BASE
        },
        DemoEntry {
            key: st("week-short-text"),
            age_ms: 2 * HOUR + 30 * MINUTE,
            text: plain("Ok pour samedi, je prends le pain."),
            ..BASE
        },
        DemoEntry {
            key: st("week-html"),
            age_ms: 9 * HOUR,
            from: Author::Home,
            text: Some(DemoText {
                plain: st(RICH_HTML_PLAIN),
                markdown: None,
                html: s(RICH_HTML),
                source_format: SourceFormat::Html,
            }),
            ..BASE
        },
        DemoEntry {
            key: st("week-markdown-code"),
            age_ms: 6 * DAY + 12 * HOUR,
            from: Author::Home,
            text: Some(DemoText {
                plain: st("Extraits de code\nKotlin, Python, SQL, JSON, Bash."),
                markdown: s(MARKDOWN_LANGS),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            ..BASE
        },
        DemoEntry {
            key: st("week-emoji"),
            age_ms: 5 * DAY + HOUR,
            from: Author::Home,
            text: plain("Joyeux anniversaire 🎉🎂 ! Rendez-vous à 19 h, côté jardin 🌳. Ça va être génial 😄"),
            ..BASE
        },
        DemoEntry {
            key: st("week-titled-text"),
            age_ms: 5 * DAY + 12 * HOUR,
            from: Author::This,
            title: s("Idées pour les vacances"),
            text: plain("Bord de mer en juillet, montagne en août ? À discuter dimanche."),
            ..BASE
        },
        // --- Links ------------------------------------------------------------------------
        DemoEntry {
            key: st("week-long-link"),
            age_ms: 11 * HOUR,
            from: Author::This,
            title: s("Comparatif détaillé des vélos électriques pour les trajets quotidiens en ville et à la campagne"),
            url: s("https://www.example.com/guides/mobilite/velos-electriques/comparatif-2026?tri=prix&ordre=croissant&categorie=urbain&page=3#autonomie"),
            ..BASE
        },
        DemoEntry {
            key: st("week-bare-link"),
            age_ms: 13 * HOUR,
            url: s("https://example.org/a/9f3k2"),
            ..BASE
        },
        DemoEntry {
            key: st("week-link-with-text"),
            age_ms: DAY + HOUR,
            from: Author::Home,
            title: s("Atelier réparation de vélos"),
            url: s("https://example.org/agenda/atelier-velo"),
            text: plain("On y va samedi ? Il faut s'inscrire avant jeudi."),
            ..BASE
        },
        DemoEntry {
            key: st("week-long-title"),
            age_ms: 6 * DAY + 20 * HOUR,
            from: Author::Home,
            title: s("Un titre volontairement très long pour vérifier la coupure des cartes du fil : liste complète des affaires à emporter pour le séjour à la montagne, avec les horaires de train, l'adresse du gîte et le plan d'accès"),
            url: s("https://example.org/sejour/montagne"),
            ..BASE
        },
        // --- Secrets ----------------------------------------------------------------------
        DemoEntry {
            key: st("week-secret-text"),
            age_ms: 16 * HOUR,
            title: s("Code de l'alarme (démo)"),
            text: plain("7314-demo"),
            secret: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-secret-once"),
            age_ms: DAY + 5 * HOUR,
            from: Author::Home,
            title: s("Mot de passe temporaire (démo)"),
            text: plain("Pz8-temp-demo-Lk2"),
            secret: true,
            after_first_read: true,
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-secret-once-sent"),
            age_ms: DAY + 10 * HOUR,
            from: Author::This,
            text: plain("Code-colis-demo-5512"),
            secret: true,
            after_first_read: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-secret-markdown"),
            age_ms: 2 * DAY + 8 * HOUR,
            title: s("Accès de la maison (démo)"),
            text: Some(DemoText {
                plain: st("Accès de la maison\nPortail : 2580-demo\nBoîte à clés : 0417-demo"),
                markdown: s("# Accès de la maison\n\n| Accès | Code |\n|---|---|\n| Portail | `2580-demo` |\n| Boîte à clés | `0417-demo` |"),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            secret: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-secret-expiring"),
            age_ms: 2 * HOUR,
            from: Author::Home,
            title: s("Code Wi-Fi du gîte (démo)"),
            text: plain("Gite-demo-2026!"),
            secret: true,
            expires_after_ms: Some(DAY),
            read: false,
            ..BASE
        },
        // --- Expirations ------------------------------------------------------------------
        DemoEntry {
            key: st("week-expires-hour"),
            age_ms: 25 * MINUTE,
            text: plain("Je suis devant l'entrée, porte B (message valable une heure)."),
            expires_after_ms: Some(HOUR),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-expires-day"),
            age_ms: 7 * HOUR,
            from: Author::Home,
            title: s("Billets du concert (lien valable un jour)"),
            url: s("https://billets.example.com/c/8812"),
            expires_after_ms: Some(DAY),
            ..BASE
        },
        DemoEntry {
            key: st("week-expires-week"),
            age_ms: 3 * DAY,
            from: Author::This,
            text: plain("Le code de la consigne change dimanche : 5541-demo."),
            expires_after_ms: Some(7 * DAY),
            ..BASE
        },
        DemoEntry {
            key: st("week-expires-month"),
            age_ms: 6 * DAY,
            text: Some(DemoText {
                plain: st("Travaux dans l'immeuble\nCoupure d'eau mardi de 9 h à 12 h."),
                markdown: s("# Travaux dans l'immeuble\n\nCoupure d'eau **mardi** de 9 h à 12 h."),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            expires_after_ms: Some(30 * DAY),
            ..BASE
        },
        // --- States -----------------------------------------------------------------------
        DemoEntry {
            key: st("week-unreadable"),
            age_ms: 4 * DAY,
            unreadable: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-pinned-markdown"),
            age_ms: 5 * DAY,
            from: Author::Home,
            title: s("Numéros utiles (démo)"),
            text: Some(DemoText {
                plain: st("Numéros utiles\nPlombier : 01 00 00 00 01\nSyndic : 01 00 00 00 02"),
                markdown: s("# Numéros utiles\n\n- Plombier : 01 00 00 00 01\n- Syndic : 01 00 00 00 02\n- Médiathèque : 01 00 00 00 03"),
                html: None,
                source_format: SourceFormat::Markdown,
            }),
            pinned: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-pinned-link"),
            age_ms: 6 * DAY + 2 * HOUR,
            title: s("Planning partagé de la semaine"),
            url: s("https://example.org/planning/semaine"),
            pinned: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-archived-pdf"),
            age_ms: 5 * DAY + 4 * HOUR,
            title: s("Bulletin de salaire (démo)"),
            files: vec![generated(
                "bulletin-demo.pdf",
                Generated::Pdf {
                    title: st("Bulletin (demo Tomi)"),
                    pages: 3,
                    seed: 230,
                },
            )],
            archived: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-archived-text"),
            age_ms: 6 * DAY + 6 * HOUR,
            from: Author::Home,
            text: plain("Réservation du restaurant confirmée pour vendredi, 4 personnes."),
            archived: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-archived-unread"),
            age_ms: 4 * DAY + 3 * HOUR,
            text: plain("Le relevé du compteur est à faire avant la fin du mois."),
            read: false,
            archived: true,
            ..BASE
        },
        DemoEntry {
            key: st("week-unread-link"),
            age_ms: 3 * DAY + 2 * HOUR,
            title: s("Horaires d'hiver de la déchetterie"),
            url: s("https://example.org/pratique/dechetterie"),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-unread-text"),
            age_ms: 3 * DAY + 5 * HOUR,
            from: Author::Home,
            text: plain("Tu peux passer prendre le colis chez la voisine ?"),
            read: false,
            ..BASE
        },
        DemoEntry {
            key: st("week-sent-text"),
            age_ms: 4 * DAY + HOUR,
            from: Author::This,
            text: plain("Arrivé, je t'appelle ce soir."),
            ..BASE
        },
        DemoEntry {
            key: st("week-sent-link"),
            age_ms: 6 * DAY + 9 * HOUR,
            from: Author::This,
            title: s("Location de kayaks"),
            url: s("https://www.example.com/loisirs/kayak"),
            ..BASE
        },
    ]
}

// ---------------------------------------------------------------------------------------------
// Three years of history
// ---------------------------------------------------------------------------------------------

/// The history runs from the end of the validation week to three years ago.
const HISTORY_FIRST_DAY: i64 = 8;
const HISTORY_LAST_DAY: i64 = 3 * 365;
/// In the tests, one day out of this many only.
const TEST_DAY_STEP: usize = 365;

const LINKS: [(&str, &str); 40] = [
    ("Recette du gratin dauphinois", "recettes/gratin-dauphinois"),
    (
        "Horaires de la piscine municipale",
        "sport/piscine-horaires",
    ),
    ("Tuto : changer une chambre à air", "velo/chambre-a-air"),
    ("Comparatif des aspirateurs sans fil", "tests/aspirateurs"),
    ("Itinéraire vélo le long du canal", "balades/canal"),
    (
        "Guide des plantes d'intérieur faciles",
        "jardin/plantes-interieur",
    ),
    ("Bien ranger son garage", "maison/garage"),
    ("Documentation de l'API météo", "docs/api-meteo"),
    ("Conseils pour un CV clair", "emploi/cv"),
    ("Jeux de société à deux", "loisirs/jeux-a-deux"),
    ("Programme du festival d'été", "agenda/festival-ete"),
    (
        "Tuto : sauvegarder ses photos",
        "numerique/sauvegarde-photos",
    ),
    ("Recette des crêpes légères", "recettes/crepes"),
    ("Entretien d'une chaudière", "maison/chaudiere"),
    ("Calendrier des collectes de déchets", "pratique/collectes"),
    ("Étirements du matin", "sante/etirements"),
    ("Détartrer une bouilloire", "maison/detartrage"),
    ("Carte des sentiers de randonnée", "balades/sentiers"),
    (
        "Podcast : histoire des cartes postales",
        "podcasts/cartes-postales",
    ),
    ("Vidéo : nœuds de marin utiles", "videos/noeuds"),
    ("Fiche : budget mensuel simple", "finances/budget-simple"),
    ("Le compost en appartement", "jardin/compost"),
    ("Recette du pain maison", "recettes/pain"),
    ("Mode d'emploi du lave-vaisselle", "notices/lave-vaisselle"),
    ("Astuces pour économiser l'eau", "maison/eau"),
    ("L'histoire du métro", "culture/metro"),
    ("Tuto : configurer un routeur Wi-Fi", "numerique/routeur"),
    ("Liste de lecture pour l'été", "lecture/ete"),
    ("Plan d'entraînement 10 km", "sport/10-km"),
    ("Recette de la soupe de légumes", "recettes/soupe"),
    ("Notice de montage de l'étagère", "notices/etagere"),
    ("Idées de sorties en famille", "loisirs/sorties"),
    ("Guide des premiers secours", "sante/premiers-secours"),
    ("Apprendre à méditer", "sante/meditation"),
    ("Randonnée : le tour du lac", "balades/tour-du-lac"),
    ("Documentation du thermostat", "notices/thermostat"),
    ("Quiz de culture générale", "loisirs/quiz"),
    ("Tuto : réparer un robinet qui fuit", "maison/robinet"),
    ("Recette du taboulé", "recettes/taboule"),
    ("Fiche : vaccins du chat", "animaux/chat-vaccins"),
];

const DOMAINS: [&str; 5] = [
    "https://example.org",
    "https://www.example.com",
    "https://docs.example.org",
    "https://blog.example.com",
    "https://wiki.example.org",
];

const THINGS: [&str; 10] = [
    "le rendez-vous chez le dentiste",
    "de rapporter les livres à la médiathèque",
    "d'arroser les plantes",
    "de payer la cantine",
    "la réunion de copropriété",
    "d'appeler le garage",
    "de prendre les billets",
    "de racheter du café",
    "de recharger la carte de bus",
    "de signer le carnet",
];

const WHEN: [&str; 7] = [
    "demain matin",
    "lundi",
    "avant vendredi",
    "ce soir",
    "la semaine prochaine",
    "à 18 h",
    "samedi midi",
];

const GROCERIES: [&str; 12] = [
    "lait", "pain", "œufs", "pommes", "riz", "café", "farine", "tomates", "fromage", "lessive",
    "yaourts", "pâtes",
];

const STREETS: [&str; 6] = [
    "des Tilleuls",
    "des Peupliers",
    "des Érables",
    "des Châtaigniers",
    "des Ormes",
    "des Saules",
];

const TOWNS: [&str; 4] = [
    "Villeneuve-Exemple",
    "Bourg-Démo",
    "Saint-Fictif",
    "Montexemple",
];

const GIFTS: [&str; 5] = [
    "un livre de cuisine",
    "une plante",
    "un jeu de cartes",
    "une lampe de lecture",
    "un carnet",
];

const SERVICES: [&str; 8] = [
    "Wi-Fi du salon",
    "box internet",
    "alarme",
    "coffre",
    "boîte aux lettres",
    "médiathèque",
    "imprimante",
    "garage",
];

const LISTS: [&str; 5] = [
    "Courses",
    "Valise",
    "Bricolage",
    "Menu de la semaine",
    "Idées week-end",
];

const TOKEN_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789";

fn token(rng: &mut Rng, len: usize) -> String {
    (0..len)
        .map(|_| char::from(*rng.pick(TOKEN_ALPHABET)))
        .collect()
}

fn link(rng: &mut Rng) -> (String, String) {
    let (title, slug) = *rng.pick(&LINKS);
    let domain = rng.pick(&DOMAINS);
    let url = if rng.chance(30) {
        format!("{domain}/{slug}?id={}", rng.range(100, 99_999))
    } else {
        format!("{domain}/{slug}")
    };
    (title.to_owned(), url)
}

fn text_line(rng: &mut Rng) -> String {
    match rng.below(7) {
        0 => format!("Rappel : {} {}.", rng.pick(&THINGS), rng.pick(&WHEN)),
        1 => format!("N'oublie pas {}.", rng.pick(&THINGS)),
        2 => format!(
            "Liste : {}, {}, {}.",
            rng.pick(&GROCERIES),
            rng.pick(&GROCERIES),
            rng.pick(&GROCERIES)
        ),
        3 => format!(
            "Numéro de commande : {}-{} (livraison prévue {}).",
            token(rng, 4).to_uppercase(),
            rng.range(10_000, 99_999),
            rng.pick(&WHEN)
        ),
        4 => format!(
            "Adresse : {} rue {}, {}.",
            rng.range(1, 120),
            rng.pick(&STREETS),
            rng.pick(&TOWNS)
        ),
        5 => format!("Idée cadeau : {}.", rng.pick(&GIFTS)),
        _ => {
            let count = 1 + rng.below(3) as usize;
            demo_files::paragraph(rng, count)
        }
    }
}

/// One ordinary item of the history, written `age_ms` ago, number `index`.
fn history_entry(rng: &mut Rng, index: usize, age_ms: i64) -> DemoEntry {
    let from = match rng.below(100) {
        0..=44 => Author::Office,
        45..=79 => Author::Home,
        _ => Author::This,
    };
    let mut e = DemoEntry {
        key: Cow::Owned(format!("history-{index:04}")),
        age_ms,
        from,
        ..BASE
    };
    match rng.below(100) {
        0..=39 => {
            let (title, url) = link(rng);
            if rng.chance(85) {
                e.title = owned(title);
            }
            e.url = owned(url);
        }
        40..=63 => {
            e.text = Some(DemoText {
                plain: Cow::Owned(text_line(rng)),
                markdown: None,
                html: None,
                source_format: SourceFormat::Plain,
            });
        }
        64..=71 => {
            let list = rng.pick(&LISTS);
            let items: Vec<&str> = (0..3).map(|_| *rng.pick(&GROCERIES)).collect();
            let when = rng.pick(&WHEN);
            e.text = markdown(
                format!("{list}\n{}\nÀ faire {when}.", items.join("\n")),
                format!(
                    "# {list}\n\n- {}\n\n**À faire** {when}.",
                    items.join("\n- ")
                ),
            );
        }
        72..=81 => {
            let service = rng.pick(&SERVICES);
            e.title = owned(format!("Accès {service} (démo)"));
            e.text = Some(DemoText {
                plain: Cow::Owned(format!("{}-demo", token(rng, 12))),
                markdown: None,
                html: None,
                source_format: SourceFormat::Plain,
            });
            e.secret = true;
            // A "read once" secret sent from here stays in the feed (it is never read here).
            e.after_first_read = from == Author::This && rng.chance(25);
        }
        82..=85 => {
            let list = rng.pick(&LISTS);
            let line = text_line(rng);
            e.text = Some(DemoText {
                plain: Cow::Owned(format!("{list}\n{line}")),
                markdown: None,
                html: owned(format!("<p><b>{list}</b></p><p>{line}</p>")),
                source_format: SourceFormat::Html,
            });
        }
        86..=93 => {
            let n = rng.range(100, 9_999);
            let (title, file) = match rng.below(5) {
                0 => (
                    format!("Facture n° {n}"),
                    DemoFile::Pdf {
                        name: Cow::Owned(format!("facture-{n}.pdf")),
                        title: Cow::Owned(format!("Facture {n} (demo Tomi)")),
                    },
                ),
                1 => (
                    "Dépenses du mois".to_owned(),
                    generated(
                        format!("depenses-{n}.csv"),
                        Generated::Csv {
                            rows: 40,
                            seed: 1_000 + index as u64,
                        },
                    ),
                ),
                2 => (
                    "Réglages exportés".to_owned(),
                    generated(
                        format!("reglages-{n}.json"),
                        Generated::Json {
                            records: 4,
                            seed: 1_000 + index as u64,
                        },
                    ),
                ),
                3 => (
                    "Script".to_owned(),
                    generated(
                        format!("script_{n}.py"),
                        Generated::Code {
                            lang: CodeLang::Python,
                            functions: 2,
                        },
                    ),
                ),
                _ => {
                    let count = 2 + rng.below(4) as usize;
                    (
                        "Notes".to_owned(),
                        DemoFile::Text {
                            name: Cow::Owned(format!("notes-{n}.txt")),
                            mime: st("text/plain"),
                            content: Cow::Owned(demo_files::paragraph(rng, count)),
                        },
                    )
                }
            };
            e.title = owned(title);
            e.files = vec![file];
        }
        94..=96 => {
            let (title, url) = link(rng);
            e.title = owned(title);
            e.url = owned(url);
            e.text = Some(DemoText {
                plain: Cow::Owned(text_line(rng)),
                markdown: None,
                html: None,
                source_format: SourceFormat::Plain,
            });
        }
        _ => {
            e.title = s("Photo");
            e.files = vec![generated(
                format!("photo-{index:04}.bmp"),
                photo(240, 180, 2_000 + index as u64),
            )];
        }
    }
    if rng.below(1000) < 3 {
        // Now and then, an item of a future format.
        e.title = None;
        e.text = None;
        e.url = None;
        e.files = Vec::new();
        e.secret = false;
        e.after_first_read = false;
        e.unreadable = true;
    }
    let days = age_ms / DAY;
    if from != Author::This {
        let unread = if days < 30 { 6 } else { 1 };
        e.read = rng.below(200) >= unread;
    }
    let archived = match days {
        0..=90 => 5,
        91..=365 => 30,
        _ => 55,
    };
    e.archived = rng.chance(archived);
    e
}

/// About three years of history before the validation week: around three items a day, with
/// quiet days. A shorter history in the tests (one day out of [`TEST_DAY_STEP`]).
pub(crate) fn history(scale: Scale) -> Vec<DemoEntry> {
    let mut rng = Rng::new(0x7031_2023);
    let step = match scale {
        Scale::Full => 1,
        Scale::Test => TEST_DAY_STEP,
    };
    let mut out = Vec::new();
    for day in (HISTORY_FIRST_DAY..=HISTORY_LAST_DAY).step_by(step) {
        let count = if rng.chance(20) {
            0
        } else {
            1 + rng.below(6) as usize
        };
        // Between 7 h and 23 h of that day, oldest first.
        let mut minutes: Vec<i64> = (0..count).map(|_| rng.range(7 * 60, 23 * 60)).collect();
        minutes.sort_unstable();
        for m in minutes {
            let age = day * DAY - m * MINUTE;
            let index = out.len();
            out.push(history_entry(&mut rng, index, age));
        }
    }
    // A few pins over the years, on items that are not archived.
    for days in [15, 120, 410, 800] {
        if let Some(e) = out
            .iter_mut()
            .filter(|e| e.age_ms >= days * DAY && !e.archived && !e.unreadable)
            .min_by_key(|e| e.age_ms)
        {
            e.pinned = true;
        }
    }
    out
}
