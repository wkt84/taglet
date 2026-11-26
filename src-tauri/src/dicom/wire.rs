//! The form in which `open_dicom_file` sends a file's nodes to the page (#20).
//!
//! As JSON objects, the nodes of a file with many short values (an RT Structure Set
//! with thousands of short contours) were mostly not values: each element carried its
//! path, description and field names, and sending them took most of the time to open
//! the file. Here each node is an array, and what the page can rebuild is left out:
//!
//! - Tags, VRs and descriptions are sent once, in `entries`, and nodes refer to them by
//!   index.
//! - A node's path is left out when it is its parent's path and its tag (an item's
//!   elements: the sequence's path, `Item#<index>` and their tag; the file meta
//!   information's: `FileMetaInformation` and their tag), and sent otherwise.
//! - A preview's key is left out when it is its path joined with `/` (as `previews`
//!   makes them), and sent otherwise.
//!
//! The page rebuilds the nodes as they are (`src/data/openedFile.ts`): the JSON of the
//! nodes it rebuilds is the JSON of these nodes, which `tests/fixtures/open-wire.json`
//! checks on both sides. What the page sends back to save is unchanged.
//!
//! - Element: `[0, entry, value, length, flags, extra?]`
//! - Sequence: `[1, entry, length, items, extra?]`
//!
//! `flags` is 1 for an editable element, plus 2 for a preview whose key is left out.
//! `extra` is an object with what cannot be left out, and only when there is any:
//! `path`, `inferred_vr`, `full_value_key`, `item_sources`.

use std::collections::HashMap;

use serde::ser::{SerializeMap, SerializeSeq, Serializer};
use serde::Serialize;

use super::model::{DicomFileContent, DicomNode};

/// The first part of the paths of the file meta information's elements.
pub const FILE_META_PATH: &str = "FileMetaInformation";

const ELEMENT: u8 = 0;
const SEQUENCE: u8 = 1;
const EDITABLE: u8 = 1;
const PREVIEW: u8 = 2;

/// A file's content, serialized in the form described above.
pub struct WireFileContent(pub DicomFileContent);

/// A tag, VR (empty for a sequence) and description.
type Entry<'a> = (&'a str, &'a str, &'a str);

#[derive(Default)]
struct Entries<'a> {
    index: HashMap<Entry<'a>, u32>,
    list: Vec<Entry<'a>>,
}

impl<'a> Entries<'a> {
    fn collect(&mut self, nodes: &'a [DicomNode]) {
        for node in nodes {
            let entry = entry_of(node);
            if !self.index.contains_key(&entry) {
                self.index.insert(entry, self.list.len() as u32);
                self.list.push(entry);
            }
            if let DicomNode::Sequence { items, .. } = node {
                for item in items {
                    self.collect(item);
                }
            }
        }
    }
}

fn entry_of(node: &DicomNode) -> Entry<'_> {
    match node {
        DicomNode::Element {
            tag,
            vr,
            description,
            ..
        } => (tag, vr, description),
        DicomNode::Sequence {
            tag, description, ..
        } => (tag, "", description),
    }
}

/// Where the nodes of a list are: their paths are `base`, then `item` (for the
/// elements of an item), then their tag.
#[derive(Clone, Copy)]
struct Parent<'a> {
    base: &'a [String],
    item: Option<&'a str>,
}

impl Parent<'_> {
    fn path_is_derived(&self, path: &[String], tag: &str) -> bool {
        let base = self.base.len();
        let item = usize::from(self.item.is_some());
        path.len() == base + item + 1
            && path[..base] == *self.base
            && self.item.is_none_or(|segment| path[base] == segment)
            && path[base + item] == tag
    }
}

/// Whether `key` is the parts of `path` joined with `/`.
fn is_joined_path(path: &[String], key: &str) -> bool {
    let mut rest = key;
    for (index, part) in path.iter().enumerate() {
        if index > 0 {
            let Some(after) = rest.strip_prefix('/') else {
                return false;
            };
            rest = after;
        }
        let Some(after) = rest.strip_prefix(part.as_str()) else {
            return false;
        };
        rest = after;
    }
    rest.is_empty()
}

struct WireNodes<'a> {
    nodes: &'a [DicomNode],
    parent: Parent<'a>,
    entries: &'a Entries<'a>,
}

