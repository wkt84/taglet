//! Decoding of Japanese ISO 2022 text (Specific Character Set, PS3.5 6.1.2.5 / Annex H).
//!
//! dicom-rs 0.7 does not understand ISO 2022 IR 13 / 87 / 159 and only looks at the first
//! value of (0008,0005). Datasets using those character sets are therefore decoded by
//! dicom-rs with its default codec, which is ISO-8859-1. Because ISO-8859-1 maps every
//! byte to exactly one code point, the original bytes can be recovered losslessly from
//! such strings and decoded here with proper code extension handling.
//!
//! `Iso2022Charset::from_specific_character_set` only returns a charset when that
//! assumption holds (see `dicom_rs_reads_as_latin1`), so upgrading dicom-rs to a version
//! that decodes these character sets itself makes this module step aside instead of
//! decoding twice. A unit test guards the assumption.

use dicom_core::VR;
use dicom_encoding::text::{SpecificCharacterSet, TextCodec};
use encoding_rs::EUC_JP;

const ESC: u8 = 0x1B;

/// Graphic character set designated to G0 (bytes 0x21-0x7E).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum G0 {
    /// ISO 646 / ASCII (ESC ( B)
    Ascii,
    /// JIS X 0201 Romaji (ESC ( J). Displayed as ASCII so that `\` stays a visible
    /// value delimiter instead of turning into a yen sign.
    JisRoman,
    /// JIS X 0208 (ESC $ B, ESC $ @)
    Jis0208,
    /// JIS X 0212 (ESC $ ( D)
    Jis0212,
}

/// Graphic character set designated to G1 (bytes 0xA0-0xFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum G1 {
    None,
    /// JIS X 0201 Katakana (ESC ) I)
    Katakana,
    /// ISO 8859-1 right-hand part (ESC - A)
    Latin1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iso2022Charset {
    initial_g0: G0,
    initial_g1: G1,
}

impl Iso2022Charset {
    /// Interpret the value of Specific Character Set (0008,0005).
    ///
    /// Returns `Some` only when the value names a Japanese character set that dicom-rs
    /// cannot decode itself and whose text dicom-rs therefore read as ISO-8859-1.
    pub fn from_specific_character_set(value: &str) -> Option<Self> {
        let terms = value
            .split('\\')
            .map(|term| term.trim_matches(|c: char| c == ' ' || c == '\0'))
            .collect::<Vec<_>>();
        let has_japanese = terms.iter().any(|term| is_japanese_term(term));
        if !has_japanese || !dicom_rs_reads_as_latin1(terms[0]) {
            return None;
        }

        let (initial_g0, initial_g1) = match normalize_term(terms[0]).as_str() {
            "ISO_IR 13" | "ISO 2022 IR 13" => (G0::JisRoman, G1::Katakana),
            "ISO_IR 100" | "ISO 2022 IR 100" => (G0::Ascii, G1::Latin1),
            _ => (G0::Ascii, G1::None),
        };
        Some(Self {
            initial_g0,
            initial_g1,
        })
    }

    /// Decode a value that dicom-rs has already read as ISO-8859-1.
    ///
    /// Returns `None` when the value needs no re-decoding (VR not affected by the
    /// character set, plain ASCII) or cannot be re-decoded safely; callers should then
    /// keep the original string.
    pub fn redecode(&self, vr: VR, value: &str) -> Option<String> {
        if !is_charset_sensitive(vr) {
            return None;
        }
        let bytes = latin1_bytes(value)?;
        if bytes.iter().all(|byte| *byte != ESC && *byte < 0x80) {
            return None;
        }
        self.decode(&bytes, vr == VR::PN)
    }

