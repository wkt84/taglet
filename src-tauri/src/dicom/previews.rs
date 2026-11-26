//! Sending only the start of long values to the page (#20).
//!
//! Opening a large file (e.g. an RT Structure Set, with thousands of Contour Data
//! values) mostly took sending its values to the page: the JSON was as large as the
//! file. Long values are therefore sent as a preview (their start) with a key, and the
//! backend keeps the whole values of the open file by key. The page asks for a whole
//! value when it needs one (to show, edit or copy it), and saving puts the whole values
//! back first.
//!
//! Saving must never write a preview instead of its value. A node with a key must still
//! hold exactly the preview it was sent with; the page drops the key when it changes
//! the value (having fetched the whole value to edit it). Anything else fails the save.

use std::collections::HashMap;

use super::model::DicomNode;

/// Whether long values are sent as previews.
pub const SEND_PREVIEWS: bool = true;

/// Values longer than this many characters are sent as previews: also the Contour Data
/// of short contours (a few hundred characters), which RT Structure Sets can have by
/// the thousand.
pub const PREVIEW_ABOVE_CHARS: usize = 256;

/// How many characters a preview has: more than the page shows in the table (120), so
/// that previews look the same there.
pub const PREVIEW_CHARS: usize = 128;

/// The whole values of long elements of an open file, by key.
pub type FullValues = HashMap<String, String>;

/// The preview of a whole value: its first `PREVIEW_CHARS` characters.
pub fn preview_of(value: &str) -> String {
    value.chars().take(PREVIEW_CHARS).collect()
}

/// Replace the values longer than `PREVIEW_ABOVE_CHARS` characters with their previews,
/// keeping the whole values in `full_values` by key (the element's path as read, which
/// stays unique while items are renumbered on the page).
pub fn take_previews(nodes: &mut [DicomNode], full_values: &mut FullValues) {
    for node in nodes {
        match node {
            DicomNode::Element {
                value,
                path,
                full_value_key,
                ..
            } => {
                if full_value_key.is_some() || value.chars().count() <= PREVIEW_ABOVE_CHARS {
                    continue;
                }
                let key = path.join("/");
                let preview = preview_of(value);
                full_values.insert(key.clone(), std::mem::replace(value, preview));
                *full_value_key = Some(key);
            }
            DicomNode::Sequence { items, .. } => {
                for item in items {
                    take_previews(item, full_values);
                }
            }
        }
    }
}

