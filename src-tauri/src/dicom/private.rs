use std::collections::HashMap;

use dicom_core::header::Header;
use dicom_core::{Tag, VR};
use dicom_object::InMemDicomObject;

use super::model::DicomNode;
use super::parser::parse_tag;

const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);

fn creator_tag(tag: Tag) -> Option<Tag> {
    (tag.group() % 2 == 1 && tag.element() >= 0x1000).then(|| Tag(tag.group(), tag.element() >> 8))
}

fn stored_creator(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    let element = obj.get(tag)?;
    if element.vr() != VR::LO {
        return None;
    }
    element.to_str().ok().map(|value| value.into_owned())
}

fn effective_creator(
    obj: &InMemDicomObject,
    nodes: &HashMap<Tag, &DicomNode>,
    tag: Tag,
) -> Option<String> {
    match nodes.get(&tag) {
        Some(DicomNode::Element {
            vr,
            value,
            editable: true,
            ..
        }) if vr == "LO" => Some(value.trim_end_matches([' ', '\0']).to_string()),
        Some(DicomNode::Element {
            editable: false, ..
        }) => stored_creator(obj, tag),
        _ if obj.get(PIXEL_DATA).is_some() && tag >= PIXEL_DATA => stored_creator(obj, tag),
        _ => None,
    }
}

