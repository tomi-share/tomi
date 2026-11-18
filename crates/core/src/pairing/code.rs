//! The pairing code, its link and its reading (`spec/pairing-v1.md` §4).
//!
//! ```text
//! code = channel_id "-" word1 "-" word2        e.g. 4821-crayon-renard
//! link = "https://" relay_authority "/l/pair#" code
//! ```
//!
//! The two words are the only secret of the code (22 bits). A `PairingCode` is therefore never
//! printed by `Debug` and has no `Display`: the explicit [`PairingCode::to_code_string`] and
//! [`PairingCode::link`] are the only ways to show it, to the user, on the pairing screen.

use zeroize::{Zeroize, Zeroizing};

use super::entropy::Entropy;
use super::wordlist;
use crate::error::{CoreError, Result};

/// Number of words in a code.
pub const CODE_WORDS: usize = 2;
/// Path of the pairing link on the relay host.
const LINK_PATH: &str = "/l/pair";
/// Host of the `tomi-share://` form of the link.
const DEEP_LINK_HOST: &str = "pair";

/// A pairing code: the (public) channel identifier and the two secret words, as ranks.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    channel_id: String,
    ranks: [u16; CODE_WORDS],
}

impl PairingCode {
    /// Draws a fresh code for the channel the relay just opened (A3): two ranks, independent and
    /// uniform in `0..2048` (11 bits of CSPRNG each, no rejection).
    pub fn generate(channel_id: &str) -> Result<Self> {
        Self::generate_with(channel_id, &mut Entropy::os())
    }

    pub(super) fn generate_with(channel_id: &str, entropy: &mut Entropy) -> Result<Self> {
        check_channel_id(channel_id)?;
        let mut ranks = [0u16; CODE_WORDS];
        for r in &mut ranks {
            *r = u16::from_be_bytes(entropy.array::<2>()?) & 0x07ff;
        }
        Ok(Self {
            channel_id: channel_id.to_owned(),
            ranks,
        })
    }

    /// A code from known parts (reading and tests).
    pub(super) fn from_parts(channel_id: &str, ranks: [u16; CODE_WORDS]) -> Result<Self> {
        check_channel_id(channel_id)?;
        if ranks
            .iter()
            .any(|&r| usize::from(r) >= wordlist::WORD_COUNT)
        {
            return Err(CoreError::input("pairing code: word rank out of range"));
        }
        Ok(Self {
            channel_id: channel_id.to_owned(),
            ranks,
        })
    }

    /// The channel identifier (not secret).
    pub fn channel_id(&self) -> &str {
        &self.channel_id
    }

    /// The ranks of the two words (secret).
    pub fn ranks(&self) -> [u16; CODE_WORDS] {
        self.ranks
    }

    /// `PRS = u16be(i1) || u16be(i2)`, the CPace password (secret).
    pub fn prs(&self) -> Zeroizing<[u8; 4]> {
        let mut prs = Zeroizing::new([0u8; 4]);
        prs[..2].copy_from_slice(&self.ranks[0].to_be_bytes());
        prs[2..].copy_from_slice(&self.ranks[1].to_be_bytes());
        prs
    }

    /// The code to show on the pairing screen: `4821-crayon-renard` (secret).
    pub fn to_code_string(&self) -> String {
        let word = |r: u16| wordlist::word(r).unwrap_or_default();
        format!(
            "{}-{}-{}",
            self.channel_id,
            word(self.ranks[0]),
            word(self.ranks[1])
        )
    }

    /// The link carried by the QR code: `https://<relay_authority>/l/pair#<code>` (secret).
    /// The code is in the fragment, which a browser never sends to a server.
    pub fn link(&self, relay_authority: &str) -> String {
        format!(
            "https://{relay_authority}{LINK_PATH}#{}",
            self.to_code_string()
        )
    }

    /// The fallback form `tomi-share://pair#<code>` (secret). A QR code produced by Tomi always uses
    /// [`link`](Self::link).
    pub fn deep_link(&self) -> String {
        format!("tomi-share://{DEEP_LINK_HOST}#{}", self.to_code_string())
    }

