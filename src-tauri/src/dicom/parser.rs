use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry, VirtualVr};
use dicom_core::header::{HasLength, Header, Length};
use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::{Tag, VR};
use dicom_dictionary_std::{tags, StandardDataDictionary};
use dicom_object::{open_file, DefaultDicomObject, InMemDicomObject, OpenFileOptions};
use base64::Engine;
use std::collections::HashSet;
use std::str::FromStr;

use super::model::{DicomFrameImage, DicomFramePixels, DicomImageInfo, DicomNode, DicomTagInfo};

const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);
const DOSE_GRID_SCALING: Tag = Tag(0x3004, 0x000E);

pub fn open_nodes(path: &str) -> Result<(DefaultDicomObject, Vec<DicomNode>), String> {
    let obj = OpenFileOptions::new()
        .read_until(tags::PIXEL_DATA)
        .open_file(path)
        .map_err(|error| error.to_string())?;
    let mut nodes = object_to_nodes(&obj, Vec::new());
    if pixel_data_likely_present(&obj) && !nodes.iter().any(is_pixel_data_node) {
        nodes.push(pixel_data_placeholder_node());
    }
    Ok((obj, nodes))
}

pub fn open_full_object(path: &str) -> Result<DefaultDicomObject, String> {
    open_file(path).map_err(|error| error.to_string())
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

pub fn image_info(obj: &DefaultDicomObject) -> Result<DicomImageInfo, String> {
    let transfer_syntax_uid = Some(obj.meta().transfer_syntax().trim_end_matches('\0').to_string());
    let has_pixel_data = obj.get(tags::PIXEL_DATA).is_some();
    let rows = get_u32(obj, tags::ROWS);
    let columns = get_u32(obj, tags::COLUMNS);
    let samples_per_pixel = get_u32(obj, tags::SAMPLES_PER_PIXEL);
    let bits_allocated = get_u32(obj, tags::BITS_ALLOCATED);
    let bits_stored = get_u32(obj, tags::BITS_STORED);
    let high_bit = get_u32(obj, tags::HIGH_BIT);
    let pixel_representation = get_u32(obj, tags::PIXEL_REPRESENTATION);
    let number_of_frames = get_u32(obj, tags::NUMBER_OF_FRAMES).unwrap_or(1).max(1);
    let photometric_interpretation = get_string(obj, tags::PHOTOMETRIC_INTERPRETATION);
    let modality = get_string(obj, tags::MODALITY);
    let sop_class_uid = get_string(obj, tags::SOP_CLASS_UID);
    let window_center = get_f64_values(obj, tags::WINDOW_CENTER);
    let window_width = get_f64_values(obj, tags::WINDOW_WIDTH);
    let rescale_intercept = get_f64_values(obj, tags::RESCALE_INTERCEPT).into_iter().next();
    let rescale_slope = get_f64_values(obj, tags::RESCALE_SLOPE).into_iter().next();
    let dose_grid_scaling = get_f64_values(obj, DOSE_GRID_SCALING).into_iter().next();

    let unsupported_reason = image_unsupported_reason(
        has_pixel_data,
        transfer_syntax_uid.as_deref(),
        rows,
        columns,
        samples_per_pixel,
        bits_allocated,
        photometric_interpretation.as_deref(),
    );
    let supported = unsupported_reason.is_none();

    Ok(DicomImageInfo {
        has_pixel_data,
        supported,
        unsupported_reason,
        modality,
        sop_class_uid,
        transfer_syntax_uid,
        rows,
        columns,
        samples_per_pixel,
        photometric_interpretation,
        bits_allocated,
        bits_stored,
        high_bit,
        pixel_representation,
        number_of_frames,
        window_center,
        window_width,
        rescale_intercept,
        rescale_slope,
        dose_grid_scaling,
    })
}

pub fn frame_image(
    obj: &DefaultDicomObject,
    frame_index: u32,
    window_center_override: Option<f64>,
    window_width_override: Option<f64>,
) -> Result<DicomFrameImage, String> {
    let info = image_info(obj)?;
    if !info.supported {
        return Err(info
            .unsupported_reason
            .unwrap_or_else(|| "Image is not supported".to_string()));
    }

    let width = info.columns.ok_or_else(|| "Columns is missing".to_string())?;
    let height = info.rows.ok_or_else(|| "Rows is missing".to_string())?;
    let samples_per_pixel = info.samples_per_pixel.unwrap_or(1);
    if samples_per_pixel != 1 {
        return Err("Only grayscale frames are rendered for now".to_string());
    }
    if frame_index >= info.number_of_frames {
        return Err(format!(
            "Frame index {frame_index} is out of range for {} frame(s)",
            info.number_of_frames
        ));
    }

    let bits_allocated = info
        .bits_allocated
        .ok_or_else(|| "Bits Allocated is missing".to_string())?;
    let bytes_per_sample = bytes_per_sample(bits_allocated)?;
    let frame_sample_count = width as usize * height as usize * samples_per_pixel as usize;
    let frame_byte_len = frame_sample_count
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| "Frame byte size overflow".to_string())?;
    let frame_offset = frame_byte_len
        .checked_mul(frame_index as usize)
        .ok_or_else(|| "Frame offset overflow".to_string())?;

    let pixel_bytes = obj
        .get(tags::PIXEL_DATA)
        .ok_or_else(|| "Pixel Data element is missing".to_string())?
        .to_bytes()
        .map_err(|error| error.to_string())?;

    if pixel_bytes.len() < frame_offset + frame_byte_len {
        return Err(format!(
            "Pixel Data is too short for frame {frame_index}: expected at least {} bytes, got {}",
            frame_offset + frame_byte_len,
            pixel_bytes.len()
        ));
    }

    let frame = &pixel_bytes[frame_offset..frame_offset + frame_byte_len];
    let signed = info.pixel_representation.unwrap_or(0) == 1;
    let (slope, intercept) = pixel_value_transform(&info);
    let mut values = Vec::with_capacity(width as usize * height as usize);

    for index in 0..frame_sample_count {
        let stored = stored_pixel_value(frame, index, bits_allocated, signed);
        values.push(stored * slope + intercept);
    }

    let (window_center, window_width) = match (window_center_override, window_width_override) {
        (Some(center), Some(width)) if width > 0.0 => (center, width),
        _ => window_for_frame(&info, &values),
    };
    let invert = info
        .photometric_interpretation
        .as_deref()
        .is_some_and(|photometric| photometric == "MONOCHROME1");
    let rgba = grayscale_to_rgba(&values, window_center, window_width, invert);

    Ok(DicomFrameImage {
        width,
        height,
        frame_index,
        rgba_base64: base64::engine::general_purpose::STANDARD.encode(rgba),
    })
}