impl Serialize for WireNodes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.nodes.len()))?;
        for node in self.nodes {
            seq.serialize_element(&WireNode {
                node,
                parent: self.parent,
                entries: self.entries,
            })?;
        }
        seq.end()
    }
}

struct WireNode<'a> {
    node: &'a DicomNode,
    parent: Parent<'a>,
    entries: &'a Entries<'a>,
}

/// What a node sends besides its fixed fields.
#[derive(Serialize, Default)]
struct Extra<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inferred_vr: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_value_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_sources: Option<&'a [Option<usize>]>,
}

impl Extra<'_> {
    fn is_empty(&self) -> bool {
        self.path.is_none()
            && self.inferred_vr.is_none()
            && self.full_value_key.is_none()
            && self.item_sources.is_none()
    }
}

impl Serialize for WireNode<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let entry = self.entries.index[&entry_of(self.node)];
        match self.node {
            DicomNode::Element {
                tag,
                inferred_vr,
                value,
                length,
                path,
                editable,
                full_value_key,
                ..
            } => {
                let derived_key = full_value_key
                    .as_deref()
                    .is_some_and(|key| is_joined_path(path, key));
                let extra = Extra {
                    path: (!self.parent.path_is_derived(path, tag)).then_some(path.as_slice()),
                    inferred_vr: inferred_vr.as_deref(),
                    full_value_key: full_value_key.as_deref().filter(|_| !derived_key),
                    item_sources: None,
                };
                let flags =
                    if *editable { EDITABLE } else { 0 } | if derived_key { PREVIEW } else { 0 };
                let mut seq = serializer.serialize_seq(None)?;
                seq.serialize_element(&ELEMENT)?;
                seq.serialize_element(&entry)?;
                seq.serialize_element(value)?;
                seq.serialize_element(length)?;
                seq.serialize_element(&flags)?;
                if !extra.is_empty() {
                    seq.serialize_element(&extra)?;
                }
                seq.end()
            }
            DicomNode::Sequence {
                tag,
                length,
                path,
                items,
                item_sources,
                ..
            } => {
                let extra = Extra {
                    path: (!self.parent.path_is_derived(path, tag)).then_some(path.as_slice()),
                    item_sources: item_sources.as_deref(),
                    ..Extra::default()
                };
                let mut seq = serializer.serialize_seq(None)?;
                seq.serialize_element(&SEQUENCE)?;
                seq.serialize_element(&entry)?;
                seq.serialize_element(length)?;
                seq.serialize_element(&WireItems {
                    items,
                    path,
                    entries: self.entries,
                })?;
                if !extra.is_empty() {
                    seq.serialize_element(&extra)?;
                }
                seq.end()
            }
        }
    }
}

struct WireItems<'a> {
    items: &'a [Vec<DicomNode>],
    /// The sequence's path.
    path: &'a [String],
    entries: &'a Entries<'a>,
}

impl Serialize for WireItems<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.items.len()))?;
        for (index, item) in self.items.iter().enumerate() {
            let segment = format!("Item#{index}");
            seq.serialize_element(&WireNodes {
                nodes: item,
                parent: Parent {
                    base: self.path,
                    item: Some(&segment),
                },
                entries: self.entries,
            })?;
        }
        seq.end()
    }
}