    /// Reads a typed code (§4.4, rules 2 to 5): NFKD fold, three tokens, a channel number and
    /// two words (full, or a prefix of at least 4 letters that identifies a single word).
    pub fn parse(input: &str) -> Result<Self> {
        let invalid = || CoreError::input("invalid pairing code");
        // `None` here: the input held a letter or digit we do not fold exactly (a ligature, an
        // exotic script...). Refuse locally rather than risk reading a different valid word.
        let tokens = fold_tokens(input).ok_or_else(invalid)?;
        let [channel, first, second] = tokens.as_slice() else {
            return Err(invalid());
        };
        check_channel_id(channel).map_err(|_| invalid())?;
        let a = word_rank(first).ok_or_else(invalid)?;
        let b = word_rank(second).ok_or_else(invalid)?;
        Self::from_parts(channel, [a, b])
    }

    /// Reads a link against one expected relay (§4.4, rule 1): `https://<relay_authority>/l/pair#<code>`
    /// where the authority is `relay_authority`, or `tomi-share://pair#<code>`. The engine reads links of any
    /// valid relay with [`crate::parse_pair_link`] instead and shows that relay to the user; this one
    /// serves a reader whose relay is fixed (and the frozen vectors). Nothing but the
    /// fragment is read; a link never starts the pairing by itself.
    pub fn from_link(input: &str, relay_authority: &str) -> Result<Self> {
        let refuse = || CoreError::input("not a Tomi pairing link");
        let s = input.trim();
        let (scheme, rest) = s.split_once("://").ok_or_else(refuse)?;
        let (location, fragment) = rest.split_once('#').ok_or_else(refuse)?;
        let (authority, path_and_query) = match location.find(['/', '?']) {
            Some(i) => location.split_at(i),
            None => (location, ""),
        };
        // The query string, if any, is ignored: no parameter is read.
        let path = path_and_query.split('?').next().unwrap_or_default();
        let ok = if scheme.eq_ignore_ascii_case("https") {
            authority.eq_ignore_ascii_case(relay_authority) && path == LINK_PATH
        } else if scheme.eq_ignore_ascii_case("tomi-share") {
            authority.eq_ignore_ascii_case(DEEP_LINK_HOST) && (path.is_empty() || path == "/")
        } else {
            false
        };
        if !ok {
            return Err(refuse());
        }
        Self::parse(fragment).map_err(|_| refuse())
    }

    /// Reads whatever the user gave: a link (scan, App Link, `tomi-share://`) or a typed code.
    pub fn read(input: &str, relay_authority: &str) -> Result<Self> {
        if input.contains("://") {
            Self::from_link(input, relay_authority)
        } else {
            Self::parse(input)
        }
    }
}

impl Drop for PairingCode {
    fn drop(&mut self) {
        self.ranks.zeroize();
    }
}

impl std::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingCode")
            .field("channel_id", &self.channel_id)
            .field("words", &"<redacted>")
            .finish()
    }
}

/// `channel_id`: 1 to 4 ASCII digits, no leading zero (value 1 to 9999), `api-v1.md` §4.6.
pub(super) fn check_channel_id(s: &str) -> Result<()> {
    let ok =
        (1..=4).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()) && !s.starts_with('0');
    if ok {
        Ok(())
    } else {
        Err(CoreError::input("invalid channel id"))
    }
}

/// The relay authority the device retained: lowercase ASCII host, optional `:port`.
/// It enters the CPace channel identifier, so both devices must spell it the same way.
pub(crate) fn check_relay_authority(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 255
        && s.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b'.' | b'-' | b':' | b'[' | b']')
        });
    if ok {
        Ok(())
    } else {
        Err(CoreError::input(
            "relay authority must be lowercase ASCII (host[:port])",
        ))
    }
}

/// A word token (letters `a`-`z` only): the word itself, or the unique word it prefixes (§4.4
/// rule 4).
fn word_rank(token: &str) -> Option<u16> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_lowercase()) {
        return None;
    }
    wordlist::rank_of(token).or_else(|| wordlist::rank_of_unique_prefix(token))
}

// ---------------------------------------------------------------------------
// Folding of typed text (§4.4 rule 2)
// ---------------------------------------------------------------------------

