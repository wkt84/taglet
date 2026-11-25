//! Text decoding by Specific Character Set (0008,0005), including Japanese ISO 2022
//! code extensions (PS3.5 6.1.2.5 / Annex H).
//!
//! dicom-rs 0.7 does not support ISO 2022 IR 13 / 87 / 159, only looks at the first
//! value of (0008,0005), and never restores the parent character set when a sequence
//! item ends. Taglet therefore decodes text values from their original bytes itself
//! (see `text_values`), using the rules in this module, and uses the `dicom_rs_*`
//! helpers to tell whether dicom-rs keeps a value's bytes intact on save.

use dicom_core::VR;
use dicom_encoding::text::{SpecificCharacterSet, TextCodec};
use encoding_rs::EUC_JP;

const ESC: u8 = 0x1B;

/// The character set in effect for a dataset or sequence item, per the DICOM standard.
#[derive(Debug, Clone)]
pub enum TextCharset {
    /// Japanese ISO 2022 code extensions.
    Iso2022(Iso2022Charset),
    /// A single character set that dicom-rs can decode.
    Codec(SpecificCharacterSet),
}

impl Default for TextCharset {
    fn default() -> Self {
        TextCharset::Codec(SpecificCharacterSet::default())
    }
}

impl TextCharset {
    /// Interpret the value of Specific Character Set (0008,0005).
    pub fn from_specific_character_set(value: &str) -> Self {
        let terms = value
            .split('\\')
            .map(|term| term.trim_matches(|c: char| c == ' ' || c == '\0'))
            .collect::<Vec<_>>();
        if terms.iter().any(|term| is_japanese_term(term)) {
            return TextCharset::Iso2022(Iso2022Charset::from_first_term(terms[0]));
        }
        // Unknown first terms fall back to the default repertoire, like dicom-rs does.
        TextCharset::Codec(SpecificCharacterSet::from_code(terms[0]).unwrap_or_default())
    }

    /// Decode a value's bytes, without trailing padding.
    pub fn decode(&self, vr: VR, bytes: &[u8]) -> String {
        let decoded = match self {
            TextCharset::Iso2022(charset) => charset.decode(vr, bytes),
            TextCharset::Codec(codec) => codec.decode(bytes).ok(),
        };
        let decoded = decoded.unwrap_or_else(|| latin1(bytes));
        trim_padding(&decoded).to_string()
    }
}