    fn decode(&self, bytes: &[u8], person_name: bool) -> Option<String> {
        let mut out = String::with_capacity(bytes.len());
        let mut g0 = self.initial_g0;
        let mut g1 = self.initial_g1;
        let mut index = 0;

        while index < bytes.len() {
            let byte = bytes[index];

            if byte == ESC {
                let (consumed, designation) = parse_escape(&bytes[index + 1..])?;
                match designation {
                    Designation::G0(set) => g0 = set,
                    Designation::G1(set) => g1 = set,
                }
                index += 1 + consumed;
                continue;
            }

            if byte >= 0x80 {
                match g1 {
                    G1::Katakana if (0xA1..=0xDF).contains(&byte) => {
                        out.push(char::from_u32(0xFF61 + u32::from(byte - 0xA1))?);
                    }
                    G1::Katakana => out.push(char::REPLACEMENT_CHARACTER),
                    G1::Latin1 | G1::None => out.push(char::from(byte)),
                }
                index += 1;
                continue;
            }

            let is_graphic = (0x21..=0x7E).contains(&byte);
            match g0 {
                G0::Jis0208 | G0::Jis0212 if is_graphic => {
                    let Some(&second) = bytes.get(index + 1) else {
                        out.push(char::REPLACEMENT_CHARACTER);
                        index += 1;
                        continue;
                    };
                    if !(0x21..=0x7E).contains(&second) {
                        out.push(char::REPLACEMENT_CHARACTER);
                        index += 1;
                        continue;
                    }
                    let euc = if g0 == G0::Jis0208 {
                        vec![byte | 0x80, second | 0x80]
                    } else {
                        vec![0x8F, byte | 0x80, second | 0x80]
                    };
                    let (text, _) = EUC_JP.decode_without_bom_handling(&euc);
                    out.push_str(&text);
                    index += 2;
                }
                _ => {
                    out.push(char::from(byte));
                    index += 1;
                    // Delimiters switch back to the initial character sets (PS3.5 6.1.2.5.3).
                    // In multi-byte mode they are only recognised outside of a byte pair,
                    // which the branch above guarantees.
                    let resets = matches!(byte, b'\\' | b'\r' | b'\n' | 0x0C | b'\t')
                        || (person_name && matches!(byte, b'^' | b'='));
                    if resets {
                        g0 = self.initial_g0;
                        g1 = self.initial_g1;
                    }
                }
            }
        }

        Some(out)
    }
}

enum Designation {
    G0(G0),
    G1(G1),
}

/// Parse the bytes following ESC. Returns the number of bytes consumed after ESC.
fn parse_escape(rest: &[u8]) -> Option<(usize, Designation)> {
    const SEQUENCES: &[(&[u8], Designation)] = &[
        (b"(B", Designation::G0(G0::Ascii)),
        (b"(J", Designation::G0(G0::JisRoman)),
        (b"$B", Designation::G0(G0::Jis0208)),
        (b"$@", Designation::G0(G0::Jis0208)),
        (b"$(B", Designation::G0(G0::Jis0208)),
        (b"$(D", Designation::G0(G0::Jis0212)),
        (b")I", Designation::G1(G1::Katakana)),
        (b"-A", Designation::G1(G1::Latin1)),
    ];
    SEQUENCES.iter().find_map(|(sequence, designation)| {
        rest.starts_with(sequence).then(|| {
            let designation = match designation {
                Designation::G0(set) => Designation::G0(*set),
                Designation::G1(set) => Designation::G1(*set),
            };
            (sequence.len(), designation)
        })
    })
}

fn normalize_term(term: &str) -> String {
    term.replace('_', " ")
        .replace("ISO IR", "ISO_IR")
        .to_ascii_uppercase()
}

fn is_japanese_term(term: &str) -> bool {
    matches!(
        normalize_term(term).as_str(),
        "ISO_IR 13" | "ISO 2022 IR 13" | "ISO 2022 IR 87" | "ISO 2022 IR 159"
    )
}

/// Whether dicom-rs decodes text under this first Specific Character Set value as
/// ISO-8859-1 (its fallback for unknown character sets), so that bytes are recoverable.
fn dicom_rs_reads_as_latin1(first_term: &str) -> bool {
    match SpecificCharacterSet::from_code(first_term) {
        None => true,
        Some(charset) => matches!(charset.name().as_ref(), "ISO_IR 6" | "ISO_IR 100"),
    }
}

/// VRs whose values are affected by Specific Character Set (PS3.5 6.1.2.3).
fn is_charset_sensitive(vr: VR) -> bool {
    matches!(
        vr,
        VR::SH | VR::LO | VR::ST | VR::LT | VR::PN | VR::UC | VR::UT
    )
}

