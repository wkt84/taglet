use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry, VirtualVr};
use dicom_core::header::{HasLength, Header, Length};
use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::{Tag, VR};
use dicom_dictionary_std::StandardDataDictionary;
use dicom_object::{open_file, DefaultDicomObject, InMemDicomObject};
use std::collections::HashSet;
use std::str::FromStr;

use super::model::{DicomNode, DicomTagInfo};

const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);

pub fn open_nodes(path: &str) -> Result<(DefaultDicomObject, Vec<DicomNode>), String> {
    let obj = open_file(path).map_err(|error| error.to_string())?;
    let nodes = object_to_nodes(&obj, Vec::new());
    Ok((obj, nodes))
}

pub fn tag_info(tag_text: &str) -> Result<DicomTagInfo, String> {
    let tag = parse_tag(tag_text)?;
    let vr = StandardDataDictionary
        .by_tag(tag)
        .map(|entry| virtual_vr_to_string(entry.vr()))
        .unwrap_or_else(|| "LO".to_string());

    let vr_for_editable = VR::from_str(&vr).unwrap_or(VR::LO);

    Ok(DicomTagInfo {
        tag: format_tag(tag),
        vr,
        description: description_for(tag),
        editable: tag != PIXEL_DATA && is_text_editable(vr_for_editable),
    })
}

pub fn object_to_nodes(obj: &InMemDicomObject, parent_path: Vec<String>) -> Vec<DicomNode> {
    obj.iter()
        .map(|element| {
            let tag = element.tag();
            let tag_text = format_tag(tag);
            let mut path = parent_path.clone();
            path.push(tag_text.clone());

            match element.value() {
                Value::Sequence(sequence) => DicomNode::Sequence {
                    tag: tag_text,
                    description: description_for(tag),
                    length: length_to_u32(element.length()),
                    path: path.clone(),
                    items: sequence
                        .items()
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            let mut item_path = path.clone();
                            item_path.push(format!("Item#{index}"));
                            object_to_nodes(item, item_path)
                        })
                        .collect(),
                },
                Value::PixelSequence(_) if tag == PIXEL_DATA => DicomNode::Element {
                    tag: tag_text,
                    vr: element.header().vr().to_string().to_owned(),
                    description: description_for(tag),
                    value: "[Binary Data]".to_string(),
                    length: length_to_u32(element.length()),
                    path,
                    editable: false,
                },
                _ => {
                    let vr = element.header().vr();
                    DicomNode::Element {
                        tag: tag_text,
                        vr: vr.to_string().to_owned(),
                        description: description_for(tag),
                        value: display_value(element.value()),
                        length: length_to_u32(element.length()),
                        path,
                        editable: tag != PIXEL_DATA && is_text_editable(vr),
                    }
                }
            }
        })
        .collect()
}

pub fn apply_nodes_to_object(obj: &mut InMemDicomObject, nodes: &[DicomNode]) -> Result<(), String> {
    let desired_tags = nodes
        .iter()
        .map(node_tag)
        .collect::<Result<HashSet<_>, _>>()?;
    let existing_tags = obj.iter().map(|element| element.tag()).collect::<Vec<_>>();

    for tag in existing_tags {
        if tag != PIXEL_DATA && !desired_tags.contains(&tag) {
            obj.remove_element(tag);
        }
    }

    for node in nodes {
        match node {
            DicomNode::Element {
                tag,
                vr,
                value,
                editable,
                ..
            } if *editable => {
                let tag = parse_tag(tag)?;
                let vr = VR::from_str(vr).map_err(|_| format!("Unsupported VR {vr}"))?;
                if is_text_editable(vr) {
                    obj.put_str(tag, vr, value);
                }
            }
            DicomNode::Sequence { tag, items, .. } => {
                let tag = parse_tag(tag)?;
                obj.update_value(tag, |value| {
                    if let Some(sequence_items) = value.items_mut() {
                        for (item, next_nodes) in sequence_items.iter_mut().zip(items.iter()) {
                            let _ = apply_nodes_to_object(item, next_nodes);
                        }
                    }
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn node_tag(node: &DicomNode) -> Result<Tag, String> {
    match node {
        DicomNode::Element { tag, .. } | DicomNode::Sequence { tag, .. } => parse_tag(tag),
    }
}

fn display_value(value: &Value<InMemDicomObject>) -> String {
    match value {
        Value::Primitive(PrimitiveValue::Empty) => String::new(),
        Value::Primitive(_) => value
            .to_str()
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| "[Binary Data]".to_string()),
        Value::PixelSequence(_) => "[Binary Data]".to_string(),
        Value::Sequence(sequence) => format!("[Sequence: {} item(s)]", sequence.items().len()),
    }
}

fn description_for(tag: Tag) -> String {
    if tag.group() % 2 == 1 {
        return "[Private]".to_string();
    }

    StandardDataDictionary
        .by_tag(tag)
        .map(|entry| entry.alias().to_string())
        .unwrap_or_else(|| "[Unknown]".to_string())
}

fn virtual_vr_to_string(vr: VirtualVr) -> String {
    match vr.exact() {
        Some(vr) => vr.to_string().to_owned(),
        None => vr.relaxed().to_string().to_owned(),
    }
}

fn is_text_editable(vr: VR) -> bool {
    matches!(
        vr,
        VR::AE
            | VR::AS
            | VR::CS
            | VR::DA
            | VR::DS
            | VR::DT
            | VR::IS
            | VR::LO
            | VR::LT
            | VR::PN
            | VR::SH
            | VR::ST
            | VR::TM
            | VR::UC
            | VR::UI
            | VR::UR
            | VR::UT
    )
}

fn format_tag(tag: Tag) -> String {
    format!("({:04X},{:04X})", tag.group(), tag.element())
}

fn parse_tag(tag: &str) -> Result<Tag, String> {
    let text = tag
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| format!("Invalid tag format: {tag}"))?;
    let (group, element) = text
        .split_once(',')
        .ok_or_else(|| format!("Invalid tag format: {tag}"))?;
    let group =
        u16::from_str_radix(group, 16).map_err(|_| format!("Invalid tag group: {group}"))?;
    let element =
        u16::from_str_radix(element, 16).map_err(|_| format!("Invalid tag element: {element}"))?;
    Ok(Tag(group, element))
}

fn length_to_u32(length: Length) -> u32 {
    if length.is_defined() {
        length.0
    } else {
        u32::MAX
    }
}