fn valid_creator(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 64
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

fn node_changed(obj: &InMemDicomObject, node: &DicomNode) -> Result<bool, String> {
    match node {
        DicomNode::Element {
            tag,
            vr,
            value,
            editable: true,
            ..
        } => Ok(obj.get(parse_tag(tag)?).is_none_or(|original| {
            original.vr().to_string() != *vr
                || original
                    .to_str()
                    .map(|text| text.as_ref() != value.trim_end_matches([' ', '\0']))
                    .unwrap_or(true)
        })),
        DicomNode::Element { tag, .. } => Ok(obj.get(parse_tag(tag)?).is_none()),
        DicomNode::Sequence { tag, items, .. } => {
            let Some(original) = obj
                .get(parse_tag(tag)?)
                .and_then(|element| element.value().items())
            else {
                return Ok(true);
            };
            if original.len() != items.len() {
                return Ok(true);
            }
            for (item, next_nodes) in original.iter().zip(items) {
                let desired = next_nodes
                    .iter()
                    .map(|node| match node {
                        DicomNode::Element { tag, .. } | DicomNode::Sequence { tag, .. } => {
                            parse_tag(tag)
                        }
                    })
                    .collect::<Result<std::collections::HashSet<_>, _>>()?;
                let has_pixel_data = item.get(PIXEL_DATA).is_some();
                if item.iter().any(|element| {
                    let tag = element.tag();
                    !(tag == PIXEL_DATA
                        || has_pixel_data && tag >= PIXEL_DATA
                        || desired.contains(&tag))
                }) {
                    return Ok(true);
                }
                for node in next_nodes {
                    if node_changed(item, node)? {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
    }
}

/// Validate before mutating the source object. Each Sequence Item has its own
/// creator reservations; a parent or sibling Item cannot supply a creator.
/// Existing orphan elements may be preserved, but cannot be added or edited.
pub(super) fn validate_private_nodes(
    obj: &InMemDicomObject,
    nodes: &[DicomNode],
) -> Result<(), String> {
    let proposed = nodes
        .iter()
        .map(|node| {
            let tag = match node {
                DicomNode::Element { tag, .. } | DicomNode::Sequence { tag, .. } => tag,
            };
            Ok((parse_tag(tag)?, node))
        })
        .collect::<Result<HashMap<_, _>, String>>()?;

    // The lightweight open omits elements after Pixel Data, which save retains.
    let mut final_tags = proposed.keys().copied().collect::<Vec<_>>();
    if obj.get(PIXEL_DATA).is_some() {
        final_tags.extend(
            obj.iter()
                .map(|element| element.tag())
                .filter(|tag| *tag >= PIXEL_DATA),
        );
    }
    for tag in final_tags {
        let Some(creator) = creator_tag(tag) else {
            continue;
        };
        let next_creator = effective_creator(obj, &proposed, creator);
        let previous_creator = stored_creator(obj, creator);
        if obj.get(tag).is_some()
            && previous_creator.as_deref().is_some_and(valid_creator)
            && previous_creator != next_creator
        {
            return Err(format!(
                "Cannot change or delete Private Creator {creator} while private element {tag} remains. Remove its block elements first."
            ));
        }

        let changed = match proposed.get(&tag) {
            Some(node) => node_changed(obj, node)?,
            None => false,
        };
        if changed && !next_creator.as_deref().is_some_and(valid_creator) {
            return Err(format!(
                "Private element {tag} requires a non-empty LO Private Creator {creator} in the same dataset or Sequence Item."
            ));
        }
    }

    for node in nodes {
        if let DicomNode::Sequence { tag, items, .. } = node {
            if let Some(original) = obj
                .get(parse_tag(tag)?)
                .and_then(|element| element.value().items())
            {
                for (index, (item, next_nodes)) in original.iter().zip(items).enumerate() {
                    validate_private_nodes(item, next_nodes)
                        .map_err(|error| format!("{tag} / Item #{}: {error}", index + 1))?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::parser::{apply_nodes_to_object, object_to_nodes};
    use super::*;
    use dicom_core::{value::DataSetSequence, DataElement, Length};

    const CREATOR: Tag = Tag(0x0019, 0x0010);
    const DATA: Tag = Tag(0x0019, 0x1001);

    fn object() -> InMemDicomObject {
        let mut obj = InMemDicomObject::new_empty();
        obj.put_str(CREATOR, VR::LO, "VENDOR");
        obj.put_str(DATA, VR::LO, "OLD");
        obj
    }

    fn set_value(nodes: &mut [DicomNode], tag: &str, value: &str) {
        for node in nodes {
            if let DicomNode::Element {
                tag: current,
                value: text,
                ..
            } = node
            {
                if current == tag {
                    *text = value.to_string();
                }
            }
        }
    }

    #[test]
    fn accepts_private_edits_with_local_creator() {
        let mut obj = object();
        let mut nodes = object_to_nodes(&obj, vec![]);
        set_value(&mut nodes, "(0019,1001)", "NEW");
        apply_nodes_to_object(&mut obj, &nodes).unwrap();
        assert_eq!(obj.get(DATA).unwrap().to_str().unwrap(), "NEW");
    }

    #[test]
    fn rejects_creator_change_clear_and_deletion_before_any_mutation() {
        for value in [Some("OTHER"), Some(""), None] {
            let mut obj = object();
            obj.put_str(Tag(0x0010, 0x0020), VR::LO, "PATIENT");
            let mut nodes = object_to_nodes(&obj, vec![]);
            set_value(&mut nodes, "(0010,0020)", "CHANGED");
            if let Some(value) = value {
                set_value(&mut nodes, "(0019,0010)", value);
            } else {
                nodes.retain(
                    |node| !matches!(node, DicomNode::Element { tag, .. } if tag == "(0019,0010)"),
                );
            }
            assert!(apply_nodes_to_object(&mut obj, &nodes).is_err());
            assert_eq!(
                obj.get(Tag(0x0010, 0x0020)).unwrap().to_str().unwrap(),
                "PATIENT"
            );
            assert_eq!(obj.get(CREATOR).unwrap().to_str().unwrap(), "VENDOR");
        }
    }

    #[test]
    fn permits_removing_creator_with_entire_block() {
        let mut obj = object();
        apply_nodes_to_object(&mut obj, &[]).unwrap();
        assert!(obj.get(CREATOR).is_none());
        assert!(obj.get(DATA).is_none());
    }

    #[test]
    fn preserves_existing_orphans_but_rejects_orphan_edits_and_additions() {
        let mut obj = object();
        obj.remove_element(CREATOR);
        let mut nodes = object_to_nodes(&obj, vec![]);
        apply_nodes_to_object(&mut obj, &nodes).unwrap();
        set_value(&mut nodes, "(0019,1001)", "NEW");
        assert!(apply_nodes_to_object(&mut obj, &nodes).is_err());
        assert!(apply_nodes_to_object(&mut InMemDicomObject::new_empty(), &nodes).is_err());
    }

    #[test]
    fn accepts_new_private_block_with_creator() {
        let nodes = object_to_nodes(&object(), vec![]);
        let mut obj = InMemDicomObject::new_empty();
        apply_nodes_to_object(&mut obj, &nodes).unwrap();
        assert!(obj.get(DATA).is_some());
    }

    #[test]
    fn checks_exact_creator_slot_and_group() {
        for wrong_creator in [Tag(0x0019, 0x0011), Tag(0x0021, 0x0010)] {
            let mut obj = object();
            obj.remove_element(CREATOR);
            obj.put_str(wrong_creator, VR::LO, "VENDOR");
            let mut nodes = object_to_nodes(&obj, vec![]);
            set_value(&mut nodes, "(0019,1001)", "NEW");
            assert!(apply_nodes_to_object(&mut obj, &nodes).is_err());
        }
    }

    #[test]
    fn checks_private_sequences_and_isolates_item_creators() {
        let mut obj = object();
        let mut orphan = object();
        orphan.remove_element(CREATOR);
        obj.put(DataElement::new(
            Tag(0x0019, 0x1002),
            VR::SQ,
            DataSetSequence::new(vec![object(), orphan], Length::UNDEFINED),
        ));
        let mut nodes = object_to_nodes(&obj, vec![]);
        for node in &mut nodes {
            if let DicomNode::Sequence { items, .. } = node {
                set_value(&mut items[1], "(0019,1001)", "NEW");
            }
        }
        let error = apply_nodes_to_object(&mut obj, &nodes).unwrap_err();
        assert!(error.contains("Item #2"));
        let mut nodes = object_to_nodes(&obj, vec![]);
        nodes
            .retain(|node| !matches!(node, DicomNode::Element { tag, .. } if tag == "(0019,0010)"));
        assert!(apply_nodes_to_object(&mut obj, &nodes).is_err());
    }

    #[test]
    fn orphan_private_sequence_can_be_preserved_but_not_modified() {
        let mut child = InMemDicomObject::new_empty();
        child.put_str(Tag(0x0010, 0x0020), VR::LO, "PATIENT");
        let mut obj = InMemDicomObject::new_empty();
        obj.put(DataElement::new(
            DATA,
            VR::SQ,
            DataSetSequence::new(vec![child], Length::UNDEFINED),
        ));
        let mut nodes = object_to_nodes(&obj, vec![]);
        apply_nodes_to_object(&mut obj, &nodes).unwrap();
        if let DicomNode::Sequence { items, .. } = &mut nodes[0] {
            set_value(&mut items[0], "(0010,0020)", "CHANGED");
        }
        assert!(apply_nodes_to_object(&mut obj, &nodes).is_err());
        if let DicomNode::Sequence { items, .. } = &mut nodes[0] {
            items[0].clear();
        }
        assert!(apply_nodes_to_object(&mut obj, &nodes).is_err());
    }

    #[test]
    fn protects_creator_for_private_data_omitted_after_pixel_data() {
        let creator = Tag(0x7FE1, 0x0010);
        let mut obj = InMemDicomObject::new_empty();
        obj.put_str(creator, VR::LO, "VENDOR");
        obj.put(DataElement::new(
            PIXEL_DATA,
            VR::OB,
            dicom_core::value::PrimitiveValue::U8(vec![0, 0].into()),
        ));
        obj.put_str(Tag(0x7FE1, 0x1001), VR::LO, "HIDDEN");
        apply_nodes_to_object(&mut obj, &[]).unwrap();
        let mut node = object_to_nodes(&obj, vec![])
            .into_iter()
            .find(|node| matches!(node, DicomNode::Element { tag, .. } if tag == "(7FE1,0010)"))
            .unwrap();
        if let DicomNode::Element { value, .. } = &mut node {
            *value = "OTHER".into();
        }
        assert!(apply_nodes_to_object(&mut obj, &[node]).is_err());
        assert_eq!(obj.get(creator).unwrap().to_str().unwrap(), "VENDOR");
    }
}
