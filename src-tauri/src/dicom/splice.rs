//! Saving by copying the original file and rewriting only edited values (#8).
//!
//! The file is indexed once: where each element's header and value are, and how
//! sequences and items nest. Saving then streams the original bytes and replaces only
//! the values that were edited, along with the length fields that change with them
//! (the element's own and those of enclosing defined-length items and sequences).
//! Everything else, including text in character sets dicom-rs cannot write and the
//! pixel data, is copied byte for byte.
//!
//! Supported: Part 10 files whose data set is Explicit VR Little Endian (including
//! encapsulated pixel data), Implicit VR Little Endian or Explicit VR Big Endian, and
//! deflated ones (Deflated Explicit VR Little Endian), which are inflated to be indexed
//! and spliced, and deflated again when written (see `deflate`).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom, Write};

use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom_core::{Tag, VR};
use dicom_dictionary_std::{uids, StandardDataDictionary};
use std::str::FromStr;

use super::deflate;

const ITEM: Tag = Tag(0xFFFE, 0xE000);
const ITEM_DELIMITER: Tag = Tag(0xFFFE, 0xE00D);
const SEQUENCE_DELIMITER: Tag = Tag(0xFFFE, 0xE0DD);
const UNDEFINED: u32 = u32::MAX;

const EXPLICIT_VR_BIG_ENDIAN: &str = "1.2.840.10008.1.2.2";

/// Transfer syntaxes whose data set is encoded as Explicit or Implicit VR Little
/// Endian, or as Explicit VR Big Endian, including the deflated one.
pub fn supports_transfer_syntax(uid: &str) -> bool {
    Encoding::of_transfer_syntax(uid).is_some()
}

/// How a data set is encoded: Explicit or Implicit VR, and the byte order of tags,
/// lengths and binary values. Text values read the same in either byte order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub explicit: bool,
    pub big_endian: bool,
}

impl Encoding {
    /// The file meta information, which is always Explicit VR Little Endian.
    const META: Self = Self {
        explicit: true,
        big_endian: false,
    };

    /// The encoding of the data set for a transfer syntax, if splicing supports it.
    pub fn of_transfer_syntax(uid: &str) -> Option<Self> {
        let uid = uid.trim_end_matches(['\0', ' ']);
        if uid == uids::IMPLICIT_VR_LITTLE_ENDIAN {
            Some(Self {
                explicit: false,
                big_endian: false,
            })
        } else if uid == EXPLICIT_VR_BIG_ENDIAN {
            Some(Self {
                explicit: true,
                big_endian: true,
            })
        } else if is_explicit_vr_little_endian(uid) || deflate::is_deflated(uid) {
            Some(Self::META)
        } else {
            None
        }
    }

    fn u16_bytes(self, value: u16) -> [u8; 2] {
        if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    }

    fn u32_bytes(self, value: u32) -> [u8; 4] {
        if self.big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        }
    }

    fn read_u16(self, bytes: [u8; 2]) -> u16 {
        if self.big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        }
    }

    fn read_u32(self, bytes: [u8; 4]) -> u32 {
        if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    }

    fn tag_bytes(self, tag: Tag) -> [u8; 4] {
        let [a, b] = self.u16_bytes(tag.group());
        let [c, d] = self.u16_bytes(tag.element());
        [a, b, c, d]
    }
}

/// Transfer syntaxes whose data set is encoded as Explicit VR Little Endian.
fn is_explicit_vr_little_endian(uid: &str) -> bool {
    // Standard transfer syntaxes other than Implicit VR Little Endian (1.2.840.10008.1.2)
    // all live under 1.2.840.10008.1.2. and encode the data set as Explicit VR Little
    // Endian, except for Big Endian and the deflated ones.
    uid.starts_with("1.2.840.10008.1.2.")
        && !matches!(
            uid,
            EXPLICIT_VR_BIG_ENDIAN // Explicit VR Big Endian (retired)
            | uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN
            | "1.2.840.10008.1.2.4.95" // JPIP Referenced Deflate
        )
}

#[derive(Debug, Clone)]
pub enum IndexNode {
    Element {
        tag: Tag,
        /// `None` for VR bytes that are not a known VR.
        vr: Option<VR>,
        vr_bytes: [u8; 2],
        /// Offset of the element header.
        start: u64,
        /// Offset just after the value.
        end: u64,
    },
    Sequence {
        tag: Tag,
        vr: [u8; 2],
        start: u64,
        /// Length of the sequence's own header (12 bytes in Explicit VR, 8 in Implicit).
        header_length: u64,
        /// Offset just after the sequence, including its delimiter if any.
        end: u64,
        defined_length: bool,
        items: Vec<IndexItem>,
    },
    /// Encapsulated pixel data or another element copied as a whole.
    Opaque { tag: Tag, start: u64, end: u64 },
}