impl Serialize for WireFileContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let content = &self.0;
        let mut entries = Entries::default();
        entries.collect(&content.file_meta);
        entries.collect(&content.nodes);
        let file_meta_base = [FILE_META_PATH.to_string()];

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("entries", &entries.list)?;
        map.serialize_entry(
            "file_meta",
            &WireNodes {
                nodes: &content.file_meta,
                parent: Parent {
                    base: &file_meta_base,
                    item: None,
                },
                entries: &entries,
            },
        )?;
        map.serialize_entry(
            "nodes",
            &WireNodes {
                nodes: &content.nodes,
                parent: Parent {
                    base: &[],
                    item: None,
                },
                entries: &entries,
            },
        )?;
        map.serialize_entry("warnings", &content.warnings)?;
        if let Some(open_ms) = content.open_ms {
            map.serialize_entry("open_ms", &open_ms)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn element(path: &[&str], vr: &str, value: &str) -> DicomNode {
        DicomNode::Element {
            tag: path.last().unwrap().to_string(),
            vr: vr.to_string(),
            inferred_vr: None,
            description: format!("Description of {}", path.last().unwrap()),
            value: value.to_string(),
            length: value.len() as u32,
            path: strings(path),
            editable: true,
            full_value_key: None,
        }
    }

    fn sequence(path: &[&str], items: Vec<Vec<DicomNode>>) -> DicomNode {
        DicomNode::Sequence {
            tag: path.last().unwrap().to_string(),
            description: format!("Description of {}", path.last().unwrap()),
            length: u32::MAX,
            path: strings(path),
            items,
            item_sources: None,
        }
    }

    fn with(mut node: DicomNode, change: impl FnOnce(&mut DicomNode)) -> DicomNode {
        change(&mut node);
        node
    }

    /// Nodes of every shape the page must rebuild: derived and other paths, derived and
    /// other preview keys, inferred VRs, read-only elements, nested, empty and changed
    /// sequences, repeated and multi-byte text.
    fn every_shape() -> DicomFileContent {
        let seq = "(3006,0039)";
        let contours = "(3006,0040)";
        let item = |index: usize| format!("Item#{index}");
        let contour = |roi: usize, index: usize, value: &str| {
            let path = [seq, &item(roi), contours, &item(index), "(3006,0050)"];
            with(element(&path, "DS", value), |node| {
                if let DicomNode::Element {
                    full_value_key,
                    path,
                    ..
                } = node
                {
                    *full_value_key = Some(path.join("/"));
                }
            })
        };
        DicomFileContent {
            file_meta: vec![
                with(
                    element(
                        &[FILE_META_PATH, "(0002,0010)"],
                        "UI",
                        "1.2.840.10008.1.2.1",
                    ),
                    |node| {
                        if let DicomNode::Element { editable, .. } = node {
                            *editable = false;
                        }
                    },
                ),
                element(&["(0002,0013)"], "SH", "path not under the file meta"),
            ],
            nodes: vec![
                element(&["(0008,0005)"], "CS", "ISO_IR 192"),
                element(&["(0010,0010)"], "PN", "山田^太郎"),
                with(element(&["(0019,1001)"], "UN", "1\\2"), |node| {
                    if let DicomNode::Element { inferred_vr, .. } = node {
                        *inferred_vr = Some("LO".to_string());
                    }
                }),
                with(element(&["(0008,1030)"], "LO", "preview"), |node| {
                    if let DicomNode::Element { full_value_key, .. } = node {
                        *full_value_key = Some("not/the/path".to_string());
                    }
                }),
                element(&["(0008,103E)", "unexpected"], "LO", "path not derived"),
                sequence(
                    &[seq],
                    vec![
                        vec![
                            element(&[seq, &item(0), "(3006,0084)"], "IS", "1"),
                            sequence(
                                &[seq, &item(0), contours],
                                vec![
                                    vec![
                                        element(
                                            &[seq, &item(0), contours, &item(0), "(3006,0042)"],
                                            "CS",
                                            "CLOSED_PLANAR",
                                        ),
                                        contour(0, 0, "1.5\\2.5\\3.5"),
                                    ],
                                    vec![contour(0, 1, "4.5\\5.5\\6.5")],
                                ],
                            ),
                        ],
                        vec![],
                        vec![
                            element(&[seq, &item(2), "(3006,0084)"], "IS", "3"),
                            sequence(&[seq, &item(2), contours], vec![]),
                            // In an item, but not where its position says.
                            element(&[seq, &item(1), "(3006,0085)"], "SH", "moved"),
                        ],
                    ],
                ),
                with(
                    sequence(
                        &["(300A,00B0)"],
                        vec![
                            vec![element(
                                &["(300A,00B0)", "Item#0", "(300A,00C2)"],
                                "LO",
                                "kept",
                            )],
                            vec![element(
                                &["(300A,00B0)", "Item#1", "(300A,00C2)"],
                                "LO",
                                "added",
                            )],
                        ],
                    ),
                    |node| {
                        if let DicomNode::Sequence { item_sources, .. } = node {
                            *item_sources = Some(vec![Some(2), None]);
                        }
                    },
                ),
                element(&["(7FE0,0010)"], "OW", "[Binary Data]"),
            ],
            warnings: vec!["A warning".to_string()],
            open_ms: Some(12.5),
        }
    }

    fn fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/open-wire.json")
    }

    /// The cases the page's decoder is tested with (`tests/openedFile.test.mjs`): each
    /// has the nodes as sent and as JSON objects, which the page must rebuild exactly.
    fn fixture() -> serde_json::Value {
        let mut without_previews = every_shape();
        without_previews.open_ms = None;
        without_previews.warnings.clear();
        let cases = [
            ("every shape", every_shape()),
            ("no timing or warnings", without_previews),
        ]
        .into_iter()
        .map(|(name, content)| {
            serde_json::json!({
                "name": name,
                "wire": serde_json::to_value(WireFileContent(content.clone())).unwrap(),
                "nodes": serde_json::to_value(&content).unwrap(),
            })
        })
        .collect::<Vec<_>>();
        serde_json::json!({
            "about": "Written by `cargo test wire` with TAGLET_UPDATE_FIXTURES=1 (src-tauri/src/dicom/wire.rs); read by tests/openedFile.test.mjs.",
            "cases": cases,
        })
    }

    /// The fixture the page's decoder is tested with is what this side sends. Compared as
    /// JSON values (not text, which a checkout may change the line endings of).
    #[test]
    fn the_fixture_for_the_page_is_up_to_date() {
        let expected = fixture();
        if std::env::var_os("TAGLET_UPDATE_FIXTURES").is_some() {
            let text = serde_json::to_string_pretty(&expected).unwrap() + "\n";
            std::fs::write(fixture_path(), text).unwrap();
        }
        let committed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(fixture_path()).unwrap()).unwrap();
        assert!(
            committed == expected,
            "tests/fixtures/open-wire.json is out of date: run `TAGLET_UPDATE_FIXTURES=1 cargo test wire` and commit it"
        );
    }

    /// Paths and keys are left out where the page can rebuild them, and sent otherwise.
    #[test]
    fn only_paths_and_keys_that_cannot_be_rebuilt_are_sent() {
        let wire = serde_json::to_value(WireFileContent(every_shape())).unwrap();
        let text = wire.to_string();
        let nodes = &wire["nodes"];
        // The derived ones are not sent (the one `Item#` is the moved element's).
        assert_eq!(text.matches("Item#").count(), 1);
        assert!(!text.contains("(3006,0039)/"));
        // The others are.
        assert_eq!(wire["file_meta"][0].as_array().unwrap().len(), 5);
        assert_eq!(
            wire["file_meta"][1][5]["path"],
            serde_json::json!(["(0002,0013)"])
        );
        assert_eq!(nodes[2][5], serde_json::json!({ "inferred_vr": "LO" }));
        assert_eq!(
            nodes[3][5],
            serde_json::json!({ "full_value_key": "not/the/path" })
        );
        assert_eq!(
            nodes[4][5]["path"],
            serde_json::json!(["(0008,103E)", "unexpected"])
        );
        assert_eq!(nodes[5][3][2][2][5]["path"][1], "Item#1");
        assert_eq!(
            nodes[6][4],
            serde_json::json!({ "item_sources": [2, null] })
        );
        // A derived key is a flag.
        let contour = &nodes[5][3][0][1][3][0][1];
        assert_eq!(contour[4], EDITABLE | PREVIEW);
        assert_eq!(contour.as_array().unwrap().len(), 5);
        // Entries are sent once.
        let entries = wire["entries"].as_array().unwrap();
        let unique = entries.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), entries.len());
    }

    #[test]
    fn joined_paths() {
        let path = strings(&["(3006,0039)", "Item#0", "(3006,0050)"]);
        assert!(is_joined_path(&path, "(3006,0039)/Item#0/(3006,0050)"));
        assert!(!is_joined_path(&path, "(3006,0039)/Item#0/(3006,0050)/"));
        assert!(!is_joined_path(&path, "(3006,0039)/Item#0"));
        assert!(!is_joined_path(&path, "(3006,0039)Item#0/(3006,0050)"));
        assert!(is_joined_path(&[], ""));
    }
}
