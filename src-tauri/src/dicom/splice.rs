//! Saving by copying the original file and rewriting only edited values (#8).
//!
//! The file is indexed once: where each element's header and value are, and how
//! sequences and items nest. Saving then streams the original bytes and replaces only
//! the values that were edited, along with the length fields that change with them
//! (the element's own and those of enclosing defined-length items and sequences).
//! Everything else, including text in character sets dicom-rs cannot write and the
//! pixel data, is copied byte for byte.
//!
//! Supported for now: Part 10 files whose data set is Explicit VR Little Endian
//! (including encapsulated pixel data), and edits that only replace existing values.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

use dicom_core::{Tag, VR};
use dicom_dictionary_std::uids;
use std::str::FromStr;

const ITEM: Tag = Tag(0xFFFE, 0xE000);
const ITEM_DELIMITER: Tag = Tag(0xFFFE, 0xE00D);
const SEQUENCE_DELIMITER: Tag = Tag(0xFFFE, 0xE0DD);
const UNDEFINED: u32 = u32::MAX;

/// Transfer syntaxes whose data set is encoded as Explicit VR Little Endian.
pub fn supports_transfer_syntax(uid: &str) -> bool {
    let uid = uid.trim_end_matches(['\0', ' ']);
    // Standard transfer syntaxes other than Implicit VR Little Endian (1.2.840.10008.1.2)
    // all live under 1.2.840.10008.1.2. and encode the data set as Explicit VR Little
    // Endian, except for Big Endian and the deflated ones.
    uid.starts_with("1.2.840.10008.1.2.")
        && !matches!(
            uid,
            "1.2.840.10008.1.2.2" // Explicit VR Big Endian (retired)
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
    /// Offset of the first data set element (after the file meta information).
    dataset_start: u64,
    file_length: u64,
    nodes: Vec<IndexNode>,
}

impl FileIndex {
    pub fn read(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        let file_length = metadata.len();
        let modified = metadata.modified().ok();
        let mut reader = Reader {
            inner: BufReader::new(file),
            position: 0,
            length: file_length,
        };
        let mut head = [0_u8; 132];
        reader.read_exact(&mut head)?;
        if &head[128..132] != b"DICM" {
            return Err("Not a DICOM Part 10 file with a preamble".to_string());
        }
        // File meta information (group 0002) is Explicit VR Little Endian too.
        let meta = read_nodes(&mut reader, file_length, |tag| tag.group() != 0x0002)?;
        if meta
            .iter()
            .any(|node| !matches!(node, IndexNode::Element { .. }))
        {
            return Err("Unexpected structure in the file meta information".to_string());
        }
        let dataset_start = reader.position;
        let nodes = read_nodes(&mut reader, file_length, |_| false)?;
        Ok(Self {
            modified,
            dataset_start,
            file_length,
            nodes,
        })
    }

    pub fn nodes(&self) -> &[IndexNode] {
        &self.nodes
    }
}

struct Reader {
    inner: BufReader<File>,
    position: u64,
    length: u64,
}

impl Reader {
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
        self.inner
            .seek_relative(length as i64)
            .map_err(|error| error.to_string())?;
        self.position += length;
        Ok(())
    }

    fn tag_and_length(&mut self) -> Result<(Tag, [u8; 2], u32, bool), String> {
        let mut header = [0_u8; 8];
        self.read_exact(&mut header)?;
        let tag = Tag(
            u16::from_le_bytes([header[0], header[1]]),
            u16::from_le_bytes([header[2], header[3]]),
        );
        if tag.group() == 0xFFFE {
            // Item and delimiters: no VR, 32-bit length.
            let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            return Ok((tag, [0, 0], length, false));
        }
        let vr = [header[4], header[5]];
        if has_32_bit_length(&vr) {
            let mut length = [0_u8; 4];
            self.read_exact(&mut length)?;
            Ok((tag, vr, u32::from_le_bytes(length), true))
        } else {
            Ok((
                tag,
                vr,
                u32::from(u16::from_le_bytes([header[6], header[7]])),
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
        self.inner
            .seek_relative(-4)
            .map_err(|error| error.to_string())?;
        Ok(Some(Tag(
            u16::from_le_bytes([buffer[0], buffer[1]]),
            u16::from_le_bytes([buffer[2], buffer[3]]),
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
    let mut nodes = Vec::new();
    while reader.position < end {
        let Some(tag) = reader.peek_tag()? else {
            break;
        };
        if stop(tag) || tag == ITEM_DELIMITER {
            break;
        }
        nodes.push(read_node(reader)?);
    }
    Ok(nodes)
}

fn read_node(reader: &mut Reader) -> Result<IndexNode, String> {
    let start = reader.position;
    let (tag, vr, length, _) = reader.tag_and_length()?;
    if tag.group() == 0xFFFE {
        return Err(format!("Unexpected item tag at {start}"));
    }
    if &vr == b"SQ" || (length == UNDEFINED && &vr == b"UN") {
        if &vr == b"UN" {
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
            vr,
            start,
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
    let known_vr = std::str::from_utf8(&vr)
        .ok()
        .and_then(|text| VR::from_str(text).ok());
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
    /// Existing plain elements to leave out.
    pub removals: HashSet<Vec<String>>,
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
    let mut writer = SpliceWriter {
        source: &mut file,
        out,
        edits,
    };
    writer.copy(0, index.dataset_start)?;
    writer.nodes(&index.nodes, &[])?;
    writer.copy(index.dataset_end(), index.file_length)
}

/// An element with an Explicit VR Little Endian header, its value padded by the caller.
pub fn encode_element(tag: Tag, vr: VR, value: &[u8]) -> Result<Vec<u8>, String> {
    let mut element = element_header(tag, vr.to_bytes(), value.len())?;
    element.extend_from_slice(value);
    Ok(element)
}

impl FileIndex {
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
    source: &'a mut File,
    out: &'a mut dyn Write,
    edits: &'a Edits,
}

impl SpliceWriter<'_> {
    fn copy(&mut self, start: u64, end: u64) -> Result<(), String> {
        if end <= start {
            return Ok(());
        }
        self.source
            .seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
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
        for node in nodes {
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
                self.write(&element_header(*tag, *vr_bytes, value.len())?)?;
                self.write(value)
            }
            IndexNode::Sequence {
                tag,
                vr,
                start,
                defined_length,
                items,
                end,
                ..
            } => {
                let header_length = 12;
                if *defined_length {
                    let length = self.items_length(items, &path)?;
                    self.write(&long_header(*tag, *vr, length))?;
                } else {
                    self.copy(*start, start + header_length)?;
                }
                for (index, item) in items.iter().enumerate() {
                    self.item(item, &child_path(&path, format!("Item#{index}")))?;
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
            self.write(&item_header(length))?;
        } else {
            self.copy(item.start, item.start + 8)?;
        }
        self.nodes(&item.nodes, path)?;
        if !item.defined_length {
            self.copy(item.end - 8, item.end)?;
        }
        Ok(())
    }

    /// Whether anything at or below `path` is replaced, inserted or removed.
    fn touches(&self, path: &[String]) -> bool {
        let below = |key: &Vec<String>| key.len() >= path.len() && key[..path.len()] == *path;
        self.edits.replacements.keys().any(below)
            || self.edits.removals.iter().any(below)
            || self.edits.insertions.keys().any(below)
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
                let header = element_header(*tag, *vr_bytes, value.len())?;
                u32::try_from(header.len() + value.len())
                    .map_err(|_| "An element is too long".to_string())
            }
            IndexNode::Sequence {
                defined_length,
                items,
                ..
            } => {
                let mut total = 12_u64 + if *defined_length { 0 } else { 8 };
                for (index, item) in items.iter().enumerate() {
                    let item_path = child_path(&path, format!("Item#{index}"));
                    let content = u64::from(self.nodes_length(&item.nodes, &item_path)?);
                    total += 8 + content + if item.defined_length { 0 } else { 8 };
                }
                checked_length(total, "A sequence")
            }
            IndexNode::Opaque { .. } => original(),
        }
    }

    fn items_length(&self, items: &[IndexItem], path: &[String]) -> Result<u32, String> {
        let mut total = 0_u64;
        for (index, item) in items.iter().enumerate() {
            let item_path = child_path(path, format!("Item#{index}"));
            let content = u64::from(self.nodes_length(&item.nodes, &item_path)?);
            total += 8 + content + if item.defined_length { 0 } else { 8 };
        }
        checked_length(total, "A sequence")
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.out.write_all(bytes).map_err(|error| error.to_string())
    }
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

fn element_header(tag: Tag, vr_bytes: [u8; 2], length: usize) -> Result<Vec<u8>, String> {
    let mut header = Vec::with_capacity(12);
    header.extend_from_slice(&tag.group().to_le_bytes());
    header.extend_from_slice(&tag.element().to_le_bytes());
    header.extend_from_slice(&vr_bytes);
    if has_32_bit_length(&vr_bytes) {
        let length = u32::try_from(length)
            .ok()
            .filter(|length| *length != UNDEFINED)
            .ok_or_else(|| format!("The value of {} is too long", format_tag(tag)))?;
        header.extend_from_slice(&[0, 0]);
        header.extend_from_slice(&length.to_le_bytes());
    } else {
        let length = u16::try_from(length).map_err(|_| {
            format!(
                "The value of {} is {length} bytes, longer than the 65,534 bytes its VR allows",
                format_tag(tag)
            )
        })?;
        header.extend_from_slice(&length.to_le_bytes());
    }
    Ok(header)
}

fn long_header(tag: Tag, vr: [u8; 2], length: u32) -> Vec<u8> {
    let mut header = Vec::with_capacity(12);
    header.extend_from_slice(&tag.group().to_le_bytes());
    header.extend_from_slice(&tag.element().to_le_bytes());
    header.extend_from_slice(&vr);
    header.extend_from_slice(&[0, 0]);
    header.extend_from_slice(&length.to_le_bytes());
    header
}

fn item_header(length: u32) -> Vec<u8> {
    let mut header = Vec::with_capacity(8);
    header.extend_from_slice(&ITEM.group().to_le_bytes());
    header.extend_from_slice(&ITEM.element().to_le_bytes());
    header.extend_from_slice(&length.to_le_bytes());
    header
}