impl IndexNode {
    pub fn tag(&self) -> Tag {
        match self {
            IndexNode::Element { tag, .. }
            | IndexNode::Sequence { tag, .. }
            | IndexNode::Opaque { tag, .. } => *tag,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexItem {
    start: u64,
    end: u64,
    defined_length: bool,
    nodes: Vec<IndexNode>,
}

/// The layout of a Part 10 file.
#[derive(Debug, Clone)]
pub struct FileIndex {
    /// The file's modification time when indexed, to detect a replaced file.
    modified: Option<std::time::SystemTime>,
    /// Offset of the file meta information (after the preamble and "DICM").
    meta_start: u64,
    /// The file meta information elements, in file order.
    meta: Vec<IndexNode>,
    /// Offset of the first data set element (after the file meta information).
    dataset_start: u64,
    /// How the data set is encoded.
    encoding: Encoding,
    /// Whether the data set is deflated: offsets after `dataset_start` are then offsets
    /// in the inflated data set (see `deflate::InflatedFile`).
    deflated: bool,
    /// The length of the file, as it was indexed.
    file_length: u64,
    /// The length of what offsets refer to: the file, or the file meta information
    /// followed by the inflated data set.
    source_length: u64,
    nodes: Vec<IndexNode>,
}

impl FileIndex {
    pub fn read(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        let file_length = metadata.len();
        let modified = metadata.modified().ok();
        let mut reader = Reader {
            inner: Box::new(BufReader::new(file)),
            position: 0,
            length: file_length,
            // File meta information (group 0002) is always Explicit VR Little Endian.
            encoding: Encoding::META,
        };
        let mut head = [0_u8; 132];
        reader.read_exact(&mut head)?;
        if &head[128..132] != b"DICM" {
            return Err("Not a DICOM Part 10 file with a preamble".to_string());
        }
        let meta_start = reader.position;
        let meta = read_nodes(&mut reader, file_length, |tag| tag.group() != 0x0002)?;
        if meta
            .iter()
            .any(|node| !matches!(node, IndexNode::Element { .. }))
        {
            return Err("Unexpected structure in the file meta information".to_string());
        }
        let dataset_start = reader.position;
        let transfer_syntax = read_transfer_syntax(&mut reader, &meta)?;
        let Some(encoding) = Encoding::of_transfer_syntax(&transfer_syntax) else {
            return Err(format!(
                "Transfer syntax {transfer_syntax} is not supported"
            ));
        };
        let deflated = deflate::is_deflated(&transfer_syntax);
        if deflated {
            // Index the file meta information followed by the inflated data set.
            let bytes = inflated_source(&mut *reader.inner, dataset_start)?;
            reader.length = bytes.len() as u64;
            reader.inner = Box::new(Cursor::new(bytes));
            reader.inner.seek_to(dataset_start)?;
        }
        reader.encoding = encoding;
        let source_length = reader.length;
        let nodes = read_nodes(&mut reader, source_length, |_| false)?;
        Ok(Self {
            modified,
            encoding,
            deflated,
            source_length,
            meta_start,
            meta,
            dataset_start,
            file_length,
            nodes,
        })
    }

    pub fn nodes(&self) -> &[IndexNode] {
        &self.nodes
    }

    /// How the data set is encoded.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }
}

/// The Transfer Syntax UID (0002,0010) from the indexed file meta information.
fn read_transfer_syntax(reader: &mut Reader, meta: &[IndexNode]) -> Result<String, String> {
    let Some(IndexNode::Element {
        vr_bytes,
        start,
        end,
        ..
    }) = meta.iter().find(|node| node.tag() == Tag(0x0002, 0x0010))
    else {
        return Err("The file meta information has no transfer syntax".to_string());
    };
    let header_length = if has_32_bit_length(vr_bytes) { 12 } else { 8 };
    let value = reader.read_at(start + header_length, *end)?;
    Ok(String::from_utf8_lossy(&value)
        .trim_end_matches(['\0', ' '])
        .to_string())
}

/// The VR of a tag in the standard dictionary, as used for Implicit VR data sets.
fn dictionary_vr(tag: Tag) -> Option<VR> {
    StandardDataDictionary
        .by_tag(tag)
        .map(|entry| entry.vr().relaxed())
}

/// Where indexed bytes are read from: a file, or a file with its data set inflated.
trait Source: Read {
    fn seek_to(&mut self, position: u64) -> Result<(), String>;
    /// Move by `offset` from the current position, keeping buffered data.
    fn skip_by(&mut self, offset: i64) -> Result<(), String>;
}

impl Source for BufReader<File> {
    fn seek_to(&mut self, position: u64) -> Result<(), String> {
        self.seek(SeekFrom::Start(position))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn skip_by(&mut self, offset: i64) -> Result<(), String> {
        self.seek_relative(offset)
            .map_err(|error| error.to_string())
    }
}

impl Source for File {
    fn seek_to(&mut self, position: u64) -> Result<(), String> {
        self.seek(SeekFrom::Start(position))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn skip_by(&mut self, offset: i64) -> Result<(), String> {
        self.seek(SeekFrom::Current(offset))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

impl Source for Cursor<Vec<u8>> {
    fn seek_to(&mut self, position: u64) -> Result<(), String> {
        self.set_position(position);
        Ok(())
    }

    fn skip_by(&mut self, offset: i64) -> Result<(), String> {
        self.seek(SeekFrom::Current(offset))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// The bytes of a deflated file up to `dataset_start` (the preamble and the file meta
/// information), followed by its inflated data set.
fn inflated_source(file: &mut dyn Source, dataset_start: u64) -> Result<Vec<u8>, String> {
    file.seek_to(0)?;
    let mut bytes = Vec::new();
    file.take(dataset_start)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != dataset_start {
        return Err("Unexpected end of file".to_string());
    }
    deflate::inflate_into(file, &mut bytes)?;
    Ok(bytes)
}

struct Reader {
    inner: Box<dyn Source>,
    position: u64,
    length: u64,
    encoding: Encoding,
}

impl Reader {
    /// Read the bytes from `start` to `end`, then return to the current position.
    fn read_at(&mut self, start: u64, end: u64) -> Result<Vec<u8>, String> {
        let position = self.position;
        let mut buffer = vec![0_u8; end.saturating_sub(start) as usize];
        self.inner.seek_to(start)?;
        self.position = start;
        self.read_exact(&mut buffer)?;
        self.inner.seek_to(position)?;
        self.position = position;
        Ok(buffer)
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), String> {
        self.inner
            .read_exact(buffer)
            .map_err(|error| format!("Unexpected end of file at {}: {error}", self.position))?;
        self.position += buffer.len() as u64;
        Ok(())
    }

    fn skip(&mut self, length: u64) -> Result<(), String> {
        if self.position + length > self.length {
            return Err(format!(
                "Value at {} runs past the end of the file",
                self.position
            ));
        }
        self.inner.skip_by(length as i64)?;
        self.position += length;
        Ok(())
    }

    fn tag_and_length(&mut self) -> Result<(Tag, [u8; 2], u32, bool), String> {
        let mut header = [0_u8; 8];
        self.read_exact(&mut header)?;
        let encoding = self.encoding;
        let tag = Tag(
            encoding.read_u16([header[0], header[1]]),
            encoding.read_u16([header[2], header[3]]),
        );
        if tag.group() == 0xFFFE || !encoding.explicit {
            // Items, delimiters and Implicit VR elements: no VR, 32-bit length.
            let length = encoding.read_u32([header[4], header[5], header[6], header[7]]);
            return Ok((tag, [0, 0], length, true));
        }
        let vr = [header[4], header[5]];
        if has_32_bit_length(&vr) {
            let mut length = [0_u8; 4];
            self.read_exact(&mut length)?;
            Ok((tag, vr, encoding.read_u32(length), true))
        } else {
            Ok((
                tag,
                vr,
                u32::from(encoding.read_u16([header[6], header[7]])),
                false,
            ))
        }
    }

    fn peek_tag(&mut self) -> Result<Option<Tag>, String> {
        if self.position + 4 > self.length {
            return Ok(None);
        }
        let mut buffer = [0_u8; 4];
        self.inner
            .read_exact(&mut buffer)
            .map_err(|error| error.to_string())?;
        self.inner.skip_by(-4)?;
        Ok(Some(Tag(
            self.encoding.read_u16([buffer[0], buffer[1]]),
            self.encoding.read_u16([buffer[2], buffer[3]]),
        )))
    }
}

/// VRs whose value length is a 32-bit field in Explicit VR (PS3.5 7.1.2).
fn has_32_bit_length(vr: &[u8; 2]) -> bool {
    matches!(
        vr,
        b"OB"
            | b"OD"
            | b"OF"
            | b"OL"
            | b"OV"
            | b"OW"
            | b"SQ"
            | b"SV"
            | b"UC"
            | b"UN"
            | b"UR"
            | b"UT"
            | b"UV"
    )
}

/// Read elements until `stop` returns true for the next tag, the end of the file, or
/// (inside an item) the item ends.
fn read_nodes(
    reader: &mut Reader,
    end: u64,
    stop: impl Fn(Tag) -> bool + Copy,
) -> Result<Vec<IndexNode>, String> {
    let mut nodes: Vec<IndexNode> = Vec::new();
    let mut seen = HashSet::new();
    while reader.position < end {
        let Some(tag) = reader.peek_tag()? else {
            break;
        };
        if stop(tag) || tag == ITEM_DELIMITER {
            break;
        }
        // Splicing addresses elements by tag, so tags must be unique.
        if !seen.insert(tag) {
            return Err(format!("Element {tag} is repeated"));
        }
        nodes.push(read_node(reader)?);
    }
    Ok(nodes)
}

fn read_node(reader: &mut Reader) -> Result<IndexNode, String> {
    let start = reader.position;
    let (tag, read_vr, length, _) = reader.tag_and_length()?;
    if tag.group() == 0xFFFE {
        return Err(format!("Unexpected item tag at {start}"));
    }
    let header_length = reader.position - start;
    // Implicit VR elements take their VR from the dictionary, like dicom-rs does: a
    // sequence when the dictionary says SQ or the length is undefined (except for
    // Pixel Data, whose fragments are not items with data sets).
    let implicit_vr = (!reader.encoding.explicit).then(|| dictionary_vr(tag));
    let vr = match implicit_vr {
        Some(Some(vr)) => vr.to_bytes(),
        Some(None) => *b"UN",
        None => read_vr,
    };
    let implicit_sequence = implicit_vr.is_some()
        && tag != Tag(0x7FE0, 0x0010)
        && (length == UNDEFINED || &vr == b"SQ");
    if &vr == b"SQ" || implicit_sequence || (length == UNDEFINED && &vr == b"UN") {
        if &vr == b"UN" && !implicit_sequence {
            return Err(format!(
                "Undefined-length UN element {tag} at {start} is not supported"
            ));
        }
        let defined_length = length != UNDEFINED;
        let value_end = if defined_length {
            Some(reader.position + u64::from(length))
        } else {
            None
        };
        let mut items = Vec::new();
        loop {
            if value_end.is_some_and(|value_end| reader.position >= value_end) {
                break;
            }
            let item_start = reader.position;
            let (item_tag, _, item_length, _) = reader.tag_and_length()?;
            if item_tag == SEQUENCE_DELIMITER {
                break;
            }
            if item_tag != ITEM {
                return Err(format!(
                    "Expected an item at {item_start}, found {item_tag}"
                ));
            }
            let item_defined = item_length != UNDEFINED;
            let nodes = if item_defined {
                let item_end = reader.position + u64::from(item_length);
                let nodes = read_nodes(reader, item_end, |_| false)?;
                if reader.position != item_end {
                    return Err(format!("Item at {item_start} has an inconsistent length"));
                }
                nodes
            } else {
                let nodes = read_nodes(reader, reader.length, |_| false)?;
                let (delimiter, _, _, _) = reader.tag_and_length()?;
                if delimiter != ITEM_DELIMITER {
                    return Err(format!("Item at {item_start} is not delimited"));
                }
                nodes
            };
            items.push(IndexItem {
                start: item_start,
                end: reader.position,
                defined_length: item_defined,
                nodes,
            });
        }
        if let Some(value_end) = value_end {
            if reader.position != value_end {
                return Err(format!(
                    "Sequence {tag} at {start} has an inconsistent length"
                ));
            }
        }
        return Ok(IndexNode::Sequence {
            tag,
            vr: *b"SQ",
            start,
            header_length,
            end: reader.position,
            defined_length,
            items,
        });
    }
    if length == UNDEFINED {
        // Encapsulated pixel data: fragments until the sequence delimiter.
        loop {
            let (item_tag, _, item_length, _) = reader.tag_and_length()?;
            if item_tag == SEQUENCE_DELIMITER {
                break;
            }
            if item_tag != ITEM || item_length == UNDEFINED {
                return Err(format!("Unexpected fragment structure in {tag}"));
            }
            reader.skip(u64::from(item_length))?;
        }
        return Ok(IndexNode::Opaque {
            tag,
            start,
            end: reader.position,
        });
    }
    reader.skip(u64::from(length))?;
    let known_vr = match implicit_vr {
        Some(dictionary) => dictionary,
        None => std::str::from_utf8(&vr)
            .ok()
            .and_then(|text| VR::from_str(text).ok()),
    };
    Ok(IndexNode::Element {
        tag,
        vr: known_vr,
        vr_bytes: vr,
        start,
        end: reader.position,
    })
}

/// Changes to splice into a file. Paths are node paths (as in `DicomNode::path`).
#[derive(Debug, Default)]
pub struct Edits {
    /// New values of existing elements, encoded and padded to an even length.
    pub replacements: HashMap<Vec<String>, Vec<u8>>,
    /// New elements (tag and encoded element, header included) by the path of the data
    /// set or item they go into (empty for the top level).
    pub insertions: HashMap<Vec<String>, Vec<(Tag, Vec<u8>)>>,
    /// Existing elements, sequences and items to leave out.
    pub removals: HashSet<Vec<String>>,
    /// New items (encoded whole, see `item_start`) to write after the existing items
    /// of the sequence at the path.
    pub appended_items: HashMap<Vec<String>, Vec<Vec<u8>>>,
    /// New values of existing file meta information elements (e.g. Media Storage SOP
    /// Instance UID), encoded and padded. File Meta Information Group Length
    /// (0002,0000) is updated to match.
    pub meta_replacements: HashMap<Tag, Vec<u8>>,
}

/// Write `source` to `out` with `edits` applied, updating the lengths of enclosing
/// defined-length items and sequences.
pub fn write_spliced(
    source: &str,
    index: &FileIndex,
    edits: &Edits,
    out: &mut dyn Write,
) -> Result<(), String> {
    let mut file = File::open(source).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if metadata.len() != index.file_length || metadata.modified().ok() != index.modified {
        return Err("The file changed since it was opened".to_string());
    }
    if !index.deflated {
        let mut writer = SpliceWriter {
            source: &mut file,
            out,
            edits,
            encoding: index.encoding,
        };
        writer.meta(index)?;
        writer.nodes(&index.nodes, &[])?;
        return writer.copy(index.dataset_end(), index.source_length);
    }
    // A deflated file is copied as it is when nothing changes, so that saving without
    // edits keeps every byte (deflating again would give other bytes).
    if edits.is_empty() {
        let copied = std::io::copy(&mut file, out).map_err(|error| error.to_string())?;
        if copied != index.file_length {
            return Err("The file changed since it was opened".to_string());
        }
        return Ok(());
    }
    let mut inflated = Cursor::new(inflated_source(
        &mut BufReader::new(file),
        index.dataset_start,
    )?);
    if inflated.get_ref().len() as u64 != index.source_length {
        return Err("The file changed since it was opened".to_string());
    }
    let mut head = Vec::new();
    let mut dataset = Vec::new();
    {
        let mut writer = SpliceWriter {
            source: &mut inflated,
            out: &mut head,
            edits,
            encoding: index.encoding,
        };
        writer.meta(index)?;
        writer.out = &mut dataset;
        writer.nodes(&index.nodes, &[])?;
        writer.copy(index.dataset_end(), index.source_length)?;
    }
    out.write_all(&head).map_err(|error| error.to_string())?;
    out.write_all(&deflate::deflate(&dataset)?)
        .map_err(|error| error.to_string())
}

impl Edits {
    fn is_empty(&self) -> bool {
        self.replacements.is_empty()
            && self.insertions.is_empty()
            && self.removals.is_empty()
            && self.appended_items.is_empty()
            && self.meta_replacements.is_empty()
    }
}

/// An element with a header in the data set's encoding, its value (text, which needs no
/// byte swapping) padded by the caller.
pub fn encode_element(
    tag: Tag,
    vr: VR,
    value: &[u8],
    encoding: Encoding,
) -> Result<Vec<u8>, String> {
    let mut element = element_header(tag, vr.to_bytes(), value.len(), encoding)?;
    element.extend_from_slice(value);
    Ok(element)
}

impl FileIndex {
    /// Whether the file meta information has the element.
    pub fn has_meta_element(&self, tag: Tag) -> bool {
        self.meta.iter().any(|node| node.tag() == tag)
    }

    /// The file's length and modification time when it was indexed.
    pub fn stamp(&self) -> Option<FileStamp> {
        self.modified.map(|modified| (self.file_length, modified))
    }
}

impl IndexItem {
    pub fn nodes(&self) -> &[IndexNode] {
        &self.nodes
    }
}

impl FileIndex {
    /// Whether the elements of the data set and of every item are in ascending tag
    /// order, as needed to insert new elements in the right place.
    pub fn is_ordered(&self) -> bool {
        fn ordered(nodes: &[IndexNode]) -> bool {
            nodes.windows(2).all(|pair| pair[0].tag() < pair[1].tag())
                && nodes.iter().all(|node| match node {
                    IndexNode::Sequence { items, .. } => {
                        items.iter().all(|item| ordered(&item.nodes))
                    }
                    _ => true,
                })
        }
        ordered(&self.nodes)
    }
}

impl FileIndex {
    fn dataset_end(&self) -> u64 {
        self.nodes.last().map_or(self.dataset_start, node_end)
    }
}

fn node_start(node: &IndexNode) -> u64 {
    match node {
        IndexNode::Element { start, .. }
        | IndexNode::Sequence { start, .. }
        | IndexNode::Opaque { start, .. } => *start,
    }
}

fn node_end(node: &IndexNode) -> u64 {
    match node {
        IndexNode::Element { end, .. }
        | IndexNode::Sequence { end, .. }
        | IndexNode::Opaque { end, .. } => *end,
    }
}

pub fn format_tag(tag: Tag) -> String {
    format!("({:04X},{:04X})", tag.group(), tag.element())
}

fn child_path(parent: &[String], segment: String) -> Vec<String> {
    let mut path = parent.to_vec();
    path.push(segment);
    path
}

/// New elements for the data set or item at `parent`, in tag order.
fn sorted_insertions<'e>(edits: &'e Edits, parent: &[String]) -> Vec<&'e (Tag, Vec<u8>)> {
    let mut insertions = edits
        .insertions
        .get(parent)
        .map(|insertions| insertions.iter().collect::<Vec<_>>())
        .unwrap_or_default();
    insertions.sort_by_key(|(tag, _)| *tag);
    insertions
}

struct SpliceWriter<'a> {
    source: &'a mut dyn Source,
    out: &'a mut dyn Write,
    edits: &'a Edits,
    encoding: Encoding,
}

impl SpliceWriter<'_> {
    /// The preamble and the file meta information, with replaced values and the group
    /// length that follows from them.
    fn meta(&mut self, index: &FileIndex) -> Result<(), String> {
        const GROUP_LENGTH: Tag = Tag(0x0002, 0x0000);
        let replacements = &self.edits.meta_replacements;
        if replacements.is_empty() {
            return self.copy(0, index.dataset_start);
        }
        let element_length = |node: &IndexNode| -> Result<u64, String> {
            let IndexNode::Element {
                tag,
                vr_bytes,
                start,
                end,
                ..
            } = node
            else {
                return Err("Unexpected structure in the file meta information".to_string());
            };
            Ok(match replacements.get(tag) {
                Some(value) => {
                    (element_header(*tag, *vr_bytes, value.len(), Encoding::META)?.len()
                        + value.len()) as u64
                }
                None => end - start,
            })
        };
        self.copy(0, index.meta_start)?;
        for node in &index.meta {
            let IndexNode::Element {
                tag,
                vr_bytes,
                start,
                end,
                ..
            } = node
            else {
                return Err("Unexpected structure in the file meta information".to_string());
            };
            if *tag == GROUP_LENGTH {
                // An Explicit VR UL element: 8-byte header and a 4-byte value.
                if end - start != 12 {
                    return Err("Unexpected File Meta Information Group Length".to_string());
                }
                let mut total = 0_u64;
                for other in index
                    .meta
                    .iter()
                    .filter(|other| other.tag() != GROUP_LENGTH)
                {
                    total += element_length(other)?;
                }
                self.copy(*start, start + 8)?;
                self.write(&checked_length(total, "The file meta information")?.to_le_bytes())?;
            } else if let Some(value) = replacements.get(tag) {
                self.write(&element_header(
                    *tag,
                    *vr_bytes,
                    value.len(),
                    Encoding::META,
                )?)?;
                self.write(value)?;
            } else {
                self.copy(*start, *end)?;
            }
        }
        Ok(())
    }

    fn copy(&mut self, start: u64, end: u64) -> Result<(), String> {
        if end <= start {
            return Ok(());
        }
        self.source.seek_to(start)?;
        let copied = std::io::copy(&mut (&mut *self.source).take(end - start), self.out)
            .map_err(|error| error.to_string())?;
        if copied != end - start {
            return Err("The file changed since it was opened".to_string());
        }
        Ok(())
    }

    fn nodes(&mut self, nodes: &[IndexNode], parent: &[String]) -> Result<(), String> {
        let edits = self.edits;
        let insertions = sorted_insertions(edits, parent);
        let mut pending = insertions.into_iter().peekable();
        for (position, node) in nodes.iter().enumerate() {
            while let Some((_, element)) = pending.next_if(|(tag, _)| *tag < node.tag()) {
                self.write(element)?;
            }
            if self
                .edits
                .removals
                .contains(&child_path(parent, format_tag(node.tag())))
            {
                continue;
            }
            if let IndexNode::Element {
                tag, start, end, ..
            } = node
            {
                if tag.element() == 0x0000 && self.group_touched(nodes, parent, tag.group()) {
                    // Group length (gggg,0000), a UL value after an 8-byte header in
                    // either VR encoding. Others are left to the full writer by the
                    // caller (see `is_plain_group_length`).
                    if !is_plain_group_length(node) {
                        return Err(format!("Unexpected group length element {tag}"));
                    }
                    let length = self.group_length(&nodes[position + 1..], parent, tag.group())?;
                    self.copy(*start, end - 4)?;
                    self.write(&self.encoding.u32_bytes(length))?;
                    continue;
                }
            }
            self.node(node, parent)?;
        }
        for (_, element) in pending {
            self.write(element)?;
        }
        Ok(())
    }

    fn node(&mut self, node: &IndexNode, parent: &[String]) -> Result<(), String> {
        let path = child_path(parent, format_tag(node.tag()));
        if !self.touches(&path) {
            return self.copy(node_start(node), node_end(node));
        }
        match node {
            IndexNode::Element { tag, vr_bytes, .. } => {
                let value = &self.edits.replacements[&path];
                self.write(&element_header(
                    *tag,
                    *vr_bytes,
                    value.len(),
                    self.encoding,
                )?)?;
                self.write(value)
            }
            IndexNode::Sequence {
                tag,
                vr,
                start,
                header_length,
                defined_length,
                items,
                end,
            } => {
                if *defined_length {
                    let length = self.items_length(items, &path)?;
                    self.write(&long_header(*tag, *vr, length, self.encoding))?;
                } else {
                    self.copy(*start, start + header_length)?;
                }
                for (index, item) in items.iter().enumerate() {
                    let item_path = child_path(&path, format!("Item#{index}"));
                    if !self.edits.removals.contains(&item_path) {
                        self.item(item, &item_path)?;
                    }
                }
                let edits = self.edits;
                for item in edits.appended_items.get(&path).into_iter().flatten() {
                    self.write(item)?;
                }
                if !*defined_length {
                    // Sequence delimitation item.
                    self.copy(end - 8, *end)?;
                }
                Ok(())
            }
            IndexNode::Opaque { .. } => Err("Opaque elements cannot be edited".to_string()),
        }
    }

    fn item(&mut self, item: &IndexItem, path: &[String]) -> Result<(), String> {
        if !self.touches(path) {
            return self.copy(item.start, item.end);
        }
        if item.defined_length {
            let length = self.nodes_length(&item.nodes, path)?;
            self.write(&item_header(length, self.encoding))?;
        } else {
            self.copy(item.start, item.start + 8)?;
        }
        self.nodes(&item.nodes, path)?;
        if !item.defined_length {
            self.copy(item.end - 8, item.end)?;
        }
        Ok(())
    }

    /// Whether an element of `group` (other than its group length) in the data set or
    /// item at `parent` changes, is inserted or is removed.
    fn group_touched(&self, nodes: &[IndexNode], parent: &[String], group: u16) -> bool {
        nodes.iter().any(|node| {
            let tag = node.tag();
            tag.group() == group
                && tag.element() != 0x0000
                && self.touches(&child_path(parent, format_tag(tag)))
        }) || sorted_insertions(self.edits, parent)
            .iter()
            .any(|(tag, _)| tag.group() == group)
    }

    /// The length of the elements of `group` that follow its group length in `nodes`,
    /// up to the first element of another group, and of the ones inserted, as written.
    fn group_length(
        &self,
        nodes: &[IndexNode],
        parent: &[String],
        group: u16,
    ) -> Result<u32, String> {
        let mut total = 0_u64;
        // The group runs until an element of another group (files with tags out of
        // order can have more of the group later; they are not part of it).
        for node in nodes.iter().take_while(|node| node.tag().group() == group) {
            let tag = node.tag();
            if tag.element() == 0x0000
                || self
                    .edits
                    .removals
                    .contains(&child_path(parent, format_tag(tag)))
            {
                continue;
            }
            total += u64::from(self.node_length(node, parent)?);
        }
        for (tag, element) in sorted_insertions(self.edits, parent) {
            if tag.group() == group {
                total += element.len() as u64;
            }
        }
        checked_length(total, "A group")
    }

    /// Whether anything at or below `path` is replaced, inserted or removed.
    fn touches(&self, path: &[String]) -> bool {
        let below = |key: &Vec<String>| key.len() >= path.len() && key[..path.len()] == *path;
        self.edits.replacements.keys().any(below)
            || self.edits.removals.iter().any(below)
            || self.edits.insertions.keys().any(below)
            || self.edits.appended_items.keys().any(below)
    }

    fn nodes_length(&self, nodes: &[IndexNode], parent: &[String]) -> Result<u32, String> {
        let mut total: u64 = 0;
        for node in nodes {
            let path = child_path(parent, format_tag(node.tag()));
            if self.edits.removals.contains(&path) {
                continue;
            }
            total += u64::from(self.node_length(node, parent)?);
        }
        for (_, element) in sorted_insertions(self.edits, parent) {
            total += element.len() as u64;
        }
        checked_length(total, "An item")
    }

    fn node_length(&self, node: &IndexNode, parent: &[String]) -> Result<u32, String> {
        let path = child_path(parent, format_tag(node.tag()));
        let original = || {
            u32::try_from(node_end(node) - node_start(node))
                .map_err(|_| "An element is too long".to_string())
        };
        if !self.touches(&path) {
            return original();
        }
        match node {
            IndexNode::Element { tag, vr_bytes, .. } => {
                let value = &self.edits.replacements[&path];
                let header = element_header(*tag, *vr_bytes, value.len(), self.encoding)?;
                u32::try_from(header.len() + value.len())
                    .map_err(|_| "An element is too long".to_string())
            }
            IndexNode::Sequence {
                header_length,
                defined_length,
                items,
                ..
            } => {
                let total = header_length
                    + if *defined_length { 0 } else { 8 }
                    + u64::from(self.items_length(items, &path)?);
                checked_length(total, "A sequence")
            }
            IndexNode::Opaque { .. } => original(),
        }
    }

    fn items_length(&self, items: &[IndexItem], path: &[String]) -> Result<u32, String> {
        let mut total = 0_u64;
        for (index, item) in items.iter().enumerate() {
            let item_path = child_path(path, format!("Item#{index}"));
            if self.edits.removals.contains(&item_path) {
                continue;
            }
            let content = u64::from(self.nodes_length(&item.nodes, &item_path)?);
            total += 8 + content + if item.defined_length { 0 } else { 8 };
        }
        for item in self.edits.appended_items.get(path).into_iter().flatten() {
            total += item.len() as u64;
        }
        checked_length(total, "A sequence")
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.out.write_all(bytes).map_err(|error| error.to_string())
    }
}

/// The start of a new sequence, of undefined length (closed by `sequence_end`).
pub fn sequence_start(tag: Tag, encoding: Encoding) -> Vec<u8> {
    long_header(tag, *b"SQ", UNDEFINED, encoding)
}

/// The sequence delimitation item.
pub fn sequence_end(encoding: Encoding) -> Vec<u8> {
    delimiter(SEQUENCE_DELIMITER, encoding)
}

/// The start of a new item, of undefined length (closed by `item_end`).
pub fn item_start(encoding: Encoding) -> Vec<u8> {
    item_header(UNDEFINED, encoding)
}

/// The item delimitation item.
pub fn item_end(encoding: Encoding) -> Vec<u8> {
    delimiter(ITEM_DELIMITER, encoding)
}

fn delimiter(tag: Tag, encoding: Encoding) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&encoding.tag_bytes(tag));
    out.extend_from_slice(&encoding.u32_bytes(0));
    out
}

/// Whether a group length element (gggg,0000) is a 4-byte UL value, which splicing
/// can update in place.
pub fn is_plain_group_length(node: &IndexNode) -> bool {
    matches!(node, IndexNode::Element { tag, vr: Some(VR::UL), start, end, .. }
        if tag.element() == 0x0000 && end - start == 12)
}

/// A defined length, which cannot be 0xFFFFFFFF (reserved for undefined length).
fn checked_length(total: u64, what: &str) -> Result<u32, String> {
    u32::try_from(total)
        .ok()
        .filter(|length| *length != UNDEFINED)
        .ok_or_else(|| format!("{what} became too long"))
}

/// A file's length and modification time, to tell whether it changed.
pub type FileStamp = (u64, std::time::SystemTime);

/// The length and modification time of a file, to tell whether it changed.
pub fn file_stamp(path: &str) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

fn element_header(
    tag: Tag,
    vr_bytes: [u8; 2],
    length: usize,
    encoding: Encoding,
) -> Result<Vec<u8>, String> {
    let mut header = Vec::with_capacity(12);
    header.extend_from_slice(&encoding.tag_bytes(tag));
    if !encoding.explicit {
        // Implicit VR: no VR, 32-bit length.
        let length = u32::try_from(length)
            .ok()
            .filter(|length| *length != UNDEFINED)
            .ok_or_else(|| format!("The value of {} is too long", format_tag(tag)))?;
        header.extend_from_slice(&encoding.u32_bytes(length));
        return Ok(header);
    }
    header.extend_from_slice(&vr_bytes);
    if has_32_bit_length(&vr_bytes) {
        let length = u32::try_from(length)
            .ok()
            .filter(|length| *length != UNDEFINED)
            .ok_or_else(|| format!("The value of {} is too long", format_tag(tag)))?;
        header.extend_from_slice(&[0, 0]);
        header.extend_from_slice(&encoding.u32_bytes(length));
    } else {
        let length = u16::try_from(length).map_err(|_| {
            format!(
                "The value of {} is {length} bytes, longer than the 65,534 bytes its VR allows",
                format_tag(tag)
            )
        })?;
        header.extend_from_slice(&encoding.u16_bytes(length));
    }
    Ok(header)
}

fn long_header(tag: Tag, vr: [u8; 2], length: u32, encoding: Encoding) -> Vec<u8> {
    let mut header = Vec::with_capacity(12);
    header.extend_from_slice(&encoding.tag_bytes(tag));
    if encoding.explicit {
        header.extend_from_slice(&vr);
        header.extend_from_slice(&[0, 0]);
    }
    header.extend_from_slice(&encoding.u32_bytes(length));
    header
}

fn item_header(length: u32, encoding: Encoding) -> Vec<u8> {
    let mut header = Vec::with_capacity(8);
    header.extend_from_slice(&encoding.tag_bytes(ITEM));
    header.extend_from_slice(&encoding.u32_bytes(length));
    header
}
