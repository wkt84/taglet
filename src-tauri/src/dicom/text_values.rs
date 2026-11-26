//! Text values read from a file's original bytes.
//!
//! dicom-rs decodes text with the character set handling described in `charset`, which
//! is wrong for Japanese ISO 2022 character sets and for character sets set inside
//! sequence items. This module walks the file once more, keeps the original bytes of
//! every value whose VR is affected by Specific Character Set, decodes them with the
//! character set the DICOM standard says applies, and records whether dicom-rs writes
//! those bytes back unchanged on save.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_encoding::text::SpecificCharacterSet;
use dicom_encoding::transfer_syntax::TransferSyntaxIndex;
use dicom_object::FileMetaTable;
use dicom_parser::dataset::lazy_read::LazyDataSetReader;
use dicom_parser::dataset::LazyDataToken;
use dicom_parser::stateful::decode::StatefulDecode;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;

use super::charset::{
    dicom_rs_codec_after, dicom_rs_codec_is_byte_transparent, dicom_rs_round_trips,
    is_charset_sensitive, TextCharset,
};
use super::deflate;

const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);

#[derive(Debug, Clone)]
pub struct TextValue {
    /// The character set in effect for the value's dataset or item.
    pub charset: TextCharset,
    pub vr: VR,
    /// The original bytes of the value.
    pub bytes: Vec<u8>,
    /// The codec dicom-rs used to read the value.
    pub read_codec: SpecificCharacterSet,
    /// The value decoded with the character set in effect for its dataset or item.
    pub decoded: String,
    /// Whether dicom-rs writes the original bytes back unchanged when the file is saved
    /// without changes.
    pub round_trips: bool,
    /// Whether the value can be edited: dicom-rs reads and writes it byte for byte, so
    /// Taglet can hand it the bytes it encodes itself.
    pub editable: bool,
}

/// Text values keyed by node path, e.g. `["(3006,0020)", "Item#0", "(3006,0026)"]`.
#[derive(Debug, Clone, Default)]
pub struct TextValues {
    values: HashMap<Vec<String>, TextValue>,
}

impl TextValues {
    /// Read the text values of a file. Returns no values when the file cannot be walked,
    /// in which case Taglet falls back to the values dicom-rs decoded.
    pub fn read(path: &str) -> Self {
        Self::try_read(path).unwrap_or_default()
    }

    /// Read the text values of a file, failing when the file cannot be walked. Saving
    /// uses this so that values that could not be checked never pass as safe.
    pub fn try_read(path: &str) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|error| error.to_string())?;
        let mut head = [0_u8; 132];
        let head_len = read_up_to(&mut file, &mut head)?;
        let meta_offset = if head_len == 132 && &head[128..132] == b"DICM" {
            Some(128)
        } else if head_len >= 4 && &head[..4] == b"DICM" {
            Some(0)
        } else {
            None
        };

        let mut reader = BufReader::new(file);
        let transfer_syntax_uid = match meta_offset {
            Some(offset) => {
                reader
                    .seek(SeekFrom::Start(offset))
                    .map_err(|error| error.to_string())?;
                let meta =
                    FileMetaTable::from_reader(&mut reader).map_err(|error| error.to_string())?;
                meta.transfer_syntax().to_string()
            }
            None => {
                // Raw dataset without file meta information, read as Implicit VR Little
                // Endian like `parser::open_nodes` does.
                reader
                    .seek(SeekFrom::Start(0))
                    .map_err(|error| error.to_string())?;
                uids::IMPLICIT_VR_LITTLE_ENDIAN.to_string()
            }
        };
        let mut values = HashMap::new();
        // Deflated data sets are inflated first (see `deflate`); they are Explicit VR
        // Little Endian.
        if deflate::is_deflated(&transfer_syntax_uid) {
            let mut inflated = Vec::new();
            deflate::inflate_into(&mut reader, &mut inflated)?;
            let transfer_syntax = TransferSyntaxRegistry
                .get(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .ok_or_else(|| "Explicit VR Little Endian is not available".to_string())?;
            let mut dataset = LazyDataSetReader::new_with_ts_cs(
                std::io::Cursor::new(inflated),
                transfer_syntax,
                SpecificCharacterSet::default(),
            )
            .map_err(|error| error.to_string())?;
            walk(&mut dataset, &mut values)?;
            return Ok(Self { values });
        }
        let transfer_syntax = TransferSyntaxRegistry
            .get(&transfer_syntax_uid)
            .ok_or_else(|| format!("Unknown transfer syntax {transfer_syntax_uid}"))?;
        // Other deflated transfer syntaxes (JPIP Referenced Deflate) are not walked.
        let deflated = transfer_syntax
            .name()
            .to_ascii_lowercase()
            .contains("deflate");
        if deflated || !transfer_syntax.can_decode_dataset() {
            return Err(format!(
                "Transfer syntax {transfer_syntax_uid} cannot be walked"
            ));
        }
        let mut dataset = LazyDataSetReader::new_with_ts_cs(
            reader,
            transfer_syntax,
            SpecificCharacterSet::default(),
        )
        .map_err(|error| error.to_string())?;

        walk(&mut dataset, &mut values)?;
        Ok(Self { values })
    }

    pub fn get(&self, path: &[String]) -> Option<&TextValue> {
        self.values.get(path)
    }

    /// Paths of values that dicom-rs would not write back unchanged.
    pub fn paths_not_round_tripping(&self) -> Vec<&[String]> {
        let mut paths = self
            .values
            .iter()
            .filter(|(_, value)| !value.round_trips)
            .map(|(path, _)| path.as_slice())
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
}