pub fn frame_pixels(obj: &DefaultDicomObject, frame_index: u32) -> Result<DicomFramePixels, String> {
    let info = image_info(obj)?;
    if !info.supported {
        return Err(info
            .unsupported_reason
            .unwrap_or_else(|| "Image is not supported".to_string()));
    }

    let width = info.columns.ok_or_else(|| "Columns is missing".to_string())?;
    let height = info.rows.ok_or_else(|| "Rows is missing".to_string())?;
    let samples_per_pixel = info.samples_per_pixel.unwrap_or(1);
    if samples_per_pixel != 1 {
        return Err("Only grayscale frames are rendered for now".to_string());
    }
    if frame_index >= info.number_of_frames {
        return Err(format!(
            "Frame index {frame_index} is out of range for {} frame(s)",
            info.number_of_frames
        ));
    }

    let bits_allocated = info
        .bits_allocated
        .ok_or_else(|| "Bits Allocated is missing".to_string())?;
    let bytes_per_sample = bytes_per_sample(bits_allocated)?;
    let frame_sample_count = width as usize * height as usize * samples_per_pixel as usize;
    let frame_byte_len = frame_sample_count
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| "Frame byte size overflow".to_string())?;
    let frame_offset = frame_byte_len
        .checked_mul(frame_index as usize)
        .ok_or_else(|| "Frame offset overflow".to_string())?;

    let pixel_bytes = obj
        .get(tags::PIXEL_DATA)
        .ok_or_else(|| "Pixel Data element is missing".to_string())?
        .to_bytes()
        .map_err(|error| error.to_string())?;

    if pixel_bytes.len() < frame_offset + frame_byte_len {
        return Err(format!(
            "Pixel Data is too short for frame {frame_index}: expected at least {} bytes, got {}",
            frame_offset + frame_byte_len,
            pixel_bytes.len()
        ));
    }

    let frame = &pixel_bytes[frame_offset..frame_offset + frame_byte_len];
    let signed = info.pixel_representation.unwrap_or(0) == 1;
    let (slope, intercept) = pixel_value_transform(&info);
    let (min_value, max_value) =
        frame_min_max(frame, bits_allocated, signed, slope, intercept, frame_sample_count);

    Ok(DicomFramePixels {
        width,
        height,
        frame_index,
        bits_allocated,
        pixel_representation: info.pixel_representation.unwrap_or(0),
        photometric_interpretation: info.photometric_interpretation,
        rescale_intercept: intercept,
        rescale_slope: slope,
        dose_grid_scaling: info.dose_grid_scaling,
        pixel_base64: base64::engine::general_purpose::STANDARD.encode(frame),
        min_value,
        max_value,
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
    let has_pixel_data = obj.get(PIXEL_DATA).is_some();

    for tag in existing_tags {
        if has_pixel_data && tag >= PIXEL_DATA {
            continue;
        }
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
                if tag == PIXEL_DATA {
                    continue;
                }
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

fn pixel_data_likely_present(obj: &DefaultDicomObject) -> bool {
    get_u32(obj, tags::ROWS).is_some()
        && get_u32(obj, tags::COLUMNS).is_some()
        && get_u32(obj, tags::BITS_ALLOCATED).is_some()
}

fn is_pixel_data_node(node: &DicomNode) -> bool {
    match node {
        DicomNode::Element { tag, .. } | DicomNode::Sequence { tag, .. } => tag == "(7FE0,0010)",
    }
}

fn pixel_data_placeholder_node() -> DicomNode {
    DicomNode::Element {
        tag: format_tag(PIXEL_DATA),
        vr: "OB/OW".to_string(),
        description: "PixelData".to_string(),
        value: "[Binary Data]".to_string(),
        length: u32::MAX,
        path: vec![format_tag(PIXEL_DATA)],
        editable: false,
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

fn get_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.get(tag)
        .and_then(|element| element.to_str().ok())
        .map(|value| value.trim_end_matches('\0').to_string())
}

fn get_u32(obj: &InMemDicomObject, tag: Tag) -> Option<u32> {
    obj.get(tag).and_then(|element| {
        element
            .to_int::<u32>()
            .ok()
            .or_else(|| element.to_str().ok()?.trim().parse::<u32>().ok())
    })
}

fn get_f64_values(obj: &InMemDicomObject, tag: Tag) -> Vec<f64> {
    obj.get(tag)
        .and_then(|element| element.to_multi_float64().ok())
        .unwrap_or_default()
}

fn image_unsupported_reason(
    has_pixel_data: bool,
    transfer_syntax_uid: Option<&str>,
    rows: Option<u32>,
    columns: Option<u32>,
    samples_per_pixel: Option<u32>,
    bits_allocated: Option<u32>,
    photometric_interpretation: Option<&str>,
) -> Option<String> {
    if !has_pixel_data {
        return Some("No Pixel Data element found".to_string());
    }
    if rows.unwrap_or_default() == 0 || columns.unwrap_or_default() == 0 {
        return Some("Rows or Columns is missing".to_string());
    }
    if samples_per_pixel.unwrap_or_default() == 0 {
        return Some("Samples Per Pixel is missing".to_string());
    }
    if bits_allocated.unwrap_or_default() == 0 {
        return Some("Bits Allocated is missing".to_string());
    }
    if !matches!(
        transfer_syntax_uid,
        Some("1.2.840.10008.1.2") | Some("1.2.840.10008.1.2.1")
    ) {
        return Some("Only uncompressed Little Endian transfer syntaxes are prepared for now".to_string());
    }
    if let Some(photometric) = photometric_interpretation {
        if !matches!(photometric, "MONOCHROME1" | "MONOCHROME2" | "RGB") {
            return Some(format!("Photometric Interpretation {photometric} is not prepared yet"));
        }
    }

    None
}

fn window_for_frame(info: &DicomImageInfo, values: &[f64]) -> (f64, f64) {
    if let (Some(center), Some(width)) = (info.window_center.first(), info.window_width.first()) {
        if *width > 0.0 {
            return (*center, *width);
        }
    }

    let (min, max) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), value| {
            (min.min(*value), max.max(*value))
        });

    if min.is_finite() && max.is_finite() && max > min {
        ((min + max) / 2.0, max - min)
    } else {
        (0.0, 1.0)
    }
}

fn grayscale_to_rgba(values: &[f64], center: f64, width: f64, invert: bool) -> Vec<u8> {
    let low = center - width / 2.0;
    let mut rgba = Vec::with_capacity(values.len() * 4);

    for value in values {
        let normalized = ((*value - low) / width).clamp(0.0, 1.0);
        let gray = if invert {
            ((1.0 - normalized) * 255.0).round() as u8
        } else {
            (normalized * 255.0).round() as u8
        };
        rgba.extend_from_slice(&[gray, gray, gray, 255]);
    }

    rgba
}

fn bytes_per_sample(bits_allocated: u32) -> Result<usize, String> {
    match bits_allocated {
        8 => Ok(1),
        16 => Ok(2),
        32 => Ok(4),
        other => Err(format!("Bits Allocated {other} is not supported yet")),
    }
}

fn pixel_value_transform(info: &DicomImageInfo) -> (f64, f64) {
    if let Some(dose_grid_scaling) = info.dose_grid_scaling {
        return (dose_grid_scaling, 0.0);
    }

    (
        info.rescale_slope.unwrap_or(1.0),
        info.rescale_intercept.unwrap_or(0.0),
    )
}

fn stored_pixel_value(frame: &[u8], index: usize, bits_allocated: u32, signed: bool) -> f64 {
    match bits_allocated {
        8 => {
            let value = frame[index];
            if signed {
                i8::from_le_bytes([value]) as f64
            } else {
                value as f64
            }
        }
        16 => {
            let offset = index * 2;
            let bytes = [frame[offset], frame[offset + 1]];
            if signed {
                i16::from_le_bytes(bytes) as f64
            } else {
                u16::from_le_bytes(bytes) as f64
            }
        }
        32 => {
            let offset = index * 4;
            let bytes = [
                frame[offset],
                frame[offset + 1],
                frame[offset + 2],
                frame[offset + 3],
            ];
            if signed {
                i32::from_le_bytes(bytes) as f64
            } else {
                u32::from_le_bytes(bytes) as f64
            }
        }
        _ => 0.0,
    }
}

fn frame_min_max(
    frame: &[u8],
    bits_allocated: u32,
    signed: bool,
    slope: f64,
    intercept: f64,
    sample_count: usize,
) -> (f64, f64) {
    let mut min_value = f64::INFINITY;
    let mut max_value = f64::NEG_INFINITY;

    for index in 0..sample_count {
        let stored = stored_pixel_value(frame, index, bits_allocated, signed) * slope + intercept;
        min_value = min_value.min(stored);
        max_value = max_value.max(stored);
    }

    if min_value.is_finite() && max_value.is_finite() {
        (min_value, max_value)
    } else {
        (0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_unsigned_32_bit_pixels() {
        let frame = [
            0x78, 0x56, 0x34, 0x12,
            0xFF, 0xFF, 0xFF, 0xFF,
        ];

        assert_eq!(stored_pixel_value(&frame, 0, 32, false), 0x1234_5678 as f64);
        assert_eq!(stored_pixel_value(&frame, 1, 32, false), u32::MAX as f64);
    }

    #[test]
    fn decodes_signed_32_bit_pixels() {
        let frame = [
            0xFF, 0xFF, 0xFF, 0xFF,
            0x00, 0x00, 0x00, 0x80,
        ];

        assert_eq!(stored_pixel_value(&frame, 0, 32, true), -1.0);
        assert_eq!(stored_pixel_value(&frame, 1, 32, true), i32::MIN as f64);
    }

    #[test]
    fn applies_dose_grid_scaling_to_min_max() {
        let frame = [
            0x00, 0x00, 0x00, 0x00,
            0x10, 0x27, 0x00, 0x00,
        ];

        let (min, max) = frame_min_max(&frame, 32, false, 0.001, 0.0, 2);

        assert_eq!(min, 0.0);
        assert_eq!(max, 10.0);
    }
}