/// Graphic character set designated to G0 (bytes 0x21-0x7E).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum G0 {
    /// ISO 646 / ASCII (ESC ( B)
    Ascii,
    /// JIS X 0201 Romaji (ESC ( J): ASCII except 0x5C (yen sign) and 0x7E (overline).
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
    fn from_first_term(term: &str) -> Self {
        let (initial_g0, initial_g1) = match normalize_term(term).as_str() {
            "ISO_IR 13" | "ISO 2022 IR 13" => (G0::JisRoman, G1::Katakana),
            "ISO_IR 100" | "ISO 2022 IR 100" => (G0::Ascii, G1::Latin1),
            _ => (G0::Ascii, G1::None),
        };
        Self {
            initial_g0,
            initial_g1,
        }
    }

    /// Returns `None` for escape sequences this decoder does not know.
    fn decode(&self, vr: VR, bytes: &[u8]) -> Option<String> {
        let multi_valued = !is_single_valued_text(vr);
        let person_name = vr == VR::PN;
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
                    index += 1;
                    // Delimiters switch back to the initial character sets (PS3.5 6.1.2.5.3).
                    // In multi-byte mode they are only recognised outside of a byte pair,
                    // which the branch above guarantees.
                    let is_value_delimiter = byte == b'\\' && multi_valued;
                    let resets = is_value_delimiter
                        || matches!(byte, b'\r' | b'\n' | 0x0C | b'\t')
                        || (person_name && matches!(byte, b'^' | b'='));
                    let c = match (g0, byte) {
                        (G0::JisRoman, b'\\') if !is_value_delimiter => '\u{00A5}',
                        (G0::JisRoman, b'~') => '\u{203E}',
                        _ => char::from(byte),
                    };
                    out.push(c);
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
    let designation = |sequence: &[u8]| -> Option<Designation> {
        Some(match sequence {
            b"(B" => Designation::G0(G0::Ascii),
            b"(J" => Designation::G0(G0::JisRoman),
            b"$B" | b"$@" => Designation::G0(G0::Jis0208),
            b")I" => Designation::G1(G1::Katakana),
            b"-A" => Designation::G1(G1::Latin1),
            b"$(B" => Designation::G0(G0::Jis0208),
            b"$(D" => Designation::G0(G0::Jis0212),
            _ => return None,
        })
    };
    [2, 3].into_iter().find_map(|len| {
        let sequence = rest.get(..len)?;
        designation(sequence).map(|designation| (len, designation))
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

/// VRs whose values are affected by Specific Character Set (PS3.5 6.1.2.3).
pub fn is_charset_sensitive(vr: VR) -> bool {
    matches!(
        vr,
        VR::SH | VR::LO | VR::ST | VR::LT | VR::PN | VR::UC | VR::UT
    )
}

/// Text VRs that hold a single value, so that `\` is not a value delimiter.
fn is_single_valued_text(vr: VR) -> bool {
    matches!(vr, VR::ST | VR::LT | VR::UT)
}

pub fn trim_padding(value: &str) -> &str {
    value.trim_end_matches([' ', '\0'])
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

/// The text codec dicom-rs switches to when it reads a Specific Character Set value:
/// it looks at the first value only and ignores character sets it does not know.
/// The switch is never undone at the end of a sequence item, in both its reader and
/// its writer.
pub fn dicom_rs_codec_after(
    current: SpecificCharacterSet,
    first_value: Option<&str>,
) -> SpecificCharacterSet {
    first_value
        .and_then(SpecificCharacterSet::from_code)
        .unwrap_or(current)
}

/// Whether dicom-rs, reading and writing a value with `codec`, writes back the same bytes.
pub fn dicom_rs_round_trips(codec: &SpecificCharacterSet, vr: VR, bytes: &[u8]) -> bool {
    let round_trips = |part: &[u8]| match codec.decode(part) {
        Ok(text) => codec.encode(&text).is_ok_and(|encoded| encoded == part),
        Err(_) => false,
    };
    if is_single_valued_text(vr) {
        round_trips(bytes)
    } else {
        bytes.split(|byte| *byte == b'\\').all(round_trips)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_encoding::text::DefaultCharacterSetCodec;

    /// PS3.5 Annex H.3.1
    const YAMADA_IR87: &[u8] =
        b"Yamada^Tarou=\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B=\x1b$B$d$^$@\x1b(B^\x1b$B$?$m$&\x1b(B";

    fn charset(value: &str) -> TextCharset {
        TextCharset::from_specific_character_set(value)
    }

    #[test]
    fn interprets_specific_character_set_values() {
        for value in [
            "\\ISO 2022 IR 87",
            "ISO 2022 IR 87",
            "ISO 2022 IR 6\\ISO 2022 IR 87",
            "ISO 2022 IR 13\\ISO 2022 IR 87",
            "ISO_IR 13",
            "ISO 2022 IR 6\\ISO 2022 IR 87\\ISO 2022 IR 159",
        ] {
            assert!(
                matches!(charset(value), TextCharset::Iso2022(_)),
                "{value:?}"
            );
        }
        for value in ["", "ISO_IR 100", "ISO_IR 192", "ISO 2022 IR 6", "GB18030"] {
            assert!(matches!(charset(value), TextCharset::Codec(_)), "{value:?}");
        }
    }

    #[test]
    fn decodes_annex_h_3_1_person_name() {
        for value in [
            "\\ISO 2022 IR 87",
            "ISO 2022 IR 87",
            "ISO 2022 IR 6\\ISO 2022 IR 87",
        ] {
            assert_eq!(
                charset(value).decode(VR::PN, YAMADA_IR87),
                "Yamada^Tarou=山田^太郎=やまだ^たろう",
                "{value:?}"
            );
        }
    }

    #[test]
    fn decodes_annex_h_3_2_half_width_katakana() {
        // PS3.5 Annex H.3.2: ISO 2022 IR 13\ISO 2022 IR 87
        let bytes = b"\xd4\xcf\xc0\xde^\xc0\xdb\xb3=\x1b$B;3ED\x1b(J^\x1b$BB@O:\x1b(J=\x1b$B$d$^$@\x1b(J^\x1b$B$?$m$&\x1b(J";
        assert_eq!(
            charset("ISO 2022 IR 13\\ISO 2022 IR 87").decode(VR::PN, bytes),
            "ﾔﾏﾀﾞ^ﾀﾛｳ=山田^太郎=やまだ^たろう"
        );
    }

    #[test]
    fn jis_roman_yen_sign_and_overline() {
        // PS3.5 H.1.1: in JIS X 0201 Romaji, 0x5C is a yen sign and 0x7E an overline.
        let ir13 = charset("ISO_IR 13");
        assert_eq!(ir13.decode(VR::ST, b"100\\~ "), "100¥‾");
        // In multi-valued VRs, 0x5C stays the value delimiter.
        assert_eq!(ir13.decode(VR::LO, b"A~\\B"), "A‾\\B");
        // After switching G0 to ASCII, 0x5C and 0x7E are ASCII again.
        assert_eq!(
            charset("ISO 2022 IR 13\\ISO 2022 IR 87").decode(VR::ST, b"\x1b(B\\~"),
            "\\~"
        );
    }

    #[test]
    fn backslash_byte_inside_kanji_pair_is_not_a_delimiter() {
        // JIS X 0208 row 0x3F, cell 0x5C: the second byte equals '\\' in ASCII.
        let (kanji, _) = EUC_JP.decode_without_bom_handling(&[0xBF, 0xDC]);
        assert_eq!(kanji.chars().count(), 1);
        assert_eq!(
            charset("\\ISO 2022 IR 87").decode(VR::LO, b"\x1b$B?\\\x1b(B\\ABC"),
            format!("{kanji}\\ABC")
        );
    }

    #[test]
    fn decodes_jis_x_0212() {
        // JIS X 0212 0x30 0x21 is U+4E02 (丂)
        assert_eq!(
            charset("\\ISO 2022 IR 87\\ISO 2022 IR 159").decode(VR::LO, b"\x1b$(D0!\x1b(B"),
            "丂"
        );
    }

    #[test]
    fn unknown_escape_sequences_fall_back_to_latin1() {
        assert_eq!(
            charset("\\ISO 2022 IR 87").decode(VR::LO, b"\x1b$)CA"),
            "\u{1b}$)CA"
        );
    }

    #[test]
    fn decodes_other_character_sets_with_dicom_rs_codecs() {
        assert_eq!(
            charset("ISO_IR 192").decode(VR::LO, "前立腺 ".as_bytes()),
            "前立腺"
        );
        assert_eq!(charset("ISO_IR 100").decode(VR::LO, b"Caf\xe9"), "Café");
        assert_eq!(charset("").decode(VR::LO, b"ABC\0"), "ABC");
    }

    #[test]
    fn simulates_dicom_rs_codec_switching() {
        let default = SpecificCharacterSet::default();
        let utf8 = dicom_rs_codec_after(default, Some("ISO_IR 192"));
        assert_eq!(utf8.name(), "ISO_IR 192");
        // Unknown and empty first values keep the current codec.
        assert_eq!(
            dicom_rs_codec_after(utf8.clone(), Some("ISO_IR 13")).name(),
            "ISO_IR 192"
        );
        assert_eq!(
            dicom_rs_codec_after(utf8.clone(), Some("")).name(),
            "ISO_IR 192"
        );
        assert_eq!(
            dicom_rs_codec_after(utf8.clone(), None).name(),
            "ISO_IR 192"
        );
    }

    #[test]
    fn detects_values_dicom_rs_cannot_write_back() {
        let latin1 = SpecificCharacterSet::default();
        let utf8 = SpecificCharacterSet::ISO_IR_192;
        assert_eq!(
            DefaultCharacterSetCodec.decode(&[0xCA]).unwrap(),
            "\u{00CA}"
        );
        assert!(dicom_rs_round_trips(&latin1, VR::PN, YAMADA_IR87));
        assert!(dicom_rs_round_trips(&latin1, VR::LO, b"\xca\xdd"));
        assert!(!dicom_rs_round_trips(&utf8, VR::LO, b"\xca\xdd"));
        assert!(dicom_rs_round_trips(&utf8, VR::LO, "前立腺".as_bytes()));
    }
}