fn read_up_to(file: &mut File, buffer: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(filled)
}

enum Frame {
    Dataset {
        path: Vec<String>,
        charset: TextCharset,
    },
    Sequence {
        path: Vec<String>,
        charset: TextCharset,
        next_item: usize,
    },
}

fn walk<S>(
    reader: &mut LazyDataSetReader<S>,
    values: &mut HashMap<Vec<String>, TextValue>,
) -> Result<(), String>
where
    S: StatefulDecode,
{
    let mut frames = vec![Frame::Dataset {
        path: Vec::new(),
        charset: TextCharset::default(),
    }];
    // dicom-rs keeps a single codec for the whole stream (see `dicom_rs_codec_after`).
    let mut dicom_rs_codec = SpecificCharacterSet::default();

    while let Some(token) = reader.advance() {
        let token = token.map_err(|error| error.to_string())?;
        match token {
            LazyDataToken::ElementHeader(_) => {}
            token @ LazyDataToken::LazyValue { .. } => {
                let LazyDataToken::LazyValue { header, .. } = &token else {
                    unreachable!();
                };
                let (tag, vr, length) = (header.tag, header.vr, header.len);
                let Some(Frame::Dataset { path, charset }) = frames.last_mut() else {
                    return Err("Element value outside of a dataset".to_string());
                };
                if tag == PIXEL_DATA && path.is_empty() {
                    break;
                }
                if tag == tags::SPECIFIC_CHARACTER_SET {
                    let value = token.into_value().map_err(|error| error.to_string())?;
                    let strings = value.to_multi_str();
                    *charset = TextCharset::from_specific_character_set(&value.to_str());
                    dicom_rs_codec =
                        dicom_rs_codec_after(dicom_rs_codec, strings.first().map(String::as_str));
                } else if is_charset_sensitive(vr) && length.get().is_some() {
                    let mut bytes = Vec::new();
                    token
                        .read_value_into(&mut bytes)
                        .map_err(|error| error.to_string())?;
                    let mut element_path = path.clone();
                    element_path.push(format_tag(tag));
                    values.insert(
                        element_path,
                        TextValue {
                            charset: charset.clone(),
                            vr,
                            decoded: charset.decode(vr, &bytes),
                            round_trips: dicom_rs_round_trips(&dicom_rs_codec, vr, &bytes),
                            editable: dicom_rs_codec_is_byte_transparent(&dicom_rs_codec),
                            read_codec: dicom_rs_codec.clone(),
                            bytes,
                        },
                    );
                } else {
                    token.skip().map_err(|error| error.to_string())?;
                }
            }
            LazyDataToken::SequenceStart { tag, .. } => {
                let Some(Frame::Dataset { path, charset }) = frames.last() else {
                    return Err("Sequence outside of a dataset".to_string());
                };
                let mut sequence_path = path.clone();
                sequence_path.push(format_tag(tag));
                frames.push(Frame::Sequence {
                    path: sequence_path,
                    charset: charset.clone(),
                    next_item: 0,
                });
            }
            LazyDataToken::ItemStart { .. } => {
                let Some(Frame::Sequence {
                    path,
                    charset,
                    next_item,
                }) = frames.last_mut()
                else {
                    return Err("Item outside of a sequence".to_string());
                };
                let mut item_path = path.clone();
                item_path.push(format!("Item#{next_item}"));
                *next_item += 1;
                let charset = charset.clone();
                frames.push(Frame::Dataset {
                    path: item_path,
                    charset,
                });
            }
            LazyDataToken::ItemEnd | LazyDataToken::SequenceEnd => {
                if frames.len() > 1 {
                    frames.pop();
                }
            }
            LazyDataToken::PixelSequenceStart => break,
            LazyDataToken::LazyItemValue { .. } => {
                token.skip().map_err(|error| error.to_string())?;
            }
            _ => return Err("Unsupported DICOM token".to_string()),
        }
    }

    Ok(())
}

fn format_tag(tag: Tag) -> String {
    format!("({:04X},{:04X})", tag.group(), tag.element())
}