/// Put the whole values back in place of previews, for saving, in place (the nodes of
/// a large file are not copied). Fails, so that the save does not happen, when a node
/// with a key holds anything but the preview of the value kept under it, or the key is
/// unknown.
pub fn restore_full_values(
    nodes: &mut [DicomNode],
    full_values: &FullValues,
) -> Result<(), String> {
    for node in nodes {
        match node {
            DicomNode::Element {
                value,
                path,
                full_value_key,
                ..
            } => {
                let Some(key) = full_value_key.as_ref() else {
                    continue;
                };
                let shown = path.join(" / ");
                let full = full_values.get(key).ok_or_else(|| {
                    format!("Cannot save: the whole value of {shown} is no longer available. Open the file again.")
                })?;
                if *value != preview_of(full) {
                    return Err(format!(
                        "Cannot save: {shown} holds only the start of its value, which was changed. Open the file again and edit the whole value."
                    ));
                }
                value.clone_from(full);
                *full_value_key = None;
            }
            DicomNode::Sequence { items, .. } => {
                for item in items {
                    restore_full_values(item, full_values)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether any node holds a preview.
    fn has_previews(nodes: &[DicomNode]) -> bool {
        nodes.iter().any(|node| match node {
            DicomNode::Element { full_value_key, .. } => full_value_key.is_some(),
            DicomNode::Sequence { items, .. } => items.iter().any(|item| has_previews(item)),
        })
    }

    fn element(path: &[&str], value: &str) -> DicomNode {
        DicomNode::Element {
            tag: path.last().unwrap().to_string(),
            vr: "DS".to_string(),
            inferred_vr: None,
            description: String::new(),
            value: value.to_string(),
            length: 0,
            path: path.iter().map(|part| part.to_string()).collect(),
            editable: true,
            full_value_key: None,
        }
    }

    fn sequence(tag: &str, items: Vec<Vec<DicomNode>>) -> DicomNode {
        DicomNode::Sequence {
            tag: tag.to_string(),
            description: String::new(),
            length: 0,
            path: vec![tag.to_string()],
            items,
            item_sources: None,
        }
    }

    fn value_of(node: &DicomNode) -> (&str, Option<&str>) {
        match node {
            DicomNode::Element {
                value,
                full_value_key,
                ..
            } => (value, full_value_key.as_deref()),
            DicomNode::Sequence { .. } => panic!("not an element"),
        }
    }

    fn items_of(node: &DicomNode) -> &[Vec<DicomNode>] {
        match node {
            DicomNode::Sequence { items, .. } => items,
            DicomNode::Element { .. } => panic!("not a sequence"),
        }
    }

    /// Long values, also in items and with multi-byte characters, become previews and
    /// come back whole; short ones are left alone.
    #[test]
    fn long_values_become_previews_and_come_back_whole() {
        let long = "1.5\\".repeat(400);
        let japanese = "前立腺".repeat(400);
        let short = "PTV";
        let opened = vec![
            element(&["(0010,0010)"], short),
            element(&["(0008,1030)"], &japanese),
            sequence(
                "(3006,0039)",
                vec![vec![element(
                    &["(3006,0039)", "Item#0", "(3006,0050)"],
                    &long,
                )]],
            ),
        ];
        let mut nodes = opened.clone();
        let mut full_values = FullValues::new();
        take_previews(&mut nodes, &mut full_values);

        assert_eq!(value_of(&nodes[0]), (short, None));
        assert_eq!(value_of(&nodes[1]).0.chars().count(), PREVIEW_CHARS);
        assert!(japanese.starts_with(value_of(&nodes[1]).0));
        assert_eq!(value_of(&nodes[1]).1, Some("(0008,1030)"));
        let contour = &items_of(&nodes[2])[0][0];
        assert_eq!(value_of(contour).0, preview_of(&long));
        assert_eq!(full_values.len(), 2);
        assert!(has_previews(&nodes));

        let mut saved = nodes.clone();
        restore_full_values(&mut saved, &full_values).unwrap();
        assert_eq!(value_of(&saved[1]), (japanese.as_str(), None));
        assert_eq!(value_of(&items_of(&saved[2])[0][0]), (long.as_str(), None));
        assert!(!has_previews(&saved));
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            serde_json::to_value(&opened).unwrap()
        );
    }

    /// A preview changed while keeping its key (which the page must never do) fails the
    /// save rather than writing it or the old value.
    #[test]
    fn changed_previews_fail_the_save() {
        let long = "1.5\\".repeat(400);
        let mut nodes = vec![element(&["(3006,0050)"], &long)];
        let mut full_values = FullValues::new();
        take_previews(&mut nodes, &mut full_values);
        if let DicomNode::Element { value, .. } = &mut nodes[0] {
            value.push('9');
        }
        let error = restore_full_values(&mut nodes, &full_values).unwrap_err();
        assert!(error.contains("holds only the start"), "{error}");
    }

    /// A value edited on the page drops its key and is saved as edited.
    #[test]
    fn edited_values_without_keys_are_kept() {
        let long = "1.5\\".repeat(400);
        let mut nodes = vec![element(&["(3006,0050)"], &long)];
        let mut full_values = FullValues::new();
        take_previews(&mut nodes, &mut full_values);
        if let DicomNode::Element {
            value,
            full_value_key,
            ..
        } = &mut nodes[0]
        {
            *value = "2.5".to_string();
            *full_value_key = None;
        }
        let mut saved = nodes.clone();
        restore_full_values(&mut saved, &full_values).unwrap();
        assert_eq!(value_of(&saved[0]), ("2.5", None));
    }

    /// Keys follow the elements, not their positions: after an item is removed on the
    /// page and the paths renumbered, the remaining item's value comes back whole.
    #[test]
    fn keys_follow_elements_when_items_are_renumbered() {
        let first = "1\\".repeat(600);
        let second = "2\\".repeat(600);
        let mut nodes = vec![sequence(
            "(3006,0039)",
            vec![
                vec![element(&["(3006,0039)", "Item#0", "(3006,0050)"], &first)],
                vec![element(&["(3006,0039)", "Item#1", "(3006,0050)"], &second)],
            ],
        )];
        let mut full_values = FullValues::new();
        take_previews(&mut nodes, &mut full_values);
        // The page removes the first item and renumbers the second one's paths.
        if let DicomNode::Sequence {
            items,
            item_sources,
            ..
        } = &mut nodes[0]
        {
            items.remove(0);
            *item_sources = Some(vec![Some(1)]);
            if let DicomNode::Element { path, .. } = &mut items[0][0] {
                path[1] = "Item#0".to_string();
            }
        }
        let mut saved = nodes.clone();
        restore_full_values(&mut saved, &full_values).unwrap();
        assert_eq!(value_of(&items_of(&saved[0])[0][0]).0, second);
    }

    /// An unknown key fails the save.
    #[test]
    fn unknown_keys_fail_the_save() {
        let mut nodes = vec![element(&["(3006,0050)"], &"1\\".repeat(600))];
        let mut full_values = FullValues::new();
        take_previews(&mut nodes, &mut full_values);
        let error = restore_full_values(&mut nodes, &FullValues::new()).unwrap_err();
        assert!(error.contains("no longer available"), "{error}");
    }

    /// Values become previews only when longer than `PREVIEW_ABOVE_CHARS` characters
    /// (counted as characters, not bytes).
    #[test]
    fn previews_start_above_the_threshold() {
        let at = "山".repeat(PREVIEW_ABOVE_CHARS);
        let above = "山".repeat(PREVIEW_ABOVE_CHARS + 1);
        let mut nodes = vec![
            element(&["(0008,1030)"], &at),
            element(&["(0008,103E)"], &above),
        ];
        let mut full_values = FullValues::new();
        take_previews(&mut nodes, &mut full_values);
        assert_eq!(value_of(&nodes[0]), (at.as_str(), None));
        assert_eq!(value_of(&nodes[1]).0.chars().count(), PREVIEW_CHARS);
        assert_eq!(value_of(&nodes[1]).1, Some("(0008,103E)"));
    }
}