fn latin1_bytes(value: &str) -> Option<Vec<u8>> {
    value
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| char::from(*byte)).collect()
    }

    /// PS3.5 Annex H.3.1
    const YAMADA_IR87: &[u8] =
        b"Yamada^Tarou=\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B=\x1b$B$d$^$@\x1b(B^\x1b$B$?$m$&\x1b(B";

    #[test]
    fn dicom_rs_does_not_decode_japanese_charsets() {
        // If this fails after a dicom-rs upgrade, the library now decodes some Japanese
        // character sets itself. Revisit this module before relying on it (see issue #5).
        for code in [
            "ISO 2022 IR 13",
            "ISO_IR 13",
            "ISO 2022 IR 87",
            "ISO 2022 IR 159",
            "",
        ] {
            assert!(
                SpecificCharacterSet::from_code(code).is_none(),
                "dicom-rs now recognizes {code:?}"
            );
        }
    }

    #[test]
    fn parses_specific_character_set_values() {
        for value in [
            "\\ISO 2022 IR 87",
            "ISO 2022 IR 87",
            "ISO 2022 IR 6\\ISO 2022 IR 87",
            "ISO 2022 IR 13\\ISO 2022 IR 87",
            "ISO_IR 13",
            "ISO 2022 IR 6\\ISO 2022 IR 87\\ISO 2022 IR 159",
        ] {
            assert!(
                Iso2022Charset::from_specific_character_set(value).is_some(),
                "{value:?}"
            );
        }
        for value in ["", "ISO_IR 100", "ISO_IR 192", "ISO 2022 IR 6", "GB18030"] {
            assert!(
                Iso2022Charset::from_specific_character_set(value).is_none(),
                "{value:?}"
            );
        }
    }

    #[test]
    fn decodes_annex_h_3_1_person_name() {
        for value in [
            "\\ISO 2022 IR 87",
            "ISO 2022 IR 87",
            "ISO 2022 IR 6\\ISO 2022 IR 87",
        ] {
            let charset = Iso2022Charset::from_specific_character_set(value).unwrap();
            assert_eq!(
                charset.redecode(VR::PN, &latin1(YAMADA_IR87)).as_deref(),
                Some("Yamada^Tarou=山田^太郎=やまだ^たろう"),
                "{value:?}"
            );
        }
    }

    #[test]
    fn decodes_annex_h_3_2_half_width_katakana() {
        // PS3.5 Annex H.3.2: ISO 2022 IR 13\ISO 2022 IR 87
        let bytes = b"\xd4\xcf\xc0\xde^\xc0\xdb\xb3=\x1b$B;3ED\x1b(J^\x1b$BB@O:\x1b(J=\x1b$B$d$^$@\x1b(J^\x1b$B$?$m$&\x1b(J";
        let charset =
            Iso2022Charset::from_specific_character_set("ISO 2022 IR 13\\ISO 2022 IR 87").unwrap();
        assert_eq!(
            charset.redecode(VR::PN, &latin1(bytes)).as_deref(),
            Some("ﾔﾏﾀﾞ^ﾀﾛｳ=山田^太郎=やまだ^たろう")
        );
    }

    #[test]
    fn backslash_byte_inside_kanji_pair_is_not_a_delimiter() {
        // JIS X 0208 row 0x3F, cell 0x5C: the second byte equals '\\' in ASCII.
        let (kanji, _) = EUC_JP.decode_without_bom_handling(&[0xBF, 0xDC]);
        assert_eq!(kanji.chars().count(), 1);
        let charset = Iso2022Charset::from_specific_character_set("\\ISO 2022 IR 87").unwrap();
        let bytes = b"\x1b$B?\\\x1b(B\\ABC";
        assert_eq!(
            charset.redecode(VR::LO, &latin1(bytes)).as_deref(),
            Some(format!("{kanji}\\ABC").as_str())
        );
    }

    #[test]
    fn decodes_jis_x_0212() {
        let charset =
            Iso2022Charset::from_specific_character_set("\\ISO 2022 IR 87\\ISO 2022 IR 159")
                .unwrap();
        // JIS X 0212 0x30 0x21 is U+4E02 (丂)
        let bytes = b"\x1b$(D0!\x1b(B";
        assert_eq!(
            charset.redecode(VR::LO, &latin1(bytes)).as_deref(),
            Some("丂")
        );
    }

    #[test]
    fn leaves_ascii_and_insensitive_vrs_alone() {
        let charset = Iso2022Charset::from_specific_character_set("\\ISO 2022 IR 87").unwrap();
        assert_eq!(charset.redecode(VR::PN, "Yamada^Tarou"), None);
        assert_eq!(charset.redecode(VR::CS, &latin1(YAMADA_IR87)), None);
        assert_eq!(charset.redecode(VR::PN, "山田"), None);
    }

    #[test]
    fn rejects_unknown_escape_sequences() {
        let charset = Iso2022Charset::from_specific_character_set("\\ISO 2022 IR 87").unwrap();
        assert_eq!(charset.redecode(VR::LO, &latin1(b"\x1b$)CABC")), None);
    }
}
