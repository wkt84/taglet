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
#[derive(Debug, Clone, PartialEq)]
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
            return TextCharset::Iso2022(Iso2022Charset::from_terms(&terms));
        }
        // Unknown first terms fall back to the default repertoire, like dicom-rs does.
        TextCharset::Codec(SpecificCharacterSet::from_code(terms[0]).unwrap_or_default())
    }

    /// Encode text for a value, one value at a time so that every value starts in the
    /// initial character set. `None` when some character cannot be represented.
    pub fn encode(&self, vr: VR, text: &str) -> Result<Vec<u8>, EncodeError> {
        let values = if is_single_valued_text(vr) {
            vec![text]
        } else {
            text.split('\\').collect()
        };
        let encoded = values
            .into_iter()
            .map(|value| match self {
                TextCharset::Iso2022(charset) => charset.encode(vr, value),
                TextCharset::Codec(codec) => codec.encode(value).map_err(|_| {
                    let mut buffer = [0_u8; 4];
                    let c = value
                        .chars()
                        .find(|c| codec.encode(c.encode_utf8(&mut buffer)).is_err())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    EncodeError::Unrepresentable(c)
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(encoded.join(&b'\\'))
    }

    /// Decode a value's bytes, without trailing padding, or `None` when they are not
    /// valid in this character set: where `decode` falls back to ISO 8859-1, or dicom-rs
    /// writes invalid bytes as octal escapes such as "\\377".
    pub fn decode_strict(&self, vr: VR, bytes: &[u8]) -> Option<String> {
        let decoded = match self {
            // Bytes the declared character sets do not cover decode as U+FFFD, or (high
            // bytes without a G1 set) as ISO 8859-1.
            TextCharset::Iso2022(charset) => charset
                .decode_with(vr, bytes, true)
                .filter(|decoded| !decoded.contains(char::REPLACEMENT_CHARACTER)),
            TextCharset::Codec(codec) => codec
                .decode(bytes)
                .ok()
                .filter(|decoded| codec.encode(decoded).is_ok_and(|encoded| encoded == bytes)),
        }?;
        Some(trim_padding(&decoded).to_string())
    }

    /// Decode a value's bytes, without trailing padding.
    pub fn decode(&self, vr: VR, bytes: &[u8]) -> String {
        let decoded = match self {
            TextCharset::Iso2022(charset) => charset.decode_with(vr, bytes, false),
            TextCharset::Codec(codec) => codec.decode(bytes).ok(),
        };
        let decoded = decoded.unwrap_or_else(|| latin1(bytes));
        trim_padding(&decoded).to_string()
    }
}

/// Why text cannot be encoded in a value's character set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// The declared character sets have no code for this character.
    Unrepresentable(char),
    /// The character needs a code extension (escape sequence), which a single-valued
    /// Specific Character Set does not allow (PS3.3 C.12.1.1.2).
    CodeExtensionNotAllowed(char),
    /// The first (alphabetic) component group of a Person Name must not use code
    /// extensions (PS3.5 6.2.1.2).
    FirstPersonNameGroup(char),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncodeError::Unrepresentable(c) => {
                write!(f, "'{c}' cannot be written in this file's character set")
            }
            EncodeError::CodeExtensionNotAllowed(c) => write!(
                f,
                "'{c}' needs an escape sequence, which this file's single-valued Specific Character Set does not allow"
            ),
            EncodeError::FirstPersonNameGroup(c) => write!(
                f,
                "'{c}' cannot be in the first (alphabetic) name group; put kanji and kana after '=', e.g. \"Yamada^Taro=山田^太郎\" or \"=山田^太郎\""
            ),
        }
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
    /// Whether JIS X 0208 (ISO 2022 IR 87) is declared, so that it can be used when
    /// encoding.
    has_jis0208: bool,
    /// Whether escape sequences may be written. Not with a single-valued declaration
    /// such as "ISO_IR 13" (PS3.3 C.12.1.1.2); a lone "ISO 2022 IR 87", seen in the
    /// field, still uses them as there is no other way to write kanji.
    code_extensions: bool,
}

impl Iso2022Charset {
    fn from_terms(terms: &[&str]) -> Self {
        let (initial_g0, initial_g1) = match normalize_term(terms[0]).as_str() {
            "ISO_IR 13" | "ISO 2022 IR 13" => (G0::JisRoman, G1::Katakana),
            "ISO_IR 100" | "ISO 2022 IR 100" => (G0::Ascii, G1::Latin1),
            _ => (G0::Ascii, G1::None),
        };
        let has_jis0208 = terms
            .iter()
            .any(|term| normalize_term(term) == "ISO 2022 IR 87");
        Self {
            initial_g0,
            initial_g1,
            has_jis0208,
            code_extensions: terms.len() > 1 || has_jis0208,
        }
    }

    /// Encode one value. Characters are taken from the initial character sets where
    /// possible and from JIS X 0208 otherwise. Before delimiters and at the end of the
    /// value the encoder switches back to the initial G0 set (PS3.5 6.1.2.5.3, H.3).
    /// JIS X 0212 and switching G1 are not used.
    fn encode(&self, vr: VR, value: &str) -> Result<Vec<u8>, EncodeError> {
        let single_valued = is_single_valued_text(vr);
        let mut out = Vec::with_capacity(value.len() * 2);
        let mut g0 = self.initial_g0;
        let mut first_name_group = vr == VR::PN;
        let switch_g0 = |out: &mut Vec<u8>,
                         g0: &mut G0,
                         target: G0,
                         c: char,
                         first_name_group: bool|
         -> Result<(), EncodeError> {
            if *g0 == target {
                return Ok(());
            }
            if target != self.initial_g0 {
                if first_name_group {
                    return Err(EncodeError::FirstPersonNameGroup(c));
                }
                if !self.code_extensions {
                    return Err(EncodeError::CodeExtensionNotAllowed(c));
                }
            }
            out.extend_from_slice(match target {
                G0::Ascii => b"\x1b(B".as_slice(),
                G0::JisRoman => b"\x1b(J".as_slice(),
                G0::Jis0208 => b"\x1b$B".as_slice(),
                G0::Jis0212 => b"\x1b$(D".as_slice(),
            });
            *g0 = target;
            Ok(())
        };

        for c in value.chars() {
            let code = u32::from(c);
            if c.is_ascii() {
                // JIS X 0201 Romaji differs from ASCII only at 0x5C and 0x7E.
                let roman_ok = c != '\\' && c != '~';
                let target = if self.initial_g0 == G0::JisRoman && roman_ok {
                    G0::JisRoman
                } else {
                    G0::Ascii
                };
                if !(g0 == G0::Ascii || (g0 == G0::JisRoman && roman_ok)) {
                    switch_g0(&mut out, &mut g0, target, c, first_name_group)?;
                }
                out.push(c as u8);
                if c == '=' {
                    first_name_group = false;
                }
            } else if (c == '\u{00A5}' && single_valued) || c == '\u{203E}' {
                // Yen sign and overline exist only in JIS X 0201 Romaji.
                if self.initial_g0 != G0::JisRoman {
                    return Err(EncodeError::Unrepresentable(c));
                }
                switch_g0(&mut out, &mut g0, G0::JisRoman, c, first_name_group)?;
                out.push(if c == '\u{00A5}' { b'\\' } else { b'~' });
            } else if (0xFF61..=0xFF9F).contains(&code) {
                // Half-width katakana in G1, a single-byte set without code extension.
                if self.initial_g1 != G1::Katakana {
                    return Err(EncodeError::Unrepresentable(c));
                }
                out.push(0xA1 + (code - 0xFF61) as u8);
            } else if self.initial_g1 == G1::Latin1 && (0xA0..=0xFF).contains(&code) {
                out.push(code as u8);
            } else {
                if !self.has_jis0208 {
                    return Err(EncodeError::Unrepresentable(c));
                }
                let mut buffer = [0_u8; 4];
                let (euc, _, had_errors) = EUC_JP.encode(c.encode_utf8(&mut buffer));
                match euc.as_ref() {
                    [first, second]
                        if !had_errors && *second >= 0xA1 && is_jis0208_row(first & 0x7F) =>
                    {
                        switch_g0(&mut out, &mut g0, G0::Jis0208, c, first_name_group)?;
                        out.extend_from_slice(&[first & 0x7F, second & 0x7F]);
                    }
                    _ => return Err(EncodeError::Unrepresentable(c)),
                }
            }
        }
        let initial = self.initial_g0;
        switch_g0(&mut out, &mut g0, initial, ' ', false)?;
        Ok(out)
    }

    /// Returns `None` for escape sequences this decoder does not know.
    /// Decode one value. Bytes the declared character sets do not cover decode as
    /// U+FFFD, or, for high bytes with no G1 set designated, as ISO 8859-1, unless
    /// `strict`, which fails on those.
    fn decode_with(&self, vr: VR, bytes: &[u8], strict: bool) -> Option<String> {
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
                    G1::None if strict => return None,
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

/// Whether a row byte (0x21-0x7E) is assigned in JIS X 0208: rows 1-8 and 16-84.
/// The WHATWG table used by encoding_rs also maps vendor extensions, such as the NEC
/// special characters in row 13 (e.g. ①, Ⅰ, ㈱), which are not ISO-IR 87 characters.
fn is_jis0208_row(row: u8) -> bool {
    (0x21..=0x28).contains(&row) || (0x30..=0x74).contains(&row)
}

/// Whether Taglet can write text under this Specific Character Set value: the default
/// repertoire, one single-byte or Unicode character set that dicom-rs encodes, or the
/// Japanese character sets (with ISO 2022 code extensions).
///
/// Terms are read as `from_specific_character_set` reads them, so that a declaration
/// counts as writable only when the character set Taglet uses for it is the declared
/// one.
pub fn is_writable_declaration(value: &str) -> bool {
    let terms = value
        .split('\\')
        .map(|term| term.trim_matches(|c: char| c == ' ' || c == '\0'))
        .collect::<Vec<_>>();
    match terms.as_slice() {
        [""] => true,
        // Single-valued: no code extensions (PS3.3 C.12.1.1.2).
        ["ISO_IR 13"] => true,
        [term] => !is_japanese_term(term) && SpecificCharacterSet::from_code(term).is_some(),
        // Multi-valued: ISO 2022 terms only, the first one (or an empty first value)
        // being the initial character set, and at least one Japanese multi-byte set.
        [first, rest @ ..] => {
            matches!(*first, "" | "ISO 2022 IR 6" | "ISO 2022 IR 13")
                && rest.iter().all(|term| {
                    matches!(
                        *term,
                        "ISO 2022 IR 13" | "ISO 2022 IR 87" | "ISO 2022 IR 159"
                    )
                })
                && rest
                    .iter()
                    .any(|term| matches!(*term, "ISO 2022 IR 87" | "ISO 2022 IR 159"))
        }
        [] => true,
    }
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
pub fn is_single_valued_text(vr: VR) -> bool {
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

/// The strings dicom-rs holds after reading a value with `codec`: one per value, split
/// at `\\` bytes except for single-valued VRs. `None` when decoding fails.
pub fn dicom_rs_read(codec: &SpecificCharacterSet, vr: VR, bytes: &[u8]) -> Option<Vec<String>> {
    if is_single_valued_text(vr) {
        Some(vec![codec.decode(bytes).ok()?])
    } else {
        bytes
            .split(|byte| *byte == b'\\')
            .map(|part| codec.decode(part).ok())
            .collect()
    }
}

/// The bytes dicom-rs writes for the given strings with `codec`.
pub fn dicom_rs_write(codec: &SpecificCharacterSet, parts: &[String]) -> Option<Vec<u8>> {
    let encoded = parts
        .iter()
        .map(|part| codec.encode(part).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(encoded.join(&b'\\'))
}

/// Whether dicom-rs maps every byte to one character and back with `codec`
/// (ISO-8859-1), so that any bytes can be passed through it as a string.
pub fn dicom_rs_codec_is_byte_transparent(codec: &SpecificCharacterSet) -> bool {
    matches!(codec.name().as_ref(), "ISO_IR 6" | "ISO_IR 100")
}

/// The string that dicom-rs writes as exactly `bytes` with a byte-transparent codec.
pub fn byte_transparent_string(bytes: &[u8]) -> String {
    latin1(bytes)
}

/// Whether dicom-rs, reading and writing a value with `codec`, writes back the same bytes.
pub fn dicom_rs_round_trips(codec: &SpecificCharacterSet, vr: VR, bytes: &[u8]) -> bool {
    dicom_rs_read(codec, vr, bytes)
        .and_then(|parts| dicom_rs_write(codec, &parts))
        .is_some_and(|written| written == bytes)
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
    fn strict_decoding_rejects_invalid_bytes() {
        let utf8 = charset("ISO_IR 192");
        assert_eq!(
            utf8.decode_strict(VR::LO, "山田 ".as_bytes()).unwrap(),
            "山田"
        );
        assert!(utf8.decode_strict(VR::LO, b"ID\xff\xfe").is_none());
        let jis = charset("\\ISO 2022 IR 87");
        assert_eq!(
            jis.decode_strict(VR::LO, b"\x1b$BA0N)A#\x1b(B").unwrap(),
            "前立腺"
        );
        // Half-width katakana bytes without ISO 2022 IR 13 declared, or designated.
        assert!(jis.decode_strict(VR::LO, b"\xca\xdd").is_none());
        assert_eq!(
            charset("\\ISO 2022 IR 13")
                .decode_strict(VR::LO, b"\x1b)I\xca\xdd")
                .unwrap(),
            "ﾊﾝ"
        );
        assert_eq!(
            charset("ISO_IR 13")
                .decode_strict(VR::LO, b"\xca\xdd")
                .unwrap(),
            "ﾊﾝ"
        );
    }

    #[test]
    fn writable_declarations() {
        for value in [
            "",
            "ISO_IR 192",
            "ISO_IR 100 ",
            "GB18030",
            "\\ISO 2022 IR 87",
            "ISO 2022 IR 13\\ISO 2022 IR 87",
            "ISO_IR 13",
            "ISO 2022 IR 6\\ISO 2022 IR 87\\ISO 2022 IR 159",
        ] {
            assert!(is_writable_declaration(value), "{value}");
        }
        for value in [
            "\\ISO 2022 IR 149",
            "ISO_IR 100\\ISO 2022 IR 87",
            // Single-valued terms in a multi-valued declaration, and the other way round.
            "ISO_IR 13\\ISO 2022 IR 87",
            "ISO_IR 6\\ISO 2022 IR 87",
            "ISO 2022 IR 87",
            "ISO 2022 IR 6\\ISO 2022 IR 13",
            "ISO_IR 192\\ISO_IR 100",
            "UTF-8",
        ] {
            assert!(!is_writable_declaration(value), "{value}");
        }
        // Spellings dicom-rs does not know are not written as if they were known.
        assert!(!is_writable_declaration("ISO IR 192"));
        assert!(!is_writable_declaration("ISO IR 100"));
        assert_eq!(charset("ISO_IR 192 "), charset("ISO_IR 192"));
        assert_ne!(charset("ISO IR 192"), charset("ISO_IR 192"));
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

    #[test]
    fn encodes_annex_h_examples() {
        // H.3.1 and H.3.2 give the exact bytes, including where escape sequences go.
        assert_eq!(
            charset("\\ISO 2022 IR 87")
                .encode(VR::PN, "Yamada^Tarou=山田^太郎=やまだ^たろう")
                .unwrap(),
            YAMADA_IR87
        );
        let h32 = b"\xd4\xcf\xc0\xde^\xc0\xdb\xb3=\x1b$B;3ED\x1b(J^\x1b$BB@O:\x1b(J=\x1b$B$d$^$@\x1b(J^\x1b$B$?$m$&\x1b(J";
        assert_eq!(
            charset("ISO 2022 IR 13\\ISO 2022 IR 87")
                .encode(VR::PN, "ﾔﾏﾀﾞ^ﾀﾛｳ=山田^太郎=やまだ^たろう")
                .unwrap(),
            h32
        );
    }

    #[test]
    fn encoded_text_decodes_back() {
        let cases = [
            ("\\ISO 2022 IR 87", VR::LO, "前立腺\\PTV 70Gy\\直腸"),
            ("\\ISO 2022 IR 87", VR::LT, "一行目\r\n二行目 \\ 記号"),
            ("ISO 2022 IR 13\\ISO 2022 IR 87", VR::SH, "ｶﾅ漢字ABC"),
            ("ISO_IR 13", VR::ST, "100¥‾ ﾃｽﾄ"),
            ("ISO 2022 IR 100\\ISO 2022 IR 87", VR::LO, "Café 山田"),
        ];
        for (value, vr, text) in cases {
            let charset = charset(value);
            let bytes = charset.encode(vr, text).unwrap();
            assert_eq!(charset.decode(vr, &bytes), text, "{value:?} {text:?}");
        }
    }

    #[test]
    fn plain_ascii_needs_no_escape_sequences() {
        assert_eq!(
            charset("\\ISO 2022 IR 87").encode(VR::LO, "PTV").unwrap(),
            b"PTV"
        );
        assert_eq!(charset("ISO_IR 13").encode(VR::LO, "PTV").unwrap(), b"PTV");
    }

    #[test]
    fn rejects_characters_the_declared_sets_cannot_hold() {
        // No JIS X 0208 declared.
        assert!(charset("ISO_IR 13").encode(VR::LO, "患者").is_err());
        // No JIS X 0201 Katakana declared.
        assert!(charset("\\ISO 2022 IR 87").encode(VR::LO, "ｶﾅ").is_err());
        // Outside JIS X 0208.
        assert!(charset("\\ISO 2022 IR 87").encode(VR::LO, "😀").is_err());
        assert!(charset("\\ISO 2022 IR 87").encode(VR::LO, "£").is_err());
        // A yen sign would be the value delimiter in a multi-valued VR.
        assert!(charset("ISO_IR 13").encode(VR::LO, "¥").is_err());
    }

    #[test]
    fn first_person_name_group_stays_in_the_initial_repertoire() {
        let ir87 = charset("\\ISO 2022 IR 87");
        assert_eq!(
            ir87.encode(VR::PN, "山田^太郎"),
            Err(EncodeError::FirstPersonNameGroup('山'))
        );
        assert!(ir87.encode(VR::PN, "=山田^太郎").is_ok());
        assert!(ir87.encode(VR::PN, "Yamada^Taro=山田^太郎").is_ok());
        // Other VRs may use kanji anywhere.
        assert!(ir87.encode(VR::LO, "山田^太郎").is_ok());
        // Half-width katakana in G1 needs no code extension.
        assert!(charset("ISO 2022 IR 13\\ISO 2022 IR 87")
            .encode(VR::PN, "ﾔﾏﾀﾞ^ﾀﾛｳ=山田^太郎")
            .is_ok());
    }

    #[test]
    fn single_valued_declarations_use_no_escape_sequences() {
        let ir13 = charset("ISO_IR 13");
        assert_eq!(
            ir13.encode(VR::LO, "PTV~"),
            Err(EncodeError::CodeExtensionNotAllowed('~'))
        );
        assert!(ir13.encode(VR::ST, "100¥‾ ﾃｽﾄ").is_ok());
        // With code extensions declared, ASCII can be switched to.
        let bytes = charset("ISO 2022 IR 13\\ISO 2022 IR 87")
            .encode(VR::LO, "PTV~")
            .unwrap();
        assert_eq!(bytes, b"PTV\x1b(B~\x1b(J");
    }

    #[test]
    fn rejects_vendor_extensions_outside_jis_x_0208() {
        let ir87 = charset("\\ISO 2022 IR 87");
        for c in ["①", "Ⅰ", "㈱", "髙"] {
            assert!(ir87.encode(VR::LO, c).is_err(), "{c}");
        }
        for c in ["本", "―", "ヴ", "ω", "凜"] {
            assert!(ir87.encode(VR::LO, c).is_ok(), "{c}");
        }
        // Vendor extensions in existing files are still shown.
        assert_eq!(ir87.decode(VR::LO, b"\x1b$B-!\x1b(B"), "①");
    }
}