/// Cuts typed text into tokens of ASCII lowercase letters and digits.
///
/// The rule of the spec is: NFKD, remove the combining marks (Mn), lowercase, then split on every
/// run of characters that are neither ASCII letters nor ASCII digits. `tomi-core` has no Unicode
/// normalization table (no new crate in the tree for a typed code), so NFKD is applied through
/// small tables that are **exact** for:
///
/// * ASCII;
/// * the Latin-1 Supplement and Latin Extended-A letters (`U+00A0`-`U+017F`), checked against
///   an ICU NFKD table by the tests: every accented French letter, precomposed or not;
/// * the Combining Diacritical Marks block (`U+0300`-`U+036F`, all Mn): dropped, which is what
///   NFKD followed by the removal of Mn does to a decomposed accent;
/// * the full-width digits and letters (`U+FF10`-`U+FF5A`), which NFKD maps to ASCII.
///
/// A non-alphanumeric character (punctuation, symbol, blank) is a separator. A character that
/// is a letter or a digit (Unicode `is_alphanumeric`) but lies outside the handled ranges makes
/// the whole code **refused** (`None`): folding it as a separator could otherwise turn it into a
/// *different* valid word (e.g. the ligature `ﬂ` dropped from `ﬂeche` leaves `eche`, a unique
/// prefix of `echelle`). Refusing is a local rejection: no `claim`, no essay consumed (§4.4
/// rule 5). This covers ligatures, the Kelvin sign, Latin Extended-B and additional, circled or
/// mathematical letters, Roman numerals, superscripts, and letters of other scripts.
fn fold_tokens(input: &str) -> Option<Vec<String>> {
    let mut folded = String::with_capacity(input.len());
    for c in input.chars() {
        if !fold_into(c, &mut folded) {
            return None;
        }
    }
    Some(
        folded
            .split(' ')
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

/// Appends the folded form of `c` (ASCII lowercase letters and digits, `' '` for a separator).
/// Returns `false` when `c` is an alphanumeric outside the handled ranges: the caller must then
/// refuse the whole code rather than risk reading another word.
fn fold_into(c: char, out: &mut String) -> bool {
    if c.is_ascii_alphanumeric() {
        out.push(c.to_ascii_lowercase());
    } else if ('\u{0300}'..='\u{036f}').contains(&c) {
        // Mn: removed, the letter it follows stays in the same token.
    } else if let Some(folded) = fold_latin(c) {
        out.push_str(folded);
    } else if let Some(ascii) = fold_fullwidth(c) {
        out.push(ascii.to_ascii_lowercase());
    } else if c.is_alphanumeric() {
        // A letter or digit we do not fold exactly: refuse rather than mis-read it.
        return false;
    } else {
        out.push(' ');
    }
    true
}

/// Full-width digits and letters (`U+FF10`-`U+FF19`, `U+FF21`-`U+FF3A`, `U+FF41`-`U+FF5A`).
fn fold_fullwidth(c: char) -> Option<char> {
    match c {
        '\u{ff10}'..='\u{ff19}' | '\u{ff21}'..='\u{ff3a}' | '\u{ff41}'..='\u{ff5a}' => {
            char::from_u32(u32::from(c) - 0xfee0)
        }
        _ => None,
    }
}

/// NFKD, Mn removed, lowercased, for `U+00A0`-`U+017F`: the ASCII letters and digits left, with
/// a space for a non-ASCII or punctuation remainder (`ŀ` is `l` and a middle dot, `ŉ` an
/// apostrophe and `n`). `None` for every character whose fold has no ASCII letter or digit.
fn fold_latin(c: char) -> Option<&'static str> {
    Some(match u32::from(c) {
        0xaa => "a",
        0xb2 => "2",
        0xb3 => "3",
        0xb9 => "1",
        0xba => "o",
        0xbc => "1 4",
        0xbd => "1 2",
        0xbe => "3 4",
        0xc0..=0xc5 | 0xe0..=0xe5 | 0x100..=0x105 => "a",
        0xc7 | 0xe7 | 0x106..=0x10d => "c",
        0x10e | 0x10f => "d",
        0xc8..=0xcb | 0xe8..=0xeb | 0x112..=0x11b => "e",
        0x11c..=0x123 => "g",
        0x124 | 0x125 => "h",
        0xcc..=0xcf | 0xec..=0xef | 0x128..=0x130 => "i",
        0x132 | 0x133 => "ij",
        0x134 | 0x135 => "j",
        0x136 | 0x137 => "k",
        0x139..=0x13e => "l",
        0x13f | 0x140 => "l ",
        0xd1 | 0xf1 | 0x143..=0x148 => "n",
        0x149 => " n",
        0xd2..=0xd6 | 0xf2..=0xf6 | 0x14c..=0x151 => "o",
        0x154..=0x159 => "r",
        0x15a..=0x161 | 0x17f => "s",
        0x162..=0x165 => "t",
        0xd9..=0xdc | 0xf9..=0xfc | 0x168..=0x173 => "u",
        0x174 | 0x175 => "w",
        0xdd | 0xfd | 0xff | 0x176..=0x178 => "y",
        0x179..=0x17e => "z",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTHORITY: &str = "relay.example.org";

    fn code(s: &str) -> PairingCode {
        PairingCode::parse(s).unwrap()
    }

    #[test]
    fn p04_generate_keeps_the_channel_and_draws_every_rank() {
        let mut seen = vec![false; wordlist::WORD_COUNT];
        for _ in 0..100_000 {
            let c = PairingCode::generate("4821").unwrap();
            assert_eq!(c.channel_id(), "4821");
            for r in c.ranks() {
                assert!(usize::from(r) < wordlist::WORD_COUNT);
                seen[usize::from(r)] = true;
            }
        }
        // 200 000 draws over 2048 ranks: a missing rank has probability about e^-97 each.
        assert!(seen.iter().all(|&s| s), "some rank never drawn");
    }

    #[test]
    fn generate_uses_eleven_bits_without_rejection() {
        let mut e = Entropy::scripted(vec![vec![0xff, 0xff], vec![0x01, 0xd1]]);
        let c = PairingCode::generate_with("7", &mut e).unwrap();
        assert_eq!(c.ranks(), [2047, 465]);
        assert_eq!(c.to_code_string(), "7-zoologie-crayon");
        assert!(PairingCode::generate("0").is_err());
        assert!(PairingCode::generate("12345").is_err());
        assert!(PairingCode::generate("").is_err());
        assert!(PairingCode::generate("04").is_err());
        assert!(PairingCode::generate("x1").is_err());
    }

    #[test]
    fn display_prs_and_links() {
        let c = code("4821-crayon-renard");
        assert_eq!(c.ranks(), [465, 1633]);
        assert_eq!(c.to_code_string(), "4821-crayon-renard");
        assert_eq!(hex::encode(*c.prs()), "01d10661");
        assert_eq!(
            c.link(AUTHORITY),
            "https://relay.example.org/l/pair#4821-crayon-renard"
        );
        assert_eq!(c.deep_link(), "tomi-share://pair#4821-crayon-renard");
    }

    #[test]
    fn p05_reading_accepts_the_forms_of_the_spec() {
        let want = code("4821-crayon-renard");
        for ok in [
            "4821-crayon-renard",
            "4821 Crayon RENARD",
            "  4821-crayon-renard\n",
            "4821-crayo-rena",
            "4821_crayon.renard",
            "4821-crâyon-renard",
            // The same accent decomposed (a + U+0302), and a precomposed one.
            "4821-cra\u{302}yon-renard",
            "4821-cr\u{e2}yon-renard",
            "4821 - crayon / renard",
            "４８２１-ｃｒａｙｏｎ-ＲＥＮＡＲＤ",
            "4821\u{a0}crayon\u{2014}renard",
        ] {
            assert_eq!(PairingCode::parse(ok).unwrap(), want, "{ok:?}");
        }
        // A separator at either end is not a token (a code copied from a sentence).
        assert_eq!(code("(4821-crayon-renard)."), want);
        assert_eq!(code("4821-école-élève").ranks(), [616, 642]);
        assert_eq!(
            code("4821-e\u{301}cole-e\u{301}le\u{300}ve").ranks(),
            [616, 642]
        );
        assert_eq!(code("9999-abaisser-zoologie").ranks(), [0, 2047]);
        assert_eq!(code("1-abai-zool").to_code_string(), "1-abaisser-zoologie");
        // The two words may be equal.
        assert_eq!(code("12-crayon-crayon").ranks(), [465, 465]);
    }

    #[test]
    fn p05_reading_refuses_everything_else() {
        for bad in [
            "",
            "0-crayon-renard",
            "04821-crayon-renard",
            "0482-crayon-renard",
            "10000-crayon-renard",
            "4821-cra-renard",
            "4821-crayon",
            "4821",
            "crayon-renard",
            "4821-crayon-renard-tomate",
            // Unknown words: too long for the list, a typo past the 4-letter prefix, `lune`.
            "4821-xylophone-renard",
            "4821-crayonn-renard",
            "4821-lune-renard",
            "4821-zzzz-renard",
            "4821-cr4yon-renard",
            "4821crayon-renard",
            "abc-crayon-renard",
            // Digits of other scripts do not fold to ASCII digits.
            "\u{664}\u{668}\u{662}\u{661}-crayon-renard",
            "\u{967}-crayon-renard",
        ] {
            let e = PairingCode::parse(bad).err();
            assert_eq!(
                e.map(|e| e.code()),
                Some("invalid_input"),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn p06_links() {
        let want = code("4821-crayon-renard");
        for ok in [
            "https://relay.example.org/l/pair#4821-crayon-renard",
            "HTTPS://RELAY.EXAMPLE.ORG/l/pair#4821-crayon-renard",
            "  https://relay.example.org/l/pair#4821-crayon-renard\n",
            "tomi-share://pair#4821-crayon-renard",
            "tomi-share://pair/#4821-crayon-renard",
            "TOMI-SHARE://PAIR#4821-Crayon-Renard",
            // Only the fragment is read.
            "https://relay.example.org/l/pair?utm=x#4821-crayon-renard",
        ] {
            assert_eq!(PairingCode::from_link(ok, AUTHORITY).unwrap(), want, "{ok}");
            assert_eq!(PairingCode::read(ok, AUTHORITY).unwrap(), want, "{ok}");
        }
        assert_eq!(
            PairingCode::read("4821 crayon renard", AUTHORITY).unwrap(),
            want
        );
        for bad in [
            "https://evil.example/l/pair#4821-crayon-renard",
            "https://relay.example.org.evil.example/l/pair#4821-crayon-renard",
            "https://user@relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org:8443/l/pair#4821-crayon-renard",
            "http://relay.example.org/l/pair#4821-crayon-renard",
            "ftp://relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org/l/other#4821-crayon-renard",
            "https://relay.example.org/l/pair/#4821-crayon-renard",
            "https://relay.example.org/#4821-crayon-renard",
            "https://relay.example.org/l/pair?code=4821-crayon-renard",
            "https://relay.example.org/l/pair?4821-crayon-renard",
            "https://relay.example.org/l/pair",
            "https://relay.example.org/l/pair#",
            "https://relay.example.org/l/pair#4821-crayon",
            "tomi-share://other#4821-crayon-renard",
            "tomi-share://pair/x#4821-crayon-renard",
            "tomi:pair#4821-crayon-renard",
            "mailto://pair#4821-crayon-renard",
        ] {
            let e = PairingCode::from_link(bad, AUTHORITY).unwrap_err();
            assert_eq!(e.code(), "invalid_input", "{bad}");
            assert_eq!(
                PairingCode::read(bad, AUTHORITY).unwrap_err().code(),
                "invalid_input"
            );
        }
        // A relay with a port: the link must carry exactly that authority.
        let c = code("7-crayon-renard");
        let link = c.link("tomi.example:8443");
        assert_eq!(link, "https://tomi.example:8443/l/pair#7-crayon-renard");
        assert_eq!(
            PairingCode::from_link(&link, "tomi.example:8443").unwrap(),
            c
        );
        assert!(PairingCode::from_link(&link, "tomi.example").is_err());
    }

    #[test]
    fn debug_hides_the_words() {
        let c = code("4821-crayon-renard");
        let dbg = format!("{c:?}");
        assert!(dbg.contains("4821"));
        assert!(!dbg.contains("crayon") && !dbg.contains("renard") && !dbg.contains("465"));
        assert!(!dbg.contains("1633") && !dbg.contains("01d1"));
    }

    #[test]
    fn relay_authority_and_channel_checks() {
        for ok in ["relay.example.org", "localhost:8080", "127.0.0.1:3000", "a"] {
            assert!(check_relay_authority(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Relay.example.org",
            "relay.example.org/",
            "a b",
            "x@y",
            "é.eu",
            "a#b",
        ] {
            assert!(check_relay_authority(bad).is_err(), "{bad}");
        }
        assert!(check_channel_id("9999").is_ok());
        assert!(check_channel_id("1").is_ok());
        assert!(check_channel_id("10").is_ok());
    }

    /// `U+00A0`-`U+017F` against an NFKD table produced with ICU (Node 22, Unicode 16):
    /// `normalize('NFKD')`, Mn removed, lowercased; `_` stands for a separator. Only the code
    /// points whose fold keeps an ASCII letter or digit are listed.
    const ICU_LATIN: &str = concat!(
        "aa:a b2:2 b3:3 b9:1 ba:o bc:1_4 bd:1_2 be:3_4 c0:a c1:a c2:a c3:a c4:a c5:a c7:c c8:e ",
        "c9:e ca:e cb:e cc:i cd:i ce:i cf:i d1:n d2:o d3:o d4:o d5:o d6:o d9:u da:u db:u dc:u ",
        "dd:y e0:a e1:a e2:a e3:a e4:a e5:a e7:c e8:e e9:e ea:e eb:e ec:i ed:i ee:i ef:i f1:n ",
        "f2:o f3:o f4:o f5:o f6:o f9:u fa:u fb:u fc:u fd:y ff:y 100:a 101:a 102:a 103:a 104:a ",
        "105:a 106:c 107:c 108:c 109:c 10a:c 10b:c 10c:c 10d:c 10e:d 10f:d 112:e 113:e 114:e ",
        "115:e 116:e 117:e 118:e 119:e 11a:e 11b:e 11c:g 11d:g 11e:g 11f:g 120:g 121:g 122:g ",
        "123:g 124:h 125:h 128:i 129:i 12a:i 12b:i 12c:i 12d:i 12e:i 12f:i 130:i 132:ij 133:ij ",
        "134:j 135:j 136:k 137:k 139:l 13a:l 13b:l 13c:l 13d:l 13e:l 13f:l_ 140:l_ 143:n 144:n ",
        "145:n 146:n 147:n 148:n 149:_n 14c:o 14d:o 14e:o 14f:o 150:o 151:o 154:r 155:r 156:r ",
        "157:r 158:r 159:r 15a:s 15b:s 15c:s 15d:s 15e:s 15f:s 160:s 161:s 162:t 163:t 164:t ",
        "165:t 168:u 169:u 16a:u 16b:u 16c:u 16d:u 16e:u 16f:u 170:u 171:u 172:u 173:u 174:w ",
        "175:w 176:y 177:y 178:y 179:z 17a:z 17b:z 17c:z 17d:z 17e:z 17f:s ",
    );

    #[test]
    fn latin_fold_table_matches_icu_nfkd() {
        let expected: std::collections::BTreeMap<u32, String> = ICU_LATIN
            .split_whitespace()
            .map(|e| {
                let (cp, s) = e.split_once(':').unwrap();
                (u32::from_str_radix(cp, 16).unwrap(), s.replace('_', " "))
            })
            .collect();
        assert_eq!(expected.len(), 175);
        for cp in 0xa0..=0x17fu32 {
            let c = char::from_u32(cp).unwrap();
            assert_eq!(
                fold_latin(c),
                expected.get(&cp).map(String::as_str),
                "U+{cp:04X}"
            );
        }
    }

    #[test]
    fn fold_tokens_shapes() {
        assert_eq!(
            fold_tokens("  Ça va-bien!  ").unwrap(),
            ["ca", "va", "bien"]
        );
        assert_eq!(fold_tokens("a\u{301}\u{300}b").unwrap(), ["ab"]);
        assert_eq!(fold_tokens("ĳ ŀ ŉ").unwrap(), ["ij", "l", "n"]);
        assert_eq!(fold_tokens("l·a").unwrap(), ["l", "a"]);
        assert_eq!(fold_tokens("ＡＢ１２").unwrap(), ["ab12"]);
        assert!(fold_tokens("---").unwrap().is_empty());
    }

    #[test]
    fn fold_tokens_refuses_unhandled_alphanumerics() {
        // A letter or digit we do not fold exactly refuses the whole code, rather than
        // dropping it as a separator and risking a different valid word (R2).
        assert!(fold_tokens("œ").is_none(), "OE ligature");
        assert!(
            fold_tokens("4821-crayon-ﬂeche").is_none(),
            "fl ligature U+FB02"
        );
        assert!(fold_tokens("4821-ǎchat-renard").is_none(), "a-caron U+01CE");
        assert!(fold_tokens("Ⅻ").is_none(), "Roman numeral twelve (U+2160+)");
        assert!(fold_tokens("Ⓐ").is_none(), "circled A (U+24B6)");
        // U+00B2 (²) is inside the handled Latin-1 range and folds to "2": not refused.
        assert_eq!(fold_tokens("x²").unwrap(), ["x2"]);
        // A non-alphanumeric symbol is still just a separator.
        assert_eq!(fold_tokens("4821 € crayon").unwrap(), ["4821", "crayon"]);
    }

    #[test]
    fn parse_refuses_a_ligature_that_would_prefix_another_word() {
        // `ﬂeche` without the fold is `eche`, a unique 4-letter prefix of `echelle`: must refuse.
        assert!(PairingCode::parse("4821-crayon-ﬂeche").is_err());
    }
}
