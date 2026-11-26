use base64::Engine;
use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry, VirtualVr};
use dicom_core::header::{HasLength, Header, Length};
use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::{DataElement, Tag, VR};
use dicom_dictionary_std::{tags, uids, StandardDataDictionary};
use dicom_encoding::text::SpecificCharacterSet;
use dicom_encoding::transfer_syntax::TransferSyntaxIndex;
use dicom_object::{
    DefaultDicomObject, FileMetaTable, FileMetaTableBuilder, InMemDicomObject, OpenFileOptions,
};
use dicom_parser::dataset::lazy_read::LazyDataSetReader;
use dicom_parser::dataset::{DataToken, LazyDataToken};
use dicom_parser::stateful::decode::StatefulDecode;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::str::FromStr;

use super::charset::{
    byte_transparent_string, dicom_rs_codec_after, dicom_rs_codec_is_byte_transparent,
    dicom_rs_read, dicom_rs_write, is_charset_sensitive, is_readable_declaration,
    is_single_valued_text, is_writable_declaration, trim_padding, TextCharset,
};
use super::deflate;
use super::model::{
    DicomFileContent, DicomFrameImage, DicomFramePixels, DicomImageInfo, DicomNode, DicomTagInfo,
    RtPlanBeam, RtPlanBeamLimitingDeviceDefinition, RtPlanBeamLimitingDevicePosition,
    RtPlanBevInfo, RtPlanControlPoint, RtStructBounds, RtStructContour, RtStructData, RtStructInfo,
    RtStructPoint, RtStructRoi, RtStructSlice, RtStructSliceContours,
};
use super::private::validate_private_nodes;
use super::splice;
use super::text_values::TextValues;

const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);
const DOSE_GRID_SCALING: Tag = Tag(0x3004, 0x000E);
const BEAM_SEQUENCE: Tag = Tag(0x300A, 0x00B0);
const BEAM_NUMBER: Tag = Tag(0x300A, 0x00C0);
const BEAM_NAME: Tag = Tag(0x300A, 0x00C2);
const BEAM_LIMITING_DEVICE_SEQUENCE: Tag = Tag(0x300A, 0x00B6);
const CONTROL_POINT_SEQUENCE: Tag = Tag(0x300A, 0x0111);
const CONTROL_POINT_INDEX: Tag = Tag(0x300A, 0x0112);
const BEAM_LIMITING_DEVICE_POSITION_SEQUENCE: Tag = Tag(0x300A, 0x011A);
const RT_BEAM_LIMITING_DEVICE_TYPE: Tag = Tag(0x300A, 0x00B8);
const NUMBER_OF_LEAF_JAW_PAIRS: Tag = Tag(0x300A, 0x00BC);
const LEAF_POSITION_BOUNDARIES: Tag = Tag(0x300A, 0x00BE);
const LEAF_JAW_POSITIONS: Tag = Tag(0x300A, 0x011C);
const GANTRY_ANGLE: Tag = Tag(0x300A, 0x011E);
const BEAM_LIMITING_DEVICE_ANGLE: Tag = Tag(0x300A, 0x0120);
const PATIENT_SUPPORT_ANGLE: Tag = Tag(0x300A, 0x0122);
const STRUCTURE_SET_LABEL: Tag = Tag(0x3006, 0x0002);
const STRUCTURE_SET_ROI_SEQUENCE: Tag = Tag(0x3006, 0x0020);
const ROI_NUMBER: Tag = Tag(0x3006, 0x0022);
const ROI_NAME: Tag = Tag(0x3006, 0x0026);
const ROI_DISPLAY_COLOR: Tag = Tag(0x3006, 0x002A);
const ROI_CONTOUR_SEQUENCE: Tag = Tag(0x3006, 0x0039);
const CONTOUR_SEQUENCE: Tag = Tag(0x3006, 0x0040);
const CONTOUR_GEOMETRIC_TYPE: Tag = Tag(0x3006, 0x0042);
const CONTOUR_DATA: Tag = Tag(0x3006, 0x0050);
const REFERENCED_ROI_NUMBER: Tag = Tag(0x3006, 0x0084);
const RTSTRUCT_Z_TOLERANCE: f64 = 0.01;
const MISSING_FILE_META_WARNING: &str =
    "File meta information header is missing. The dataset was read as Implicit VR Little Endian.";
const MAX_RAW_DISPLAY_VALUE_LENGTH: u32 = 1_048_576;
const MAX_RAW_DISPLAY_ELEMENTS: usize = 20_000;
const TEXT_READ_ONLY_WARNING: &str =
    "Some text is in a character set that cannot be written back in its place (e.g. set only inside a sequence item). It is shown read-only.";
const TEXT_SAVE_BLOCKED_WARNING: &str =
    "Some text in this file cannot be written back without changing its bytes, so saving is disabled for this file.";
const TEXT_ADD_BLOCKED_WARNING: &str =
    "Some text in this file cannot be written back by the full writer, and the file's tags are not in order, so saving fails if elements are added. Values can still be edited and elements removed.";
const DEFLATED_NEEDS_SPLICING: &str =
    "Cannot save: this file's data set is deflated (Deflated Explicit VR Little Endian), and Taglet can save such files only by editing them in place, which these changes do not allow (e.g. adding a non-text element, or removing a group length or a SOP UID).";
const RAW_DATASET_START_GROUPS: &[u16] = &[
    0x0008, 0x0010, 0x0018, 0x0020, 0x0028, 0x3004, 0x3006, 0x300A, 0x300C,
];

pub fn open_nodes(path: &str) -> Result<DicomFileContent, String> {
    let mut content = open_nodes_without_text_values(path)?;
    let text = TextValues::read(path);
    // Files that can be saved by splicing can have any decoded value edited, since
    // splicing writes the encoded bytes itself (#8).
    let splice_index = can_splice(path, &content);
    let spliceable = splice_index.is_some();
    let mut read_only = false;
    for nodes in [&mut content.file_meta, &mut content.nodes] {
        read_only |= apply_text_values(nodes, &text, spliceable);
    }
    if read_only {
        content.warnings.push(TEXT_READ_ONLY_WARNING.to_string());
    }
    let blocked = !text.paths_not_round_tripping().is_empty();
    if blocked && !spliceable {
        content.warnings.push(TEXT_SAVE_BLOCKED_WARNING.to_string());
    }
    // Elements are only inserted by splicing into files with tags in order; adding
    // one to this file goes through the full writer, which cannot write this text.
    if blocked && splice_index.is_some_and(|ordered| !ordered) {
        content.warnings.push(TEXT_ADD_BLOCKED_WARNING.to_string());
    }
    Ok(content)
}

/// Whether value edits to this file can be saved by splicing: a Part 10 file whose
/// data set is Explicit or Implicit VR Little Endian, Explicit VR Big Endian or deflated,
/// that can be indexed, and whose file meta SOP UIDs match the data set's (see
/// `plan_splice`).
/// `Some` with whether its tags are in order (needed to add elements), or `None`.
fn can_splice(path: &str, content: &DicomFileContent) -> Option<bool> {
    let value_of = |nodes: &[DicomNode], wanted: &str| {
        nodes.iter().find_map(|node| match node {
            DicomNode::Element { tag, value, .. } if tag == wanted => {
                Some(value.trim_end_matches(['\0', ' ']).to_string())
            }
            _ => None,
        })
    };
    let meta_matches = |meta_tag: &str, dataset_tag: &str| {
        let dataset = value_of(&content.nodes, dataset_tag);
        dataset.is_none() || dataset == value_of(&content.file_meta, meta_tag)
    };
    let supported = value_of(&content.file_meta, "(0002,0010)")
        .is_some_and(|uid| splice::supports_transfer_syntax(&uid))
        && meta_matches("(0002,0002)", "(0008,0016)")
        && meta_matches("(0002,0003)", "(0008,0018)");
    if !supported {
        return None;
    }
    splice::FileIndex::read(path)
        .ok()
        .filter(|index| !has_irregular_group_length(index.nodes()))
        .map(|index| index.is_ordered())
}

fn open_nodes_without_text_values(path: &str) -> Result<DicomFileContent, String> {
    let opened = match open_deflated_object(path, true)? {
        Some(obj) => Ok(obj),
        None => OpenFileOptions::new()
            .read_until(tags::PIXEL_DATA)
            .open_file(path)
            .map_err(|error| error.to_string()),
    };
    match opened {
        Ok(obj) => {
            let file_meta = file_meta_to_nodes(obj.meta());
            let mut nodes = object_to_nodes(&obj, Vec::new());
            if pixel_data_likely_present(&obj) && !nodes.iter().any(is_pixel_data_node) {
                nodes.push(pixel_data_placeholder_node());
            }
            Ok(DicomFileContent {
                file_meta,
                nodes,
                warnings: Vec::new(),
            })
        }
        Err(standard_error) => {
            let nodes = read_raw_implicit_vr_little_endian_nodes(path).map_err(|raw_error| {
                format!(
                    "Could not read as DICOM Part 10 ({standard_error}) or raw Implicit VR Little Endian dataset ({raw_error})"
                )
            })?;
            Ok(DicomFileContent {
                file_meta: Vec::new(),
                nodes,
                warnings: vec![MISSING_FILE_META_WARNING.to_string()],
            })
        }
    }
}

pub fn open_full_object(path: &str) -> Result<DefaultDicomObject, String> {
    open_object_with_fallback(path, false).map(|(obj, _)| obj)
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
    let transfer_syntax_uid = Some(
        obj.meta()
            .transfer_syntax()
            .trim_end_matches('\0')
            .to_string(),
    );
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
    let rescale_intercept = get_f64_values(obj, tags::RESCALE_INTERCEPT)
        .into_iter()
        .next();
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

    let width = info
        .columns
        .ok_or_else(|| "Columns is missing".to_string())?;
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
    let layout = PixelLayout::from_info(&info, bits_allocated);
    let (slope, intercept) = pixel_value_transform(&info);
    let mut values = Vec::with_capacity(width as usize * height as usize);

    for index in 0..frame_sample_count {
        let stored = stored_pixel_value(frame, index, layout);
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

pub fn frame_pixels(
    obj: &DefaultDicomObject,
    frame_index: u32,
) -> Result<DicomFramePixels, String> {
    let info = image_info(obj)?;
    if !info.supported {
        return Err(info
            .unsupported_reason
            .unwrap_or_else(|| "Image is not supported".to_string()));
    }

    let width = info
        .columns
        .ok_or_else(|| "Columns is missing".to_string())?;
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
    let layout = PixelLayout::from_info(&info, bits_allocated);
    let (slope, intercept) = pixel_value_transform(&info);
    let (min_value, max_value) = frame_min_max(frame, layout, slope, intercept, frame_sample_count);

    Ok(DicomFramePixels {
        width,
        height,
        frame_index,
        bits_allocated,
        bits_stored: layout.bits_stored,
        high_bit: layout.high_bit,
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

pub fn rt_plan_bev_info(
    obj: &DefaultDicomObject,
    text: &TextValues,
) -> Result<RtPlanBevInfo, String> {
    let modality = get_string(obj, tags::MODALITY);
    if modality.as_deref() != Some("RTPLAN") {
        return Ok(RtPlanBevInfo {
            supported: false,
            unsupported_reason: Some("BEV is available for RT Plan objects only".to_string()),
            modality,
            beams: Vec::new(),
        });
    }

    let Some(beam_items) = sequence_items(obj, BEAM_SEQUENCE) else {
        return Ok(RtPlanBevInfo {
            supported: false,
            unsupported_reason: Some("Beam Sequence is missing".to_string()),
            modality,
            beams: Vec::new(),
        });
    };

    let beams = beam_items
        .iter()
        .enumerate()
        .map(|(beam_index, beam)| {
            let control_points = sequence_items(beam, CONTROL_POINT_SEQUENCE)
                .map(effective_control_points)
                .unwrap_or_default();

            RtPlanBeam {
                beam_index,
                beam_number: get_i32(beam, BEAM_NUMBER),
                beam_name: get_text(
                    beam,
                    BEAM_NAME,
                    text,
                    &[format_tag(BEAM_SEQUENCE), format!("Item#{beam_index}")],
                ),
                devices: beam_limiting_device_definitions(beam),
                control_points,
            }
        })
        .collect::<Vec<_>>();

    Ok(RtPlanBevInfo {
        supported: !beams.is_empty(),
        unsupported_reason: if beams.is_empty() {
            Some("Beam Sequence is empty".to_string())
        } else {
            None
        },
        modality,
        beams,
    })
}

pub fn rt_struct_data(obj: &DefaultDicomObject, text: &TextValues) -> Result<RtStructData, String> {
    let modality = get_string(obj, tags::MODALITY);
    if modality.as_deref() != Some("RTSTRUCT") {
        return Ok(RtStructData {
            info: RtStructInfo {
                supported: false,
                unsupported_reason: Some(
                    "RT Structure Set viewer is available for RTSTRUCT objects only".to_string(),
                ),
                modality,
                structure_set_label: get_text(obj, STRUCTURE_SET_LABEL, text, &[]),
                rois: Vec::new(),
                slices: Vec::new(),
                bounds: None,
            },
            contours: Vec::new(),
        });
    }

    let roi_names = structure_set_roi_names(obj, text);
    let roi_contours = rt_struct_contours(obj, &roi_names);
    let rois = rt_struct_rois(obj, &roi_names, &roi_contours);
    let slices = rt_struct_slices(&roi_contours);
    let bounds = rt_struct_bounds(&roi_contours);
    let supported = !roi_contours.is_empty();

    Ok(RtStructData {
        info: RtStructInfo {
            supported,
            unsupported_reason: if supported {
                None
            } else {
                Some("ROI Contour Sequence is missing or contains no contour data".to_string())
            },
            modality,
            structure_set_label: get_text(obj, STRUCTURE_SET_LABEL, text, &[]),
            rois,
            slices,
            bounds,
        },
        contours: roi_contours,
    })
}

pub fn rt_struct_slice_contours_from_data(
    data: &RtStructData,
    z: f64,
    roi_numbers: Vec<i32>,
) -> Result<RtStructSliceContours, String> {
    if !data.info.supported {
        return Err(data
            .info
            .unsupported_reason
            .clone()
            .unwrap_or_else(|| "RT Structure Set is not supported".to_string()));
    }

    let selected_z = data
        .info
        .slices
        .iter()
        .min_by(|left, right| {
            (left.z - z)
                .abs()
                .partial_cmp(&(right.z - z).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|slice| slice.z)
        .ok_or_else(|| "No contour slices are available".to_string())?;
    if roi_numbers.is_empty() {
        return Ok(RtStructSliceContours {
            z: selected_z,
            contours: Vec::new(),
        });
    }

    let roi_filter = roi_numbers.into_iter().collect::<HashSet<_>>();
    let contours = data
        .contours
        .iter()
        .filter(|contour| {
            same_slice_z(contour_slice_z(contour).unwrap_or(selected_z), selected_z)
                && roi_filter.contains(&contour.roi_number)
        })
        .cloned()
        .collect();

    Ok(RtStructSliceContours {
        z: selected_z,
        contours,
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
                    item_sources: None,
                },
                Value::PixelSequence(_) if tag == PIXEL_DATA => DicomNode::Element {
                    tag: tag_text,
                    vr: element.header().vr().to_string().to_owned(),
                    inferred_vr: None,
                    description: description_for(tag),
                    value: "[Binary Data]".to_string(),
                    length: length_to_u32(element.length()),
                    path,
                    editable: false,
                },
                _ => {
                    let vr = element.header().vr();
                    let (value, inferred_vr) = display_element_value(tag, vr, element.value());
                    DicomNode::Element {
                        tag: tag_text,
                        vr: vr.to_string().to_owned(),
                        inferred_vr,
                        description: description_for(tag),
                        value,
                        length: length_to_u32(element.length()),
                        path,
                        editable: tag != PIXEL_DATA && is_text_editable(vr),
                    }
                }
            }
        })
        .collect()
}

#[cfg(test)]
pub fn apply_nodes_to_object(
    obj: &mut InMemDicomObject,
    nodes: &[DicomNode],
) -> Result<(), String> {
    apply_nodes_to_object_with_text(obj, nodes, &TextValues::default(), false).map(|_| ())
}

/// Apply nodes, comparing values that `text` decoded with their decoded form. Returns
/// the paths of the values that were edited or added.
///
/// `nodes_show_trailing` tells whether the nodes were read past Pixel Data's tag (raw
/// data sets without file meta information). Part 10 files are opened up to Pixel
/// Data, so their top-level elements from (7FE0,0010) on are not shown and are kept.
pub fn apply_nodes_to_object_with_text(
    obj: &mut InMemDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    nodes_show_trailing: bool,
) -> Result<HashSet<Vec<String>>, String> {
    if !items_match_object(obj, nodes)? {
        return Err(ITEMS_REMOVED_NEED_SPLICING.to_string());
    }
    validate_private_nodes(obj, nodes)?;
    let mut edited = HashSet::new();
    let keep_trailing = !nodes_show_trailing;
    apply_validated_nodes_to_object(obj, nodes, text, &mut edited, keep_trailing)?;
    Ok(edited)
}

fn apply_validated_nodes_to_object(
    obj: &mut InMemDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    edited: &mut HashSet<Vec<String>>,
    keep_unshown_trailing: bool,
) -> Result<(), String> {
    let desired_tags = nodes
        .iter()
        .map(node_tag)
        .collect::<Result<HashSet<_>, _>>()?;
    let existing_tags = obj.iter().map(|element| element.tag()).collect::<Vec<_>>();
    let has_pixel_data = obj.get(PIXEL_DATA).is_some();
    // Elements from Pixel Data on are kept: in items as before, and at the top level
    // whenever the nodes do not show them (see `apply_nodes_to_object_with_text`).
    let keep_trailing = has_pixel_data || keep_unshown_trailing;

    for tag in existing_tags {
        if keep_trailing && tag >= PIXEL_DATA {
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
                path,
                ..
            } if *editable => {
                let tag = parse_tag(tag)?;
                if tag == PIXEL_DATA {
                    continue;
                }
                let vr = VR::from_str(vr).map_err(|_| format!("Unsupported VR {vr}"))?;
                // Values decoded by Taglet are compared as shown, not with dicom-rs's
                // string, which can differ from what was shown.
                let unchanged = match text.get(path) {
                    Some(read) => {
                        read.decoded == trim_padding(value)
                            && obj.get(tag).is_some_and(|element| element.vr() == vr)
                    }
                    None => element_has_value(obj, tag, vr, value),
                };
                if is_text_editable(vr) && !unchanged {
                    edited.insert(path.clone());
                    if tag == tags::SPECIFIC_CHARACTER_SET {
                        // Keep multiple values, which dicom-rs needs to switch character sets.
                        let values = value.split('\\').map(str::to_string).collect();
                        obj.put(DataElement::new(tag, vr, PrimitiveValue::Strs(values)));
                    } else {
                        obj.put_str(tag, vr, value);
                    }
                }
            }
            DicomNode::Sequence { tag, items, .. } => {
                let tag = parse_tag(tag)?;
                obj.update_value(tag, |value| {
                    if let Some(sequence_items) = value.items_mut() {
                        for (item, next_nodes) in sequence_items.iter_mut().zip(items.iter()) {
                            let _ = apply_validated_nodes_to_object(
                                item, next_nodes, text, edited, false,
                            );
                        }
                    }
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Save `nodes` from the file at `source` to `destination`.
///
/// When the edits only replace values of existing elements in an Explicit or Implicit
/// VR Little Endian file, the original file is copied and only the edited values (and the lengths
/// that change with them) are rewritten, so every other byte is kept (#8). Otherwise
/// the object is written through dicom-rs by `save_object_with_nodes`.
///
/// `opened` is the source file's stamp when `object` and `nodes` were read from it.
/// Without one, the full writer is used. With one, the save fails if the file no longer
/// matches it when it is indexed or just before it is replaced, so that changes made
/// by another program are not overwritten.
///
/// Returns whether `object` now matches the saved file. After a spliced save it is
/// left as it was, and callers should read the file again when they need it.
pub fn save_file_with_nodes(
    source: &str,
    object: &mut DefaultDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    destination: &str,
    opened: Option<splice::FileStamp>,
    nodes_show_trailing: bool,
) -> Result<bool, String> {
    let nodes = &with_file_item_paths(nodes, &[]);
    // Implicit VR files do not store VRs, so readers take them from the dictionary.
    if is_implicit_vr(object) {
        ensure_vrs_match_dictionary(object, nodes)?;
    }
    // Editing signed data would leave signatures that no longer verify.
    if has_digital_signatures(object) && nodes_change_object(object, nodes, text) {
        return Err(
            "Cannot save: this file has digital signatures (Digital Signatures Sequence or MAC Parameters Sequence), which the edits would invalidate."
                .to_string(),
        );
    }
    // Overwriting the source must not replace a file changed since it was opened.
    let guard = opened
        .filter(|_| source == destination)
        .map(|stamp| (source, stamp));
    // Splicing reads the source while writing, also when saving under another name.
    let splice_guard = opened.map(|stamp| (source, stamp));
    // Splicing needs Part 10 files, which are opened up to Pixel Data.
    let planned = match opened.filter(|_| !nodes_show_trailing) {
        Some(stamp) => plan_splice(source, object, nodes, text, stamp)?,
        None => None,
    };
    let Some((index, edits)) = planned else {
        // dicom-rs cannot write deflated data sets.
        if deflate::is_deflated(object.meta().transfer_syntax()) {
            return Err(DEFLATED_NEEDS_SPLICING.to_string());
        }
        // The full writer writes elements in tag order, so in Implicit VR files whose
        // US or SS elements come before Pixel Representation, unchanged ones would
        // read back differently (#15).
        if is_implicit_vr(object) {
            ensure_vrs_match_dictionary_in(object, nodes, &mut None, true).map_err(|error| {
                format!("{error} These changes rewrite the file with its elements in tag order, which changes how this element is read.")
            })?;
        }
        save_object_with_nodes_guarded(
            object,
            nodes,
            text,
            destination,
            guard,
            nodes_show_trailing,
        )?;
        return Ok(true);
    };
    write_file_atomically_with(destination, splice_guard, |out| {
        splice::write_spliced(source, &index, &edits, out)
    })?;
    Ok(false)
}

fn is_implicit_vr(object: &DefaultDicomObject) -> bool {
    object
        .meta()
        .transfer_syntax()
        .trim_end_matches(['\0', ' '])
        == uids::IMPLICIT_VR_LITTLE_ENDIAN
}

/// Fail when an element would be saved to an Implicit VR file with a VR other than the
/// one readers take from the dictionary: its value would read back wrong, and a value
/// read as a sequence would leave a file that cannot be opened. Elements kept with the
/// VR they were read with, and tags not in the dictionary (read back as UN with their
/// bytes kept), are left alone.
fn ensure_vrs_match_dictionary(obj: &InMemDicomObject, nodes: &[DicomNode]) -> Result<(), String> {
    let recheck_unchanged = pixel_representation_changes(obj, nodes)?;
    ensure_vrs_match_dictionary_in(obj, nodes, &mut None, recheck_unchanged)
}

/// Whether the nodes add, remove or change a Pixel Representation (0028,0103) anywhere.
fn pixel_representation_changes(
    obj: &InMemDicomObject,
    nodes: &[DicomNode],
) -> Result<bool, String> {
    let mut shown = false;
    for node in nodes {
        match node {
            DicomNode::Element { tag, vr, value, .. } => {
                if parse_tag(tag)? != tags::PIXEL_REPRESENTATION {
                    continue;
                }
                shown = true;
                let vr = VR::from_str(vr).map_err(|_| format!("Unsupported VR {vr}"))?;
                let unchanged = obj.get(tags::PIXEL_REPRESENTATION).is_some_and(|element| {
                    element.vr() == vr
                        && element
                            .to_str()
                            .is_ok_and(|current| trim_padding(&current) == trim_padding(value))
                });
                if !unchanged {
                    return Ok(true);
                }
            }
            DicomNode::Sequence {
                tag,
                items,
                item_sources,
                ..
            } => {
                let empty = InMemDicomObject::new_empty();
                let obj_items = sequence_items(obj, parse_tag(tag)?).unwrap_or_default();
                // Items removed may carry a Pixel Representation.
                let kept = (0..items.len())
                    .filter_map(|index| item_source(item_sources, index))
                    .collect::<HashSet<_>>();
                if obj_items.iter().enumerate().any(|(index, item)| {
                    !kept.contains(&index) && item.get(tags::PIXEL_REPRESENTATION).is_some()
                }) {
                    return Ok(true);
                }
                for (index, item_nodes) in items.iter().enumerate() {
                    let item = source_item(obj_items, item_sources, index).unwrap_or(&empty);
                    if pixel_representation_changes(item, item_nodes)? {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(!shown && obj.get(tags::PIXEL_REPRESENTATION).is_some())
}

/// `signed_pixel_data` follows the last Pixel Representation (0028,0103) in file order,
/// across items, the way dicom-rs reads US or SS elements (such as Smallest Image Pixel
/// Value) from Implicit VR files: SS after a non-zero value, US otherwise.
///
/// Unchanged US or SS elements are checked only when `recheck_unchanged` (a Pixel
/// Representation changes): otherwise they read back as they were read, also from
/// files whose tags are not in order, which splicing keeps as they are.
fn ensure_vrs_match_dictionary_in(
    obj: &InMemDicomObject,
    nodes: &[DicomNode],
    signed_pixel_data: &mut Option<bool>,
    recheck_unchanged: bool,
) -> Result<(), String> {
    // The saved data set is in tag order.
    let mut ordered = nodes
        .iter()
        .map(|node| {
            let tag = match node {
                DicomNode::Element { tag, .. } | DicomNode::Sequence { tag, .. } => tag,
            };
            parse_tag(tag).map(|tag| (tag, node))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ordered.sort_by_key(|(tag, _)| *tag);
    for (parsed, node) in ordered {
        match node {
            DicomNode::Element {
                vr, value, path, ..
            } => {
                let vr = VR::from_str(vr).map_err(|_| format!("Unsupported VR {vr}"))?;
                let unchanged_vr = obj.get(parsed).is_some_and(|element| element.vr() == vr);
                if parsed == tags::PIXEL_REPRESENTATION {
                    if let Some(first) = value
                        .split('\\')
                        .next()
                        .and_then(|first| first.trim().parse::<u16>().ok())
                    {
                        *signed_pixel_data = Some(first != 0);
                    }
                }
                let Some(entry) = StandardDataDictionary.by_tag(parsed) else {
                    continue;
                };
                // A changed Pixel Representation changes how unchanged US or SS
                // elements read back.
                if unchanged_vr && !(recheck_unchanged && matches!(entry.vr(), VirtualVr::Xs)) {
                    continue;
                }
                let signed = [VR::SS];
                let unsigned = [VR::US];
                let (allowed, reason): (&[VR], &str) = match entry.vr() {
                    VirtualVr::Exact(exact) => (&[exact], ""),
                    VirtualVr::Xs if *signed_pixel_data == Some(true) => {
                        (&signed, " after Pixel Representation 1")
                    }
                    VirtualVr::Xs => (&unsigned, " unless Pixel Representation 1 comes first"),
                    VirtualVr::Ox | VirtualVr::Px => (&[VR::OB, VR::OW], ""),
                    VirtualVr::Lt => (&[VR::US, VR::SS, VR::OW], ""),
                    _ => continue,
                };
                if !allowed.contains(&vr) {
                    return Err(format!(
                        "Cannot save: {} has VR {vr}, but Implicit VR files do not store VRs and readers take it as {}{reason}.",
                        shown_path(path),
                        allowed
                            .iter()
                            .map(|vr| vr.to_string())
                            .collect::<Vec<_>>()
                            .join(" or ")
                    ));
                }
            }
            DicomNode::Sequence {
                items,
                path,
                item_sources,
                ..
            } => {
                let existing = obj
                    .get(parsed)
                    .is_some_and(|element| matches!(element.value(), Value::Sequence(_)));
                let dictionary_vr = StandardDataDictionary
                    .by_tag(parsed)
                    .map(|entry| entry.vr());
                if let Some(dictionary_vr) =
                    dictionary_vr.filter(|vr| !existing && *vr != VirtualVr::Exact(VR::SQ))
                {
                    return Err(format!(
                        "Cannot save: {} is a sequence, but Implicit VR files do not store VRs and readers take it as {}.",
                        shown_path(path),
                        dictionary_vr.relaxed()
                    ));
                }
                let empty = InMemDicomObject::new_empty();
                let obj_items = sequence_items(obj, parsed).unwrap_or_default();
                for (index, item_nodes) in items.iter().enumerate() {
                    ensure_vrs_match_dictionary_in(
                        source_item(obj_items, item_sources, index).unwrap_or(&empty),
                        item_nodes,
                        signed_pixel_data,
                        recheck_unchanged,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Work out the edits for saving by splicing, or `None` when the changes need the
/// full writer.
fn plan_splice(
    source: &str,
    object: &DefaultDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    opened: splice::FileStamp,
) -> Result<Option<(splice::FileIndex, splice::Edits)>, String> {
    if !splice::supports_transfer_syntax(object.meta().transfer_syntax()) {
        return Ok(None);
    }
    // The full writer also repairs file meta UIDs that disagree with the data set.
    if file_meta_needs_sync(object) {
        return Ok(None);
    }
    let Ok(index) = splice::FileIndex::read(source) else {
        return Ok(None);
    };
    if index.stamp() != Some(opened) {
        return Err(FILE_CHANGED_ON_DISK.to_string());
    }
    let Some(diff) = structure_diff(nodes, index.nodes()) else {
        return Ok(None);
    };
    // Validate the nodes and find the edited values, as the full writer does, without
    // copying the object.
    validate_private_nodes(object, nodes)?;
    let mut edited = HashSet::new();
    // Leave nodes that cannot be read here to the full writer, which skips them.
    if collect_edited_paths(object, nodes, text, &mut edited).is_err() {
        return Ok(None);
    }
    // Group lengths (gggg,0000) are updated by the splice writer, but editing or
    // removing one, or removing a SOP UID, is left to the full writer, as before.
    // (Changed SOP UIDs are written to the file meta information too, and a changed
    // Specific Character Set is handled below, by converting the text it applies to.)
    // Group lengths that are not plain 4-byte UL values cannot be updated in place.
    let changes_anything = !edited.is_empty()
        || !diff.added.is_empty()
        || !diff.removed.is_empty()
        || !diff.added_sequences.is_empty()
        || !diff.added_items.is_empty();
    if changes_anything && has_irregular_group_length(index.nodes()) {
        return Ok(None);
    }
    let sop_uid_paths = [
        vec![format_tag(tags::SOP_CLASS_UID)],
        vec![format_tag(tags::SOP_INSTANCE_UID)],
    ];
    if sop_uid_paths.iter().any(|path| diff.removed.contains(path)) {
        return Ok(None);
    }
    let needs_full_writer = |path: &Vec<String>| {
        if path.last().is_some_and(|last| last.starts_with("Item#")) {
            return false;
        }
        let Some(Ok(tag)) = path.last().map(|tag| parse_tag(tag)) else {
            return true;
        };
        tag.element() == 0x0000
    };
    if edited
        .iter()
        .chain(diff.removed.iter())
        .any(needs_full_writer)
    {
        return Ok(None);
    }

    // Added elements are spliced only as edited text values; anything else (e.g. a
    // non-text VR) is left to the full writer rather than dropped. New elements are
    // inserted in tag order, so files whose tags are not in order go to the full
    // writer when elements are added.
    if diff.added.iter().any(|path| !edited.contains(path))
        || ((!diff.added.is_empty() || !diff.added_sequences.is_empty()) && !index.is_ordered())
    {
        return Ok(None);
    }
    // Data sets and items whose character set changes, with the new one. Their text is
    // converted: written again in the new character set, reading the same.
    let mut old_declarations = HashMap::new();
    object_declarations(object, &[], "", &mut old_declarations);
    let mut new_declarations = HashMap::new();
    node_declarations(nodes, &[], "", &mut new_declarations)?;
    let mut converted = HashMap::new();
    for (container, declaration) in &new_declarations {
        // Added items and sequences have no text to convert; their own declarations
        // are checked where they are written (see `encode_new_item`).
        let Some(old) = old_declarations.get(container) else {
            continue;
        };
        let old = Some(old);
        // Declarations Taglet cannot write text in are left to the full writer, as
        // before, also when they read like the old one (e.g. an unknown spelling, read
        // as the default repertoire like no declaration).
        // The same for declarations Taglet does not read as declared (e.g. with a term
        // it does not know), as their text could not be converted.
        if old.map(|old: &String| trim_padding(old)) != Some(trim_padding(declaration))
            && !(is_writable_declaration(declaration)
                && old.is_some_and(|old| is_readable_declaration(old)))
        {
            return Ok(None);
        }
        let charset = TextCharset::from_specific_character_set(declaration);
        if old
            .map(|old| TextCharset::from_specific_character_set(old))
            .as_ref()
            != Some(&charset)
        {
            converted.insert(container.clone(), charset);
        }
    }

    let shown = shown_values(nodes);
    let mut edits = splice::Edits::default();
    let mut failures = Vec::new();
    for path in &edited {
        let Some((node_vr, value)) = shown.get(path) else {
            return Ok(None);
        };
        if diff.added.contains(path) {
            // A new element, encoded with the character set of the data set or item it
            // goes into.
            let Ok(vr) = VR::from_str(node_vr) else {
                return Ok(None);
            };
            let (parent, tag) = path.split_at(path.len() - 1);
            let tag = parse_tag(&tag[0])?;
            if !is_text_editable(vr) || (parent.is_empty() && tag >= PIXEL_DATA) {
                return Ok(None);
            }
            let Some(charset) = converted
                .get(parent)
                .cloned()
                .or_else(|| container_charset(object, parent))
            else {
                return Ok(None);
            };
            match encode_spliced_value(vr, value, Some(&charset)) {
                Ok(Some(bytes)) => {
                    match splice::encode_element(tag, vr, &bytes, index.encoding()) {
                        Ok(element) => edits
                            .insertions
                            .entry(parent.to_vec())
                            .or_default()
                            .push((tag, element)),
                        Err(error) => failures.push(format!("{} ({error})", path.join(" / "))),
                    }
                }
                Ok(None) => return Ok(None),
                Err(reason) => failures.push(format!("{} ({reason})", path.join(" / "))),
            }
            continue;
        }
        let Some(vr) = indexed_element_vr(index.nodes(), path) else {
            return Ok(None);
        };
        // A changed VR (e.g. an element removed and added again) is written by the
        // full writer.
        if *node_vr != vr.to_string() {
            return Ok(None);
        }
        let charset = converted
            .get(&path[..path.len() - 1])
            .or_else(|| text.get(path).map(|read| &read.charset));
        match encode_spliced_value(vr, value, charset) {
            Ok(Some(bytes)) => {
                edits.replacements.insert(path.clone(), bytes);
            }
            Ok(None) => return Ok(None),
            Err(reason) => failures.push(format!("{} ({reason})", path.join(" / "))),
        }
    }
    // The file meta information follows the data set's SOP UIDs (as the full writer
    // does with `sync_file_meta_from_dataset`).
    for (dataset_tag, meta_tag) in [
        (tags::SOP_CLASS_UID, tags::MEDIA_STORAGE_SOP_CLASS_UID),
        (tags::SOP_INSTANCE_UID, tags::MEDIA_STORAGE_SOP_INSTANCE_UID),
    ] {
        let path = vec![format_tag(dataset_tag)];
        if !edited.contains(&path) {
            continue;
        }
        let Some((_, value)) = shown.get(&path) else {
            return Ok(None);
        };
        if !index.has_meta_element(meta_tag) {
            return Ok(None);
        }
        match encode_spliced_value(VR::UI, value, None) {
            Ok(Some(bytes)) => {
                edits.meta_replacements.insert(meta_tag, bytes);
            }
            _ => return Ok(None),
        }
    }
    // Sequences and items added: written whole, their text in the character set in
    // effect where they go.
    let encoding = index.encoding();
    for path in &diff.added_sequences {
        let (parent, last) = path.split_at(path.len() - 1);
        let tag = parse_tag(&last[0])?;
        if parent.is_empty() && tag >= PIXEL_DATA {
            return Ok(None);
        }
        let Some(DicomNode::Sequence {
            items,
            item_sources,
            ..
        }) = node_at(nodes, path)
        else {
            return Ok(None);
        };
        let Some(bytes) = encode_new_sequence(
            tag,
            path,
            items,
            item_sources,
            &new_declarations,
            encoding,
            &mut failures,
        )?
        else {
            return Ok(None);
        };
        edits
            .insertions
            .entry(parent.to_vec())
            .or_default()
            .push((tag, bytes));
    }
    for path in &diff.added_items {
        let Some(DicomNode::Sequence {
            items,
            item_sources,
            ..
        }) = node_at(nodes, path)
        else {
            return Ok(None);
        };
        for (index, item) in items.iter().enumerate() {
            if item_source(item_sources, index).is_some() {
                continue;
            }
            let item_path = child_path(path, &item_segment(item_sources, index));
            let Some(bytes) =
                encode_new_item(item, &item_path, &new_declarations, encoding, &mut failures)?
            else {
                return Ok(None);
            };
            edits
                .appended_items
                .entry(path.clone())
                .or_default()
                .push(bytes);
        }
    }
    let mut conversion_failures = Vec::new();
    if !converted.is_empty() {
        match convert_text(
            index.nodes(),
            text,
            &new_declarations,
            &converted,
            &edited,
            &diff.removed,
        ) {
            Ok(replacements) => edits.replacements.extend(replacements),
            Err(failures) => conversion_failures = failures,
        }
    }
    if !failures.is_empty() || !conversion_failures.is_empty() {
        let mut reasons = Vec::new();
        if !failures.is_empty() {
            failures.sort();
            reasons.push(format!(
                "these text values cannot be saved as shown: {}",
                failures.join("; ")
            ));
        }
        if !conversion_failures.is_empty() {
            conversion_failures.sort();
            reasons.push(format!(
                "changing the Specific Character Set needs these values written in the new character set, which is not possible: {}",
                conversion_failures.join("; ")
            ));
        }
        return Err(format!("Cannot save: {}", reasons.join(". ")));
    }
    edits.removals = diff.removed;
    Ok(Some((index, edits)))
}

/// A node path as shown in messages: added items (`Item#new3`, see `item_segment`)
/// are numbered by their position, like the others.
fn shown_path(path: &[String]) -> String {
    path.iter()
        .map(|segment| match segment.strip_prefix("Item#new") {
            Some(position) => format!("Item#{position} (new)"),
            None => segment.clone(),
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The node at `path`.
fn node_at<'a>(nodes: &'a [DicomNode], path: &[String]) -> Option<&'a DicomNode> {
    nodes.iter().find_map(|node| {
        let (DicomNode::Element {
            path: node_path, ..
        }
        | DicomNode::Sequence {
            path: node_path, ..
        }) = node;
        if node_path == path {
            return Some(node);
        }
        match node {
            DicomNode::Sequence { items, .. } if path.starts_with(node_path) => {
                items.iter().find_map(|item| node_at(item, path))
            }
            _ => None,
        }
    })
}

/// Encode a sequence added by the nodes, of undefined length, with its items.
/// `Ok(None)` when it holds something splicing cannot write (an element of a VR other
/// than text, or a character set Taglet cannot write text in); values that cannot be
/// written are added to `failures`.
fn encode_new_sequence(
    tag: Tag,
    path: &[String],
    items: &[Vec<DicomNode>],
    item_sources: &Option<Vec<Option<usize>>>,
    declarations: &HashMap<Vec<String>, String>,
    encoding: splice::Encoding,
    failures: &mut Vec<String>,
) -> Result<Option<Vec<u8>>, String> {
    let mut out = splice::sequence_start(tag, encoding);
    for (index, item) in items.iter().enumerate() {
        let item_path = child_path(path, &item_segment(item_sources, index));
        let Some(bytes) = encode_new_item(item, &item_path, declarations, encoding, failures)?
        else {
            return Ok(None);
        };
        out.extend(bytes);
    }
    out.extend(splice::sequence_end(encoding));
    Ok(Some(out))
}

/// Encode an item added by the nodes, of undefined length (see `encode_new_sequence`).
fn encode_new_item(
    nodes: &[DicomNode],
    path: &[String],
    declarations: &HashMap<Vec<String>, String>,
    encoding: splice::Encoding,
    failures: &mut Vec<String>,
) -> Result<Option<Vec<u8>>, String> {
    // An item declaring its own character set must declare one Taglet can write.
    for node in nodes {
        if let DicomNode::Element { tag, value, .. } = node {
            if parse_tag(tag)? == tags::SPECIFIC_CHARACTER_SET && !is_writable_declaration(value) {
                return Ok(None);
            }
        }
    }
    let charset = TextCharset::from_specific_character_set(
        declarations.get(path).map(String::as_str).unwrap_or(""),
    );
    let mut sorted = nodes
        .iter()
        .map(|node| {
            let (DicomNode::Element { tag, .. } | DicomNode::Sequence { tag, .. }) = node;
            parse_tag(tag).map(|tag| (tag, node))
        })
        .collect::<Result<Vec<_>, _>>()?;
    sorted.sort_by_key(|(tag, _)| *tag);
    let mut out = splice::item_start(encoding);
    for (tag, node) in sorted {
        match node {
            DicomNode::Element {
                vr, value, path, ..
            } => {
                let Ok(vr) = VR::from_str(vr) else {
                    return Ok(None);
                };
                if !is_text_editable(vr) {
                    return Ok(None);
                }
                let shown = shown_path(path);
                match encode_spliced_value(vr, value, Some(&charset)) {
                    Ok(Some(bytes)) => match splice::encode_element(tag, vr, &bytes, encoding) {
                        Ok(element) => out.extend(element),
                        Err(error) => failures.push(format!("{shown} ({error})")),
                    },
                    // Values of VRs not affected by Specific Character Set must be ASCII.
                    Ok(None) => failures.push(format!(
                        "{shown} (a {vr} value must be ASCII, e.g. half-width digits)"
                    )),
                    Err(reason) => failures.push(format!("{shown} ({reason})")),
                }
            }
            DicomNode::Sequence {
                path,
                items,
                item_sources,
                ..
            } => {
                let Some(bytes) = encode_new_sequence(
                    tag,
                    path,
                    items,
                    item_sources,
                    declarations,
                    encoding,
                    failures,
                )?
                else {
                    return Ok(None);
                };
                out.extend(bytes);
            }
        }
    }
    out.extend(splice::item_end(encoding));
    Ok(Some(out))
}

/// The Specific Character Set value in effect for the data set and each item in
/// `obj`, by container path (empty for the data set).
fn object_declarations(
    obj: &InMemDicomObject,
    parent: &[String],
    inherited: &str,
    out: &mut HashMap<Vec<String>, String>,
) {
    let declaration = match obj.get(tags::SPECIFIC_CHARACTER_SET) {
        Some(element) => element
            .to_str()
            .map(|value| value.to_string())
            .unwrap_or_default(),
        None => inherited.to_string(),
    };
    for element in obj.iter() {
        if let Value::Sequence(sequence) = element.value() {
            for (index, item) in sequence.items().iter().enumerate() {
                let mut item_path = parent.to_vec();
                item_path.push(format_tag(element.tag()));
                item_path.push(format!("Item#{index}"));
                object_declarations(item, &item_path, &declaration, out);
            }
        }
    }
    out.insert(parent.to_vec(), declaration);
}

/// The Specific Character Set value in effect for the data set and each item as saved
/// from `nodes`, by container path (empty for the data set).
fn node_declarations(
    nodes: &[DicomNode],
    parent: &[String],
    inherited: &str,
    out: &mut HashMap<Vec<String>, String>,
) -> Result<(), String> {
    let mut declaration = inherited.to_string();
    for node in nodes {
        if let DicomNode::Element { tag, value, .. } = node {
            if parse_tag(tag)? == tags::SPECIFIC_CHARACTER_SET {
                declaration = value.clone();
            }
        }
    }
    for node in nodes {
        if let DicomNode::Sequence {
            path,
            items,
            item_sources,
            ..
        } = node
        {
            for (index, item) in items.iter().enumerate() {
                let mut item_path = path.clone();
                item_path.push(item_segment(item_sources, index));
                node_declarations(item, &item_path, &declaration, out)?;
            }
        }
    }
    out.insert(parent.to_vec(), declaration);
    Ok(())
}

/// Encode the text values of data sets and items whose character set changes in their
/// new character set (`converted`, by container path), so that they read the same.
/// Values that are edited (encoded with the new character set by the caller) or
/// removed are skipped. Fails with the values that cannot be converted.
///
/// `read` holds the data set and items that were read (as keys); items that were not
/// (in sequences after Pixel Data) take their character set from the nearest one that
/// was, so their text counts as unread when that one is converted.
fn convert_text(
    index: &[splice::IndexNode],
    text: &TextValues,
    read: &HashMap<Vec<String>, String>,
    converted: &HashMap<Vec<String>, TextCharset>,
    edited: &HashSet<Vec<String>>,
    removed: &HashSet<Vec<String>>,
) -> Result<HashMap<Vec<String>, Vec<u8>>, Vec<String>> {
    fn indexed_elements(
        nodes: &[splice::IndexNode],
        parent: &[String],
        out: &mut Vec<(Vec<String>, Option<VR>)>,
    ) {
        for node in nodes {
            match node {
                splice::IndexNode::Element { tag, vr, .. } => {
                    let mut path = parent.to_vec();
                    path.push(format_tag(*tag));
                    out.push((path, *vr));
                }
                splice::IndexNode::Sequence { tag, items, .. } => {
                    for (index, item) in items.iter().enumerate() {
                        let mut item_path = parent.to_vec();
                        item_path.push(format_tag(*tag));
                        item_path.push(format!("Item#{index}"));
                        indexed_elements(item.nodes(), &item_path, out);
                    }
                }
                splice::IndexNode::Opaque { .. } => {}
            }
        }
    }
    let mut elements = Vec::new();
    indexed_elements(index, &[], &mut elements);
    let mut replacements = HashMap::new();
    let mut failures = Vec::new();
    for (path, vr) in elements {
        // Elements of unknown VR (e.g. private elements in Implicit VR files, read as
        // UN) are bytes to Taglet, so they are kept as they are.
        let Some(vr) = vr.filter(|vr| is_charset_sensitive(*vr)) else {
            continue;
        };
        // Elements removed, or in a removed sequence or item, are not written.
        if (1..=path.len()).any(|end| removed.contains(&path[..end])) {
            continue;
        }
        let shown = path.join(" / ");
        let mut container = &path[..path.len() - 1];
        while !read.contains_key(container) && container.len() >= 2 {
            container = &container[..container.len() - 2];
        }
        let Some(charset) = converted.get(container) else {
            continue;
        };
        if !read.contains_key(&path[..path.len() - 1]) {
            failures.push(format!(
                "{shown} (it is not read, so it cannot be converted)"
            ));
            continue;
        }
        if edited.contains(&path) {
            continue;
        }
        let Some(read) = text.get(&path).filter(|read| read.vr == vr) else {
            // E.g. text after Pixel Data, which is not read.
            failures.push(format!(
                "{shown} (it is not read, so it cannot be converted)"
            ));
            continue;
        };
        let Some(decoded) = read.charset.decode_strict(vr, &read.bytes) else {
            failures.push(format!(
                "{shown} (its bytes are not valid in the current character set, so it cannot be converted)"
            ));
            continue;
        };
        match encode_spliced_value(vr, &decoded, Some(charset)) {
            Ok(Some(bytes)) if bytes == read.bytes => {}
            Ok(Some(bytes)) => {
                replacements.insert(path, bytes);
            }
            Ok(None) => failures.push(format!("{shown} (it cannot be converted)")),
            Err(reason) => failures.push(format!("{shown} ({reason})")),
        }
    }
    if failures.is_empty() {
        Ok(replacements)
    } else {
        Err(failures)
    }
}

const ITEMS_REMOVED_NEED_SPLICING: &str = "Cannot save: adding sequences or items, or removing items, needs editing the file in place, which this file or these changes do not allow (it needs a Part 10 file in Explicit or Implicit VR Little Endian whose file meta SOP UIDs match the data set and whose tags are in order when sequences are added, new items holding only text elements, and a save that removes no SOP UIDs, does not edit or remove group lengths, and removes no items holding encapsulated data).";

/// The index of the file's item that the node item at `index` shows (see
/// `DicomNode::Sequence::item_sources`).
pub(crate) fn item_source(
    item_sources: &Option<Vec<Option<usize>>>,
    index: usize,
) -> Option<usize> {
    match item_sources {
        Some(sources) => sources.get(index).copied().flatten(),
        None => Some(index),
    }
}

/// The path segment of the node item at `index`: the file's numbering, or a name of
/// its own for an item added since (see `with_file_item_paths`).
fn item_segment(item_sources: &Option<Vec<Option<usize>>>, index: usize) -> String {
    match item_source(item_sources, index) {
        Some(source) => format!("Item#{source}"),
        None => format!("Item#new{index}"),
    }
}

/// The file's item that the node item at `index` shows.
pub(crate) fn source_item<'a>(
    file_items: &'a [InMemDicomObject],
    item_sources: &Option<Vec<Option<usize>>>,
    index: usize,
) -> Option<&'a InMemDicomObject> {
    file_items.get(item_source(item_sources, index)?)
}

/// The nodes with their paths numbering items as the file does, following
/// `item_sources`: after an item is removed, the shown paths of later items follow
/// their new positions, while text values, the file index and the object address them
/// by their place in the file.
///
/// Nodes whose paths do not follow their positions (e.g. items moved without
/// `item_sources`) are returned as they are, so that `structure_diff` notices.
pub(crate) fn with_file_item_paths(nodes: &[DicomNode], parent: &[String]) -> Vec<DicomNode> {
    if parent.is_empty() && !paths_follow_positions(nodes, &[]) {
        return nodes.to_vec();
    }
    nodes
        .iter()
        .map(|node| match node {
            DicomNode::Element { tag, .. } => {
                let mut node = node.clone();
                if let DicomNode::Element { path, .. } = &mut node {
                    *path = child_path(parent, tag);
                }
                node
            }
            DicomNode::Sequence {
                tag,
                description,
                length,
                items,
                item_sources,
                ..
            } => {
                let path = child_path(parent, tag);
                let items = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let item_path = child_path(&path, &item_segment(item_sources, index));
                        with_file_item_paths(item, &item_path)
                    })
                    .collect();
                DicomNode::Sequence {
                    tag: tag.clone(),
                    description: description.clone(),
                    length: *length,
                    path,
                    items,
                    item_sources: item_sources.clone(),
                }
            }
        })
        .collect()
}

/// Whether every node's path is its parent's path and its tag, items numbered by
/// their position.
fn paths_follow_positions(nodes: &[DicomNode], parent: &[String]) -> bool {
    nodes.iter().all(|node| {
        let (DicomNode::Element { tag, path, .. } | DicomNode::Sequence { tag, path, .. }) = node;
        *path == child_path(parent, tag)
            && match node {
                DicomNode::Element { .. } => true,
                DicomNode::Sequence { items, .. } => {
                    items.iter().enumerate().all(|(index, item)| {
                        paths_follow_positions(item, &child_path(path, &format!("Item#{index}")))
                    })
                }
            }
    })
}

fn child_path(parent: &[String], segment: &str) -> Vec<String> {
    let mut path = parent.to_vec();
    path.push(segment.to_string());
    path
}

/// Whether no sequences were added, and no items removed from or added to the
/// object's: where `item_sources` is set,
/// it must list the object's items in order (removing the last items, or all of them,
/// leaves a shorter list). Without it, the items are the ones read, which can be fewer
/// in raw data sets read up to Pixel Data in an item.
fn items_match_object(obj: &InMemDicomObject, nodes: &[DicomNode]) -> Result<bool, String> {
    for node in nodes {
        let DicomNode::Sequence {
            tag,
            items,
            item_sources,
            ..
        } = node
        else {
            continue;
        };
        // A sequence the object does not have was added.
        let Some(obj_items) = sequence_items(obj, parse_tag(tag)?) else {
            return Ok(false);
        };
        if let Some(sources) = item_sources {
            if sources.len() != obj_items.len()
                || sources
                    .iter()
                    .enumerate()
                    .any(|(index, source)| *source != Some(index))
            {
                return Ok(false);
            }
        }
        for (item, item_nodes) in obj_items.iter().zip(items) {
            if !items_match_object(item, item_nodes)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Elements added by the nodes, sequences and items added (written whole), and
/// elements, sequences and items removed from the file (only the outermost of what is
/// removed).
struct StructureDiff {
    added: HashSet<Vec<String>>,
    /// Sequences not in the file, by path.
    added_sequences: HashSet<Vec<String>>,
    /// Sequences in the file with items added after their own (see `item_sources`).
    added_items: HashSet<Vec<String>>,
    removed: HashSet<Vec<String>>,
}

/// Compare the nodes with the file. Plain elements may be added; elements, sequences
/// and items may be removed. `None` when sequences or items are added. Items are
/// matched by their path, which follows `item_sources` (see `with_file_item_paths`).
/// Top-level elements from Pixel Data (7FE0,0010) on are never shown as nodes (the
/// file is opened up to Pixel Data), so the file's are left out; nodes there other than
/// Pixel Data's placeholder were added by the user and count as added.
fn structure_diff(nodes: &[DicomNode], index: &[splice::IndexNode]) -> Option<StructureDiff> {
    #[derive(Default)]
    struct Shape {
        elements: HashSet<Vec<String>>,
        sequences: HashSet<Vec<String>>,
        items: HashSet<Vec<String>>,
        /// Item counts of sequences: from the file, and from nodes without
        /// `item_sources`, which show the file's items as they are.
        item_counts: HashMap<Vec<String>, usize>,
        opaque: HashSet<Vec<String>>,
        added_sequences: HashSet<Vec<String>>,
        added_items: HashSet<Vec<String>>,
        /// Set when a node's path does not match where it is. Paths address the
        /// file's elements, so splicing could then copy values from the wrong item.
        inconsistent: bool,
    }

    /// `file_sequences`: the file's sequences; other sequence nodes were added, and
    /// are written whole, like added items.
    fn node_shape(
        nodes: &[DicomNode],
        parent: &[String],
        file_sequences: &HashSet<Vec<String>>,
        shape: &mut Shape,
    ) {
        for node in nodes {
            let (DicomNode::Element { tag, path, .. } | DicomNode::Sequence { tag, path, .. }) =
                node;
            if parent.is_empty() && parse_tag(tag).is_ok_and(|tag| tag == PIXEL_DATA) {
                continue;
            }
            if path.len() != parent.len() + 1
                || path[..parent.len()] != *parent
                || path[parent.len()] != *tag
            {
                shape.inconsistent = true;
            }
            match node {
                DicomNode::Element { .. } => {
                    shape.elements.insert(path.clone());
                }
                DicomNode::Sequence {
                    items,
                    item_sources,
                    ..
                } => {
                    if !file_sequences.contains(path) {
                        shape.added_sequences.insert(path.clone());
                        continue;
                    }
                    shape.sequences.insert(path.clone());
                    if item_sources.is_none() {
                        shape.item_counts.insert(path.clone(), items.len());
                    }
                    if let Some(sources) = item_sources {
                        // Items can be removed and added at the end, not reordered:
                        // splicing writes the file's items in their order, then the
                        // new ones.
                        let file_items = sources.iter().take_while(|source| source.is_some());
                        let new_items = sources.len() - file_items.clone().count();
                        if sources[sources.len() - new_items..]
                            .iter()
                            .any(Option::is_some)
                            || file_items
                                .flatten()
                                .collect::<Vec<_>>()
                                .windows(2)
                                .any(|pair| pair[0] >= pair[1])
                        {
                            shape.inconsistent = true;
                        }
                        if new_items > 0 {
                            shape.added_items.insert(path.clone());
                        }
                    }
                    for (index, item) in items.iter().enumerate() {
                        if item_source(item_sources, index).is_none() {
                            // Added: written whole.
                            continue;
                        }
                        let mut item_path = path.clone();
                        item_path.push(item_segment(item_sources, index));
                        if !shape.items.insert(item_path.clone()) {
                            shape.inconsistent = true;
                        }
                        node_shape(item, &item_path, file_sequences, shape);
                    }
                }
            }
        }
    }

    fn index_shape(nodes: &[splice::IndexNode], parent: &[String], shape: &mut Shape) {
        for node in nodes {
            let tag = node.tag();
            if parent.is_empty() && tag >= PIXEL_DATA {
                continue;
            }
            let mut path = parent.to_vec();
            path.push(format_tag(tag));
            match node {
                splice::IndexNode::Sequence { items, .. } => {
                    shape.sequences.insert(path.clone());
                    shape.item_counts.insert(path.clone(), items.len());
                    for (index, item) in items.iter().enumerate() {
                        let mut item_path = path.clone();
                        item_path.push(format!("Item#{index}"));
                        shape.items.insert(item_path.clone());
                        index_shape(item.nodes(), &item_path, shape);
                    }
                }
                splice::IndexNode::Element { .. } => {
                    shape.elements.insert(path);
                }
                splice::IndexNode::Opaque { .. } => {
                    shape.opaque.insert(path);
                }
            }
        }
    }

    let mut from_index = Shape::default();
    index_shape(index, &[], &mut from_index);
    let mut from_nodes = Shape::default();
    node_shape(nodes, &[], &from_index.sequences, &mut from_nodes);
    if from_nodes.inconsistent {
        return None;
    }
    // Opaque values (e.g. encapsulated pixel data in an item) show up as plain element
    // nodes, and cannot be removed here.
    for path in &from_index.opaque {
        if !from_nodes.elements.remove(path) {
            return None;
        }
    }
    // Items are only removed as `item_sources` records it; a sequence without it must
    // show all of the file's items.
    if from_nodes
        .item_counts
        .iter()
        .any(|(path, count)| from_index.item_counts.get(path) != Some(count))
    {
        return None;
    }
    if !from_nodes.sequences.is_subset(&from_index.sequences)
        || !from_nodes.items.is_subset(&from_index.items)
        || !from_nodes
            .elements
            .iter()
            .all(|path| !from_index.sequences.contains(path))
    {
        return None;
    }
    let mut removed = from_index
        .elements
        .difference(&from_nodes.elements)
        .chain(from_index.sequences.difference(&from_nodes.sequences))
        .chain(from_index.items.difference(&from_nodes.items))
        .cloned()
        .collect::<HashSet<_>>();
    // Only the outermost of what is removed.
    let nested = removed
        .iter()
        .filter(|path| (1..path.len()).any(|end| removed.contains(&path[..end])))
        .cloned()
        .collect::<Vec<_>>();
    for path in nested {
        removed.remove(&path);
    }
    Some(StructureDiff {
        added: from_nodes
            .elements
            .difference(&from_index.elements)
            .cloned()
            .collect(),
        added_sequences: from_nodes.added_sequences,
        added_items: from_nodes.added_items,
        removed,
    })
}

/// The character set in effect for the data set or item at `parent` (a node path to an
/// item, or empty for the top level). `None` if the path does not lead to an item.
fn container_charset(obj: &InMemDicomObject, parent: &[String]) -> Option<TextCharset> {
    let own = |obj: &InMemDicomObject| {
        obj.get(tags::SPECIFIC_CHARACTER_SET).map(|element| {
            element
                .to_str()
                .map(|value| TextCharset::from_specific_character_set(&value))
                .unwrap_or_default()
        })
    };
    let mut charset = own(obj).unwrap_or_default();
    let mut current = obj;
    for pair in parent.chunks(2) {
        let [sequence, item] = pair else {
            return None;
        };
        let index = item.strip_prefix("Item#")?.parse::<usize>().ok()?;
        current = sequence_items(current, parse_tag(sequence).ok()?)?.get(index)?;
        if let Some(item_charset) = own(current) {
            charset = item_charset;
        }
    }
    Some(charset)
}

/// Encode an edited value as it is written in the file, padded to an even length.
/// `Ok(None)` when the full writer has to handle it.
fn encode_spliced_value(
    vr: VR,
    value: &str,
    charset: Option<&TextCharset>,
) -> Result<Option<Vec<u8>>, String> {
    let entered = trim_padding(value);
    let mut bytes = if is_charset_sensitive(vr) {
        let Some(charset) = charset else {
            return Ok(None);
        };
        let bytes = charset
            .encode(vr, entered)
            .map_err(|error| error.to_string())?;
        if charset.decode(vr, &bytes) != entered {
            return Err("it would read back differently".to_string());
        }
        bytes
    } else if entered.is_ascii() {
        entered.as_bytes().to_vec()
    } else {
        // Leave non-ASCII text in these VRs to the full writer, as before.
        return Ok(None);
    };
    if bytes.len() % 2 == 1 {
        bytes.push(if vr == VR::UI { 0 } else { b' ' });
    }
    Ok(Some(bytes))
}

/// The VR and value shown for each element node, by path.
fn shown_values(nodes: &[DicomNode]) -> HashMap<Vec<String>, (String, String)> {
    let mut values = HashMap::new();
    for node in nodes {
        match node {
            DicomNode::Element {
                path, value, vr, ..
            } => {
                values.insert(path.clone(), (vr.clone(), value.clone()));
            }
            DicomNode::Sequence { items, .. } => {
                for item in items {
                    values.extend(shown_values(item));
                }
            }
        }
    }
    values
}

/// Whether the data set, or any item in it, has a Digital Signatures Sequence
/// (FFFA,FFFA) or a MAC Parameters Sequence (4FFE,0001).
fn has_digital_signatures(obj: &InMemDicomObject) -> bool {
    const DIGITAL_SIGNATURES_SEQUENCE: Tag = Tag(0xFFFA, 0xFFFA);
    const MAC_PARAMETERS_SEQUENCE: Tag = Tag(0x4FFE, 0x0001);
    obj.iter().any(|element| {
        let tag = element.tag();
        tag == DIGITAL_SIGNATURES_SEQUENCE
            || tag == MAC_PARAMETERS_SEQUENCE
            || matches!(element.value(), Value::Sequence(sequence)
                if sequence.items().iter().any(has_digital_signatures))
    })
}

/// Whether saving `nodes` would change `obj`: an edited value, or an element or item
/// added or removed. Nodes that cannot be read count as changes.
fn nodes_change_object(obj: &InMemDicomObject, nodes: &[DicomNode], text: &TextValues) -> bool {
    fn object_shape(obj: &InMemDicomObject, parent: &[String], out: &mut Vec<String>) {
        for element in obj.iter() {
            let tag = element.tag();
            if parent.is_empty() && tag >= PIXEL_DATA {
                continue;
            }
            let mut path = parent.to_vec();
            path.push(format_tag(tag));
            match element.value() {
                Value::Sequence(sequence) => {
                    out.push(format!("{}#{}", path.join("/"), sequence.items().len()));
                    for (index, item) in sequence.items().iter().enumerate() {
                        let mut item_path = path.clone();
                        item_path.push(format!("Item#{index}"));
                        object_shape(item, &item_path, out);
                    }
                }
                _ => out.push(path.join("/")),
            }
        }
    }
    fn node_shape(nodes: &[DicomNode], root: bool, out: &mut Vec<String>) {
        for node in nodes {
            let (DicomNode::Element { tag, path, .. } | DicomNode::Sequence { tag, path, .. }) =
                node;
            // Only Pixel Data's placeholder stands for something not shown; other nodes
            // after its tag were added.
            if root && parse_tag(tag).is_ok_and(|tag| tag == PIXEL_DATA) {
                continue;
            }
            match node {
                DicomNode::Element { .. } => out.push(path.join("/")),
                DicomNode::Sequence { items, .. } => {
                    out.push(format!("{}#{}", path.join("/"), items.len()));
                    for item in items {
                        node_shape(item, false, out);
                    }
                }
            }
        }
    }
    let mut edited = HashSet::new();
    if collect_edited_paths(obj, nodes, text, &mut edited).is_err() || !edited.is_empty() {
        return true;
    }
    let mut from_object = Vec::new();
    object_shape(obj, &[], &mut from_object);
    let mut from_nodes = Vec::new();
    node_shape(nodes, true, &mut from_nodes);
    from_object.sort();
    from_nodes.sort();
    from_object != from_nodes
}

/// Whether saving through the full writer would change the file meta information,
/// which `sync_file_meta_from_dataset` updates from the data set's SOP UIDs.
fn file_meta_needs_sync(object: &DefaultDicomObject) -> bool {
    let meta = object.meta();
    let differs = |dataset: Option<String>, meta_value: &str| {
        dataset.is_some_and(|uid| uid != meta_value.trim_end_matches(['\0', ' ']))
    };
    differs(
        get_uid_string(object, tags::SOP_CLASS_UID),
        &meta.media_storage_sop_class_uid,
    ) || differs(
        get_uid_string(object, tags::SOP_INSTANCE_UID),
        &meta.media_storage_sop_instance_uid,
    )
}

/// The paths of editable values in `nodes` that differ from `obj`, compared the same
/// way as when applying nodes, without changing `obj`.
fn collect_edited_paths(
    obj: &InMemDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    edited: &mut HashSet<Vec<String>>,
) -> Result<(), String> {
    for node in nodes {
        match node {
            DicomNode::Element {
                tag,
                vr,
                value,
                editable,
                path,
                ..
            } if *editable => {
                let tag = parse_tag(tag)?;
                if tag == PIXEL_DATA {
                    continue;
                }
                let vr = VR::from_str(vr).map_err(|_| format!("Unsupported VR {vr}"))?;
                let unchanged = match text.get(path) {
                    Some(read) => {
                        read.decoded == trim_padding(value)
                            && obj.get(tag).is_some_and(|element| element.vr() == vr)
                    }
                    None => element_has_value(obj, tag, vr, value),
                };
                if is_text_editable(vr) && !unchanged {
                    edited.insert(path.clone());
                }
            }
            DicomNode::Sequence {
                tag,
                items,
                item_sources,
                ..
            } => {
                let tag = parse_tag(tag)?;
                if let Some(obj_items) = sequence_items(obj, tag) {
                    for (index, item_nodes) in items.iter().enumerate() {
                        if let Some(item) = source_item(obj_items, item_sources, index) {
                            collect_edited_paths(item, item_nodes, text, edited)?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether the index has a group length element (gggg,0000) that is not a plain
/// 4-byte UL value (see `splice::is_plain_group_length`).
fn has_irregular_group_length(nodes: &[splice::IndexNode]) -> bool {
    nodes.iter().any(|node| match node {
        splice::IndexNode::Element { tag, .. } => {
            tag.element() == 0x0000 && !splice::is_plain_group_length(node)
        }
        splice::IndexNode::Sequence { items, .. } => items
            .iter()
            .any(|item| has_irregular_group_length(item.nodes())),
        splice::IndexNode::Opaque { .. } => false,
    })
}

/// The VR of the plain element at `path` in the index.
fn indexed_element_vr(nodes: &[splice::IndexNode], path: &[String]) -> Option<VR> {
    let (first, rest) = path.split_first()?;
    let node = nodes.iter().find(|node| match node {
        splice::IndexNode::Element { tag, .. }
        | splice::IndexNode::Sequence { tag, .. }
        | splice::IndexNode::Opaque { tag, .. } => format_tag(*tag) == *first,
    })?;
    match (node, rest) {
        (splice::IndexNode::Element { vr, .. }, []) => *vr,
        (splice::IndexNode::Sequence { items, .. }, [item, rest @ ..]) => {
            let index = item.strip_prefix("Item#")?.parse::<usize>().ok()?;
            indexed_element_vr(items.get(index)?.nodes(), rest)
        }
        _ => None,
    }
}

/// Apply nodes to a copy of `object`, check the text values and write the copy to
/// `destination`. `object` is updated only when the file was written.
#[cfg(test)]
pub fn save_object_with_nodes(
    object: &mut DefaultDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    destination: &str,
) -> Result<(), String> {
    save_object_with_nodes_guarded(object, nodes, text, destination, None, false)
}

/// The message for a file changed by another program since it was opened.
pub const FILE_CHANGED_ON_DISK: &str = "The file was changed by another program after it was opened. Open it again before saving, so that those changes are not overwritten.";

/// `save_object_with_nodes`, failing before the destination is replaced when the file
/// at `guard`'s path no longer has `guard`'s stamp.
fn save_object_with_nodes_guarded(
    object: &mut DefaultDicomObject,
    nodes: &[DicomNode],
    text: &TextValues,
    destination: &str,
    guard: Option<(&str, splice::FileStamp)>,
    nodes_show_trailing: bool,
) -> Result<(), String> {
    let mut candidate = object.clone();
    let edited = apply_nodes_to_object_with_text(&mut candidate, nodes, text, nodes_show_trailing)?;
    prepare_text_for_writing(&mut candidate, text, &edited)?;
    sync_file_meta_from_dataset(&mut candidate);
    let explicit_vr = candidate.meta().transfer_syntax() != uids::IMPLICIT_VR_LITTLE_ENDIAN;
    ensure_value_lengths_fit(&candidate, explicit_vr)?;
    write_file_atomically(&candidate, destination, guard)?;
    *object = candidate;
    Ok(())
}

/// Write to a new temporary file next to `destination` and move it into place, so that
/// a failed write never leaves `destination` truncated or replaced by a partial file.
fn write_file_atomically(
    object: &DefaultDicomObject,
    destination: &str,
    guard: Option<(&str, splice::FileStamp)>,
) -> Result<(), String> {
    write_file_atomically_with(destination, guard, |out| {
        object.write_all(out).map_err(|error| error.to_string())
    })
}

/// Like `write_file_atomically`, with the file content written by `write`.
fn write_file_atomically_with(
    destination: &str,
    guard: Option<(&str, splice::FileStamp)>,
    write: impl FnOnce(&mut dyn std::io::Write) -> Result<(), String>,
) -> Result<(), String> {
    let (temporary, file) = create_temporary_save_file(destination)?;
    let result = write_checked(file, write)
        .and_then(|()| match guard {
            Some((path, stamp)) if splice::file_stamp(path) != Some(stamp) => {
                Err(FILE_CHANGED_ON_DISK.to_string())
            }
            _ => Ok(()),
        })
        .and_then(|()| copy_existing_permissions(destination, &temporary))
        .and_then(|()| std::fs::rename(&temporary, destination).map_err(|error| error.to_string()));
    if result.is_err() {
        std::fs::remove_file(&temporary).ok();
    }
    result
}

/// Create a temporary file that only this save uses, readable by the owner only.
fn create_temporary_save_file(destination: &str) -> Result<(std::path::PathBuf, File), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let destination = std::path::Path::new(destination);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    loop {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        // A short name independent of the destination's, so that long file names
        // still fit the file system's limit.
        let temporary = destination.with_file_name(format!(".taglet-{unique}.tmp"));
        match options.open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Write a Part 10 file and make sure every byte reached the disk. dicom-rs drops its
/// internal buffer without flushing it explicitly, so errors from the final write are
/// caught by `ErrorRecordingWriter` instead.
fn write_checked(
    file: File,
    write: impl FnOnce(&mut dyn std::io::Write) -> Result<(), String>,
) -> Result<(), String> {
    let mut writer = ErrorRecordingWriter {
        inner: std::io::BufWriter::new(file),
        error: None,
    };
    let result = write(&mut writer)
        .and_then(|()| std::io::Write::flush(&mut writer).map_err(|error| error.to_string()));
    if let Some(error) = writer.error.take() {
        return Err(error);
    }
    result?;
    let file = writer
        .inner
        .into_inner()
        .map_err(|error| error.error().to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

struct ErrorRecordingWriter<W> {
    inner: W,
    error: Option<String>,
}

impl<W: std::io::Write> std::io::Write for ErrorRecordingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf).inspect_err(|error| {
            self.error.get_or_insert_with(|| error.to_string());
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().inspect_err(|error| {
            self.error.get_or_insert_with(|| error.to_string());
        })
    }
}

/// Keep the access permissions of a file that is being replaced.
fn copy_existing_permissions(destination: &str, temporary: &std::path::Path) -> Result<(), String> {
    match std::fs::metadata(destination) {
        Ok(metadata) => std::fs::set_permissions(temporary, metadata.permissions())
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn sync_file_meta_from_dataset(obj: &mut DefaultDicomObject) {
    let sop_class_uid = get_uid_string(obj, tags::SOP_CLASS_UID);
    let sop_instance_uid = get_uid_string(obj, tags::SOP_INSTANCE_UID);
    let meta = obj.meta_mut();

    if let Some(uid) = sop_class_uid {
        meta.media_storage_sop_class_uid = ui_padded(uid);
    }
    if let Some(uid) = sop_instance_uid {
        meta.media_storage_sop_instance_uid = ui_padded(uid);
    }
    meta.update_information_group_length();
}

fn file_meta_to_nodes(meta: &FileMetaTable) -> Vec<DicomNode> {
    meta.to_element_iter()
        .map(|element| {
            let tag = element.tag();
            let tag_text = format_tag(tag);
            let value = element
                .value()
                .to_str()
                .map(|value| {
                    value
                        .trim_end_matches(|c| c == '\0' || c == ' ')
                        .to_string()
                })
                .unwrap_or_else(|_| "[Binary Data]".to_string());

            DicomNode::Element {
                tag: tag_text.clone(),
                vr: element.header().vr().to_string().to_owned(),
                inferred_vr: None,
                description: description_for(tag),
                value,
                length: length_to_u32(element.length()),
                path: vec!["FileMetaInformation".to_string(), tag_text],
                editable: false,
            }
        })
        .collect()
}

/// A Deflated Explicit VR Little Endian file, read with its data set inflated (dicom-rs
/// cannot read these), or `None` for other files (see `deflate`). With
/// `read_until_pixel_data`, it is read up to Pixel Data as `OpenFileOptions::read_until`
/// does: stopping where Pixel Data starts in the file, so that what follows is neither
/// shown nor needs to be readable.
fn open_deflated_object(
    path: &str,
    read_until_pixel_data: bool,
) -> Result<Option<DefaultDicomObject>, String> {
    let Some(file) = deflate::read_inflated(path)? else {
        return Ok(None);
    };
    let transfer_syntax = TransferSyntaxRegistry
        .get(uids::EXPLICIT_VR_LITTLE_ENDIAN)
        .ok_or_else(|| "Explicit VR Little Endian transfer syntax is not available".to_string())?;
    let mut dataset = file.dataset();
    // When the elements cannot be walked (e.g. repeated tags), the whole data set is
    // read and the elements from Pixel Data on are left out.
    let cut = read_until_pixel_data
        .then(|| splice::explicit_le_offset_of(dataset, PIXEL_DATA).ok())
        .flatten();
    if let Some(end) = cut {
        dataset = &dataset[..end];
    }
    let mut obj = InMemDicomObject::read_dataset_with_ts(dataset, transfer_syntax)
        .map_err(|error| format!("Could not read the deflated data set: {error}"))?;
    if read_until_pixel_data && cut.is_none() {
        remove_from_pixel_data(&mut obj);
    }
    Ok(Some(obj.with_exact_meta(file.meta)))
}

/// Leave out Pixel Data and the elements after it, as `OpenFileOptions::read_until`
/// does.
fn remove_from_pixel_data(obj: &mut InMemDicomObject) {
    let from_pixel_data = obj
        .iter()
        .map(|element| element.tag())
        .filter(|tag| *tag >= PIXEL_DATA)
        .collect::<Vec<_>>();
    for tag in from_pixel_data {
        obj.remove_element(tag);
    }
}

fn open_object_with_fallback(
    path: &str,
    read_until_pixel_data: bool,
) -> Result<(DefaultDicomObject, bool), String> {
    if let Some(obj) = open_deflated_object(path, read_until_pixel_data)? {
        return Ok((obj, false));
    }
    let options = if read_until_pixel_data {
        OpenFileOptions::new().read_until(tags::PIXEL_DATA)
    } else {
        OpenFileOptions::new()
    };

    match options.open_file(path) {
        Ok(obj) => Ok((obj, false)),
        Err(standard_error) => read_raw_implicit_vr_little_endian(path)
            .map(|obj| (obj, true))
            .map_err(|raw_error| {
                format!(
                    "Could not read as DICOM Part 10 ({standard_error}) or raw Implicit VR Little Endian dataset ({raw_error})"
                )
            }),
    }
}

fn read_raw_implicit_vr_little_endian(path: &str) -> Result<DefaultDicomObject, String> {
    ensure_raw_dataset_candidate(path)?;
    let file = File::open(path).map_err(|error| error.to_string())?;
    let transfer_syntax = TransferSyntaxRegistry
        .get(uids::IMPLICIT_VR_LITTLE_ENDIAN)
        .ok_or_else(|| "Implicit VR Little Endian transfer syntax is not available".to_string())?;
    let obj = InMemDicomObject::read_dataset_with_ts(BufReader::new(file), transfer_syntax)
        .map_err(|error| error.to_string())?;

    obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::IMPLICIT_VR_LITTLE_ENDIAN))
        .map_err(|error| error.to_string())
}

fn read_raw_implicit_vr_little_endian_nodes(path: &str) -> Result<Vec<DicomNode>, String> {
    ensure_raw_dataset_candidate(path)?;
    let file = File::open(path).map_err(|error| error.to_string())?;
    let transfer_syntax = TransferSyntaxRegistry
        .get(uids::IMPLICIT_VR_LITTLE_ENDIAN)
        .ok_or_else(|| "Implicit VR Little Endian transfer syntax is not available".to_string())?;
    let mut reader =
        LazyDataSetReader::new_with_ts_cs(file, transfer_syntax, SpecificCharacterSet::default())
            .map_err(|error| error.to_string())?;
    let (mut nodes, found_pixel_data) = read_dataset_nodes(&mut reader, Vec::new(), false)?;
    if found_pixel_data && !nodes.iter().any(is_pixel_data_node) {
        nodes.push(pixel_data_placeholder_node());
    }
    Ok(nodes)
}

fn ensure_raw_dataset_candidate(path: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header)
        .map_err(|error| format!("Could not read raw dataset header: {error}"))?;

    let group = u16::from_le_bytes([header[0], header[1]]);
    let element = u16::from_le_bytes([header[2], header[3]]);
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let tag = Tag(group, element);

    if !RAW_DATASET_START_GROUPS.contains(&group) || tag >= PIXEL_DATA {
        return Err(format!(
            "Raw dataset fallback rejected first tag {} as unlikely to be a DICOM dataset",
            format_tag(tag)
        ));
    }
    if length == u32::MAX || length > MAX_RAW_DISPLAY_VALUE_LENGTH {
        return Err(format!(
            "Raw dataset fallback rejected first element {} with unsafe length {}",
            format_tag(tag),
            length
        ));
    }

    Ok(())
}

fn read_dataset_nodes<S>(
    reader: &mut LazyDataSetReader<S>,
    parent_path: Vec<String>,
    stop_at_item_end: bool,
) -> Result<(Vec<DicomNode>, bool), String>
where
    S: StatefulDecode,
{
    let mut nodes = Vec::new();

    loop {
        let Some(token) = next_data_token(reader)? else {
            break;
        };

        match token {
            LazyDataToken::ElementHeader(header) => {
                if header.tag == PIXEL_DATA {
                    nodes.push(pixel_data_placeholder_node_with_length(length_to_u32(
                        header.len,
                    )));
                    return Ok((nodes, true));
                }

                let Some(value_token) = next_data_token(reader)? else {
                    return Err(format!(
                        "Missing value for element {}",
                        format_tag(header.tag)
                    ));
                };
                let value_token = value_token
                    .into_owned()
                    .map_err(|error| error.to_string())?;
                let DataToken::PrimitiveValue(value) = value_token else {
                    return Err(format!(
                        "Unexpected value token after element {}",
                        format_tag(header.tag)
                    ));
                };
                nodes.push(primitive_token_to_node(
                    header.tag,
                    header.vr,
                    header.len,
                    value,
                    &parent_path,
                ));
            }
            token @ LazyDataToken::LazyValue { header, .. } => {
                let tag = header.tag;
                let vr = header.vr;
                let length = header.len;
                if tag == PIXEL_DATA {
                    nodes.push(pixel_data_placeholder_node_with_length(length_to_u32(
                        length,
                    )));
                    token.skip().map_err(|error| error.to_string())?;
                    return Ok((nodes, true));
                }
                if length.0 > MAX_RAW_DISPLAY_VALUE_LENGTH {
                    return Err(format!(
                        "Raw dataset element {} has a value length of {} bytes, which exceeds the safe fallback display limit",
                        format_tag(tag),
                        length.0
                    ));
                }
                let value_token = token.into_owned().map_err(|error| error.to_string())?;
                let DataToken::PrimitiveValue(value) = value_token else {
                    return Err(format!("Unexpected DICOM value token {value_token}"));
                };
                nodes.push(primitive_token_to_node(
                    tag,
                    vr,
                    length,
                    value,
                    &parent_path,
                ));
            }
            LazyDataToken::SequenceStart { tag, len } => {
                let tag_text = format_tag(tag);
                let mut path = parent_path.clone();
                path.push(tag_text.clone());
                let (items, found_pixel_data) = read_sequence_items(reader, path.clone())?;
                nodes.push(DicomNode::Sequence {
                    tag: tag_text,
                    description: description_for(tag),
                    length: length_to_u32(len),
                    path,
                    items,
                    item_sources: None,
                });
                if found_pixel_data {
                    return Ok((nodes, true));
                }
            }
            LazyDataToken::PixelSequenceStart => {
                nodes.push(pixel_data_placeholder_node());
                return Ok((nodes, true));
            }
            LazyDataToken::ItemEnd if stop_at_item_end => break,
            LazyDataToken::SequenceEnd if !stop_at_item_end => break,
            LazyDataToken::ItemEnd | LazyDataToken::SequenceEnd => break,
            LazyDataToken::ItemStart { .. } | LazyDataToken::LazyItemValue { .. } => {
                return Err("Unexpected DICOM token in raw dataset".to_string());
            }
            _ => return Err("Unsupported DICOM token in raw dataset".to_string()),
        }
        if nodes.len() > MAX_RAW_DISPLAY_ELEMENTS {
            return Err(format!(
                "Raw dataset has more than {MAX_RAW_DISPLAY_ELEMENTS} display elements before Pixel Data"
            ));
        }
    }

    Ok((nodes, false))
}

fn read_sequence_items<S>(
    reader: &mut LazyDataSetReader<S>,
    sequence_path: Vec<String>,
) -> Result<(Vec<Vec<DicomNode>>, bool), String>
where
    S: StatefulDecode,
{
    let mut items = Vec::new();

    loop {
        let Some(token) = next_data_token(reader)? else {
            break;
        };

        match token {
            LazyDataToken::ItemStart { .. } => {
                let mut item_path = sequence_path.clone();
                item_path.push(format!("Item#{}", items.len()));
                let (nodes, found_pixel_data) = read_dataset_nodes(reader, item_path, true)?;
                items.push(nodes);
                if found_pixel_data {
                    return Ok((items, true));
                }
            }
            LazyDataToken::SequenceEnd => break,
            _ => return Err("Unexpected DICOM sequence token in raw dataset".to_string()),
        }
    }

    Ok((items, false))
}

fn next_data_token<S>(
    reader: &mut LazyDataSetReader<S>,
) -> Result<Option<LazyDataToken<&mut S>>, String>
where
    S: StatefulDecode,
{
    reader
        .advance()
        .transpose()
        .map_err(|error| error.to_string())
}

fn primitive_token_to_node(
    tag: Tag,
    vr: VR,
    length: Length,
    value: PrimitiveValue,
    parent_path: &[String],
) -> DicomNode {
    let tag_text = format_tag(tag);
    let mut path = parent_path.to_vec();
    path.push(tag_text.clone());
    let value = Value::Primitive(value);
    let (value, inferred_vr) = display_element_value(tag, vr, &value);

    DicomNode::Element {
        tag: tag_text,
        vr: vr.to_string().to_owned(),
        inferred_vr,
        description: description_for(tag),
        value,
        length: length_to_u32(length),
        path,
        editable: tag != PIXEL_DATA && is_text_editable(vr),
    }
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
    pixel_data_placeholder_node_with_length(u32::MAX)
}

fn pixel_data_placeholder_node_with_length(length: u32) -> DicomNode {
    DicomNode::Element {
        tag: format_tag(PIXEL_DATA),
        vr: "OB/OW".to_string(),
        inferred_vr: None,
        description: "PixelData".to_string(),
        value: "[Binary Data]".to_string(),
        length,
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

/// Replace text values that dicom-rs decoded differently from the character set in
/// effect for their dataset or item. Values that dicom-rs cannot write byte for byte
/// become read-only. Returns whether any value became read-only.
pub fn apply_text_values(nodes: &mut [DicomNode], text: &TextValues, spliceable: bool) -> bool {
    let mut replaced = false;
    for node in nodes {
        match node {
            DicomNode::Element {
                path,
                value,
                editable,
                ..
            } => {
                let Some(text_value) = text.get(path) else {
                    continue;
                };
                if trim_padding(value) != text_value.decoded {
                    let writable = text_value.editable || spliceable;
                    *value = text_value.decoded.clone();
                    *editable &= writable;
                    replaced |= !writable;
                }
            }
            DicomNode::Sequence { items, .. } => {
                for item in items {
                    replaced |= apply_text_values(item, text, spliceable);
                }
            }
        }
    }
    replaced
}

/// Whether an element already holds the displayed value, so that saving can keep the
/// element as read (including its multiple values) instead of rewriting it.
fn element_has_value(obj: &InMemDicomObject, tag: Tag, vr: VR, value: &str) -> bool {
    obj.get(tag).is_some_and(|element| {
        element.vr() == vr
            && element
                .to_str()
                .is_ok_and(|current| trim_padding(&current) == trim_padding(value))
    })
}

/// Prepare the text values of `obj` for writing through dicom-rs, and fail when a value
/// would not be saved as shown.
///
/// The check follows the character set declarations that will be written, and the way
/// dicom-rs's writer switches codecs:
/// - Unedited text must keep its bytes and, under the declared character sets, its
///   meaning.
/// - Edited or added text is encoded with the declared character set (e.g. ISO 2022
///   JIS). Where dicom-rs writes byte for byte, the encoded bytes are handed to it as a
///   string; otherwise dicom-rs encodes the text itself. Either way the written bytes
///   must decode back to the entered text.
pub fn prepare_text_for_writing(
    obj: &mut InMemDicomObject,
    text: &TextValues,
    edited: &HashSet<Vec<String>>,
) -> Result<(), String> {
    let mut failures = Vec::new();
    let mut codec = SpecificCharacterSet::default();
    let context = TextWriteContext { text, edited };
    prepare_text_values(
        obj,
        &[],
        &TextCharset::default(),
        &context,
        &mut codec,
        &mut failures,
    );
    if failures.is_empty() {
        return Ok(());
    }
    let shown = failures
        .iter()
        .take(5)
        .map(|(path, reason)| format!("{} ({reason})", path.join(" / ")))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Cannot save: these text values cannot be saved as shown: {shown}{}",
        if failures.len() > 5 { "; ..." } else { "" }
    ))
}

struct TextWriteContext<'a> {
    text: &'a TextValues,
    /// Paths of values the user edited or added.
    edited: &'a HashSet<Vec<String>>,
}

fn prepare_text_values(
    obj: &mut InMemDicomObject,
    parent_path: &[String],
    inherited_charset: &TextCharset,
    context: &TextWriteContext,
    codec: &mut SpecificCharacterSet,
    failures: &mut Vec<(Vec<String>, String)>,
) {
    // The character set that the saved file declares for this dataset or item.
    let charset = match obj.get(tags::SPECIFIC_CHARACTER_SET) {
        Some(element) => element
            .to_str()
            .map(|value| TextCharset::from_specific_character_set(&value))
            .unwrap_or_default(),
        None => inherited_charset.clone(),
    };
    let element_tags = obj.iter().map(|element| element.tag()).collect::<Vec<_>>();
    for tag in element_tags {
        let mut path = parent_path.to_vec();
        path.push(format_tag(tag));
        let Some(element) = obj.get(tag) else {
            continue;
        };
        let vr = element.vr();
        match element.value() {
            Value::Sequence(_) => {
                obj.update_value(tag, |value| {
                    if let Some(items) = value.items_mut() {
                        for (index, item) in items.iter_mut().enumerate() {
                            let mut item_path = path.clone();
                            item_path.push(format!("Item#{index}"));
                            prepare_text_values(
                                item, &item_path, &charset, context, codec, failures,
                            );
                        }
                    }
                });
            }
            Value::Primitive(value) if tag == tags::SPECIFIC_CHARACTER_SET => {
                let name = match value {
                    PrimitiveValue::Str(name) => Some(name.as_str()),
                    PrimitiveValue::Strs(names) => names.first().map(String::as_str),
                    _ => None,
                };
                *codec = dicom_rs_codec_after(codec.clone(), name);
            }
            Value::Primitive(value) if is_charset_sensitive(vr) => {
                let stored = match value {
                    PrimitiveValue::Str(text) => vec![text.to_string()],
                    PrimitiveValue::Strs(texts) => texts.to_vec(),
                    _ => continue,
                };
                // Compare joined values: dicom-rs splits values at every 0x5C byte, also
                // inside a kanji, while values Taglet stored after encoding are split
                // per value.
                let original = context.text.get(&path).filter(|original| {
                    !context.edited.contains(&path)
                        && dicom_rs_read(&original.read_codec, original.vr, &original.bytes)
                            .is_some_and(|read| {
                                trim_padding(&read.join("\\")) == trim_padding(&stored.join("\\"))
                            })
                });
                match original {
                    Some(original) => {
                        // Odd-length values get a padding byte on disk, which values
                        // Taglet stored after encoding do not have yet.
                        let keeps_value = dicom_rs_write(codec, &stored).is_some_and(|written| {
                            trim_padding_bytes(&written) == trim_padding_bytes(&original.bytes)
                        }) && charset.decode(original.vr, &original.bytes)
                            == original.decoded;
                        if !keeps_value {
                            let reason = "its bytes or meaning would change".to_string();
                            failures.push((path, reason));
                        }
                    }
                    None => match encode_edited_text(&charset, codec, vr, &stored) {
                        Ok(Some(parts)) => {
                            let value = PrimitiveValue::Strs(parts.into());
                            obj.put(DataElement::new(tag, vr, value));
                        }
                        Ok(None) => {}
                        Err(reason) => failures.push((path, reason)),
                    },
                }
            }
            _ => {}
        }
    }
}

/// Fail when a text value would not fit the 16-bit value length field that Explicit VR
/// transfer syntaxes use for many VRs (at most 65,534 bytes after padding). dicom-rs
/// truncates the length silently, which would leave a file that cannot be read.
fn ensure_value_lengths_fit(obj: &InMemDicomObject, explicit_vr: bool) -> Result<(), String> {
    if !explicit_vr {
        return Ok(());
    }
    let mut failures = Vec::new();
    let mut codec = SpecificCharacterSet::default();
    collect_oversized_values(obj, &[], &mut codec, &mut failures);
    if failures.is_empty() {
        return Ok(());
    }
    let shown = failures
        .iter()
        .take(5)
        .map(|(path, length)| format!("{} ({length} bytes)", path.join(" / ")))
        .collect::<Vec<_>>()
        .join("; ");
    Err(format!(
        "Cannot save: these values are longer than the 65,534 bytes their VR allows in this transfer syntax: {shown}{}",
        if failures.len() > 5 { "; ..." } else { "" }
    ))
}

fn collect_oversized_values(
    obj: &InMemDicomObject,
    parent_path: &[String],
    codec: &mut SpecificCharacterSet,
    failures: &mut Vec<(Vec<String>, usize)>,
) {
    for element in obj.iter() {
        let tag = element.tag();
        let mut path = parent_path.to_vec();
        path.push(format_tag(tag));
        match element.value() {
            Value::Sequence(sequence) => {
                for (index, item) in sequence.items().iter().enumerate() {
                    let mut item_path = path.clone();
                    item_path.push(format!("Item#{index}"));
                    collect_oversized_values(item, &item_path, codec, failures);
                }
            }
            Value::Primitive(value) => {
                let strings = match value {
                    PrimitiveValue::Str(text) => vec![text.to_string()],
                    PrimitiveValue::Strs(texts) => texts.to_vec(),
                    _ => continue,
                };
                if has_16_bit_length(element.vr()) {
                    // ASCII-only VRs have the same length in every codec dicom-rs uses.
                    let length = dicom_rs_write(codec, &strings)
                        .map_or_else(|| strings.join("\\").len(), |bytes| bytes.len());
                    let padded = length + length % 2;
                    if padded > 0xFFFE {
                        failures.push((path.clone(), padded));
                    }
                }
                if tag == tags::SPECIFIC_CHARACTER_SET {
                    *codec =
                        dicom_rs_codec_after(codec.clone(), strings.first().map(String::as_str));
                }
            }
            Value::PixelSequence(_) => {}
        }
    }
}

/// VRs whose value length is a 16-bit field in Explicit VR transfer syntaxes
/// (PS3.5 7.1.2).
fn has_16_bit_length(vr: VR) -> bool {
    matches!(
        vr,
        VR::AE
            | VR::AS
            | VR::AT
            | VR::CS
            | VR::DA
            | VR::DS
            | VR::DT
            | VR::FL
            | VR::FD
            | VR::IS
            | VR::LO
            | VR::LT
            | VR::PN
            | VR::SH
            | VR::SL
            | VR::SS
            | VR::ST
            | VR::TM
            | VR::UI
            | VR::UL
            | VR::US
    )
}

/// `bytes` without trailing padding (spaces and NULs).
fn trim_padding_bytes(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != b' ' && *byte != 0)
        .map_or(0, |index| index + 1);
    &bytes[..end]
}

/// Encode edited text. Returns `Ok(Some(parts))` with strings to store when dicom-rs
/// must be handed pre-encoded bytes, `Ok(None)` when the stored text can be written as
/// it is, and why it cannot be saved as entered otherwise.
fn encode_edited_text(
    charset: &TextCharset,
    codec: &SpecificCharacterSet,
    vr: VR,
    stored: &[String],
) -> Result<Option<Vec<String>>, String> {
    let reads_back_differently = || "it would read back differently".to_string();
    let entered = stored.join("\\");
    let entered = trim_padding(&entered);
    let expected = charset
        .encode(vr, entered)
        .map_err(|error| error.to_string())?;
    if charset.decode(vr, &expected) != entered {
        return Err(reads_back_differently());
    }
    if dicom_rs_codec_is_byte_transparent(codec) {
        let values = if is_single_valued_text(vr) {
            vec![entered]
        } else {
            entered.split('\\').collect()
        };
        let parts = values
            .into_iter()
            .map(|value| {
                charset
                    .encode(vr, value)
                    .map(|bytes| byte_transparent_string(&bytes))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if dicom_rs_write(codec, &parts).as_ref() == Some(&expected) {
            Ok(Some(parts))
        } else {
            Err(reads_back_differently())
        }
    } else {
        match dicom_rs_write(codec, stored) {
            Some(written) if charset.decode(vr, &written) == entered => Ok(None),
            _ => Err(
                "it can only be written by editing the file in place, which is possible for Part 10 files in Explicit or Implicit VR Little Endian whose file meta SOP UIDs match the data set, when the save adds no sequences or items and removes no SOP UIDs, and, when the Specific Character Set changes, if Taglet can write text in the new one".to_string(),
            ),
        }
    }
}

/// Text decoded with the character set in effect for its dataset or item, when known.
fn get_text(
    obj: &InMemDicomObject,
    tag: Tag,
    text: &TextValues,
    path: &[String],
) -> Option<String> {
    let mut element_path = path.to_vec();
    element_path.push(format_tag(tag));
    match text.get(&element_path) {
        Some(value) if obj.get(tag).is_some() => Some(value.decoded.clone()),
        _ => get_string(obj, tag),
    }
}

fn display_element_value(
    tag: Tag,
    vr: VR,
    value: &Value<InMemDicomObject>,
) -> (String, Option<String>) {
    if tag.group() % 2 == 1 && vr == VR::UN {
        if let Some(text) = private_un_text_value(value) {
            let inferred_vr = infer_text_vr(&text).map(|vr| format!("{vr}?"));
            return (text, inferred_vr);
        }
    }

    (display_value(value), None)
}

fn private_un_text_value(value: &Value<InMemDicomObject>) -> Option<String> {
    let Value::Primitive(PrimitiveValue::U8(bytes)) = value else {
        return None;
    };
    let trimmed = trim_dicom_padding(bytes.as_slice());
    if trimmed.is_empty() || trimmed.len() > 10_240 {
        return None;
    }
    if !trimmed
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ' || *byte == b'\\')
    {
        return None;
    }

    String::from_utf8(trimmed.to_vec()).ok()
}

fn trim_dicom_padding(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == b'\0' || bytes[end - 1] == b' ') {
        end -= 1;
    }
    &bytes[..end]
}

fn infer_text_vr(value: &str) -> Option<&'static str> {
    let components = value.split('\\').collect::<Vec<_>>();
    if components.is_empty() {
        return None;
    }

    if components.iter().all(|component| {
        !component.is_empty() && component.len() <= 16 && is_code_string(component)
    }) {
        return Some("CS");
    }

    if components.iter().all(|component| component.len() <= 16) {
        return Some("SH");
    }

    if components.iter().all(|component| component.len() <= 64) {
        return Some("LO");
    }

    if value.len() <= 10_240 && !value.contains('\\') {
        return Some("LT");
    }

    None
}

fn is_code_string(value: &str) -> bool {
    value.bytes().all(|byte| {
        byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_' || byte == b' '
    })
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

pub(super) fn parse_tag(tag: &str) -> Result<Tag, String> {
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

fn get_uid_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.get(tag)
        .and_then(|element| element.to_str().ok())
        .map(|value| {
            value
                .trim_end_matches(|c| c == '\0' || c == ' ')
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn ui_padded(value: String) -> String {
    if value.len() % 2 == 0 {
        value
    } else {
        format!("{value}\0")
    }
}

fn get_u32(obj: &InMemDicomObject, tag: Tag) -> Option<u32> {
    obj.get(tag).and_then(|element| {
        element
            .to_int::<u32>()
            .ok()
            .or_else(|| element.to_str().ok()?.trim().parse::<u32>().ok())
    })
}

fn get_u32_values(obj: &InMemDicomObject, tag: Tag) -> Vec<u32> {
    obj.get(tag)
        .and_then(|element| element.to_multi_int::<u32>().ok())
        .unwrap_or_default()
}

fn get_i32(obj: &InMemDicomObject, tag: Tag) -> Option<i32> {
    obj.get(tag).and_then(|element| {
        element
            .to_int::<i32>()
            .ok()
            .or_else(|| element.to_str().ok()?.trim().parse::<i32>().ok())
    })
}

fn get_f64_values(obj: &InMemDicomObject, tag: Tag) -> Vec<f64> {
    obj.get(tag)
        .and_then(|element| element.to_multi_float64().ok())
        .unwrap_or_default()
}

fn sequence_items(obj: &InMemDicomObject, tag: Tag) -> Option<&[InMemDicomObject]> {
    obj.get(tag).and_then(|element| match element.value() {
        Value::Sequence(sequence) => Some(sequence.items()),
        _ => None,
    })
}

fn structure_set_roi_names(obj: &InMemDicomObject, text: &TextValues) -> HashMap<i32, String> {
    sequence_items(obj, STRUCTURE_SET_ROI_SEQUENCE)
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let roi_number = get_i32(item, ROI_NUMBER)?;
                    let item_path = [
                        format_tag(STRUCTURE_SET_ROI_SEQUENCE),
                        format!("Item#{index}"),
                    ];
                    let name = get_text(item, ROI_NAME, text, &item_path)?;
                    Some((roi_number, name))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn rt_struct_rois(
    obj: &InMemDicomObject,
    roi_names: &HashMap<i32, String>,
    contours: &[RtStructContour],
) -> Vec<RtStructRoi> {
    let contour_counts =
        contours
            .iter()
            .fold(HashMap::<i32, usize>::new(), |mut counts, contour| {
                *counts.entry(contour.roi_number).or_default() += 1;
                counts
            });
    let colors = contours
        .iter()
        .fold(HashMap::<i32, [u8; 3]>::new(), |mut colors, contour| {
            if let Some(color) = contour.color {
                colors.entry(contour.roi_number).or_insert(color);
            }
            colors
        });

    let mut rois = sequence_items(obj, STRUCTURE_SET_ROI_SEQUENCE)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let roi_number = get_i32(item, ROI_NUMBER)?;
                    Some(RtStructRoi {
                        roi_number,
                        name: roi_names.get(&roi_number).cloned(),
                        color: colors.get(&roi_number).copied(),
                        contour_count: *contour_counts.get(&roi_number).unwrap_or(&0),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for contour in contours {
        if !rois.iter().any(|roi| roi.roi_number == contour.roi_number) {
            rois.push(RtStructRoi {
                roi_number: contour.roi_number,
                name: roi_names.get(&contour.roi_number).cloned(),
                color: contour.color,
                contour_count: *contour_counts.get(&contour.roi_number).unwrap_or(&0),
            });
        }
    }

    rois.sort_by_key(|roi| roi.roi_number);
    rois
}

fn rt_struct_contours(
    obj: &InMemDicomObject,
    roi_names: &HashMap<i32, String>,
) -> Vec<RtStructContour> {
    sequence_items(obj, ROI_CONTOUR_SEQUENCE)
        .map(|roi_items| {
            roi_items
                .iter()
                .flat_map(|roi_item| {
                    let roi_number = get_i32(roi_item, REFERENCED_ROI_NUMBER).unwrap_or_default();
                    let roi_name = roi_names.get(&roi_number).cloned();
                    let color = get_rgb_color(roi_item, ROI_DISPLAY_COLOR);
                    sequence_items(roi_item, CONTOUR_SEQUENCE)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(move |contour_item| {
                            let points = contour_points(contour_item);
                            if points.len() < 2 {
                                return None;
                            }

                            Some(RtStructContour {
                                roi_number,
                                roi_name: roi_name.clone(),
                                color,
                                geometric_type: get_string(contour_item, CONTOUR_GEOMETRIC_TYPE),
                                points,
                            })
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn contour_points(obj: &InMemDicomObject) -> Vec<RtStructPoint> {
    get_f64_values(obj, CONTOUR_DATA)
        .chunks_exact(3)
        .map(|point| RtStructPoint {
            x: point[0],
            y: point[1],
            z: point[2],
        })
        .collect()
}

fn get_rgb_color(obj: &InMemDicomObject, tag: Tag) -> Option<[u8; 3]> {
    let values = get_u32_values(obj, tag);
    if values.len() < 3 {
        return None;
    }

    Some([
        values[0].min(255) as u8,
        values[1].min(255) as u8,
        values[2].min(255) as u8,
    ])
}

fn rt_struct_slices(contours: &[RtStructContour]) -> Vec<RtStructSlice> {
    let mut slices = Vec::<RtStructSlice>::new();

    for z in contours.iter().filter_map(contour_slice_z) {
        if let Some(slice) = slices.iter_mut().find(|slice| same_slice_z(slice.z, z)) {
            slice.contour_count += 1;
        } else {
            slices.push(RtStructSlice {
                z,
                contour_count: 1,
            });
        }
    }

    slices.sort_by(|left, right| {
        left.z
            .partial_cmp(&right.z)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    slices
}

fn rt_struct_bounds(contours: &[RtStructContour]) -> Option<RtStructBounds> {
    let mut bounds = RtStructBounds {
        min_x: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        min_y: f64::INFINITY,
        max_y: f64::NEG_INFINITY,
        min_z: f64::INFINITY,
        max_z: f64::NEG_INFINITY,
    };

    for point in contours.iter().flat_map(|contour| contour.points.iter()) {
        bounds.min_x = bounds.min_x.min(point.x);
        bounds.max_x = bounds.max_x.max(point.x);
        bounds.min_y = bounds.min_y.min(point.y);
        bounds.max_y = bounds.max_y.max(point.y);
        bounds.min_z = bounds.min_z.min(point.z);
        bounds.max_z = bounds.max_z.max(point.z);
    }

    if bounds.min_x.is_finite()
        && bounds.max_x.is_finite()
        && bounds.min_y.is_finite()
        && bounds.max_y.is_finite()
        && bounds.min_z.is_finite()
        && bounds.max_z.is_finite()
    {
        Some(bounds)
    } else {
        None
    }
}

fn contour_slice_z(contour: &RtStructContour) -> Option<f64> {
    if contour.points.is_empty() {
        return None;
    }

    Some(contour.points.iter().map(|point| point.z).sum::<f64>() / contour.points.len() as f64)
}

fn same_slice_z(left: f64, right: f64) -> bool {
    (left - right).abs() <= RTSTRUCT_Z_TOLERANCE
}

fn beam_limiting_device_positions(
    control_point: &InMemDicomObject,
) -> Vec<RtPlanBeamLimitingDevicePosition> {
    sequence_items(control_point, BEAM_LIMITING_DEVICE_POSITION_SEQUENCE)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let device_type = get_string(item, RT_BEAM_LIMITING_DEVICE_TYPE)?;
                    Some(RtPlanBeamLimitingDevicePosition {
                        device_type,
                        positions: get_f64_values(item, LEAF_JAW_POSITIONS),
                        inherited: false,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn beam_limiting_device_definitions(
    beam: &InMemDicomObject,
) -> Vec<RtPlanBeamLimitingDeviceDefinition> {
    sequence_items(beam, BEAM_LIMITING_DEVICE_SEQUENCE)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let device_type = get_string(item, RT_BEAM_LIMITING_DEVICE_TYPE)?;
                    Some(RtPlanBeamLimitingDeviceDefinition {
                        device_type,
                        number_of_pairs: get_u32(item, NUMBER_OF_LEAF_JAW_PAIRS),
                        leaf_position_boundaries: get_f64_values(item, LEAF_POSITION_BOUNDARIES),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn effective_control_points(items: &[InMemDicomObject]) -> Vec<RtPlanControlPoint> {
    let mut gantry_angle = None;
    let mut collimator_angle = None;
    let mut couch_angle = None;
    let mut devices = HashMap::<String, Vec<f64>>::new();

    items
        .iter()
        .enumerate()
        .map(|(control_point_index, control_point)| {
            let raw_gantry_angle = get_f64_values(control_point, GANTRY_ANGLE)
                .into_iter()
                .next();
            let raw_collimator_angle = get_f64_values(control_point, BEAM_LIMITING_DEVICE_ANGLE)
                .into_iter()
                .next();
            let raw_couch_angle = get_f64_values(control_point, PATIENT_SUPPORT_ANGLE)
                .into_iter()
                .next();

            let gantry_angle_inherited = raw_gantry_angle.is_none() && gantry_angle.is_some();
            let collimator_angle_inherited =
                raw_collimator_angle.is_none() && collimator_angle.is_some();
            let couch_angle_inherited = raw_couch_angle.is_none() && couch_angle.is_some();

            if raw_gantry_angle.is_some() {
                gantry_angle = raw_gantry_angle;
            }
            if raw_collimator_angle.is_some() {
                collimator_angle = raw_collimator_angle;
            }
            if raw_couch_angle.is_some() {
                couch_angle = raw_couch_angle;
            }

            let updated_devices = beam_limiting_device_positions(control_point)
                .into_iter()
                .map(|device| {
                    devices.insert(device.device_type.to_uppercase(), device.positions);
                    device.device_type.to_uppercase()
                })
                .collect::<HashSet<_>>();

            let mut effective_devices = devices
                .iter()
                .map(
                    |(device_type, positions)| RtPlanBeamLimitingDevicePosition {
                        device_type: device_type.clone(),
                        positions: positions.clone(),
                        inherited: !updated_devices.contains(device_type),
                    },
                )
                .collect::<Vec<_>>();
            effective_devices.sort_by(|left, right| left.device_type.cmp(&right.device_type));

            RtPlanControlPoint {
                control_point_index,
                nominal_index: get_i32(control_point, CONTROL_POINT_INDEX),
                gantry_angle,
                gantry_angle_inherited,
                collimator_angle,
                collimator_angle_inherited,
                couch_angle,
                couch_angle_inherited,
                devices: effective_devices,
            }
        })
        .collect()
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
        return Some(
            "Only uncompressed Little Endian transfer syntaxes are prepared for now".to_string(),
        );
    }
    if let Some(photometric) = photometric_interpretation {
        if !matches!(photometric, "MONOCHROME1" | "MONOCHROME2" | "RGB") {
            return Some(format!(
                "Photometric Interpretation {photometric} is not prepared yet"
            ));
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

/// How stored pixel values are packed into each allocated sample.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PixelLayout {
    bits_allocated: u32,
    bits_stored: u32,
    high_bit: u32,
    signed: bool,
}

impl PixelLayout {
    /// Falls back to the full sample when Bits Stored / High Bit are missing or inconsistent.
    fn new(
        bits_allocated: u32,
        bits_stored: Option<u32>,
        high_bit: Option<u32>,
        signed: bool,
    ) -> Self {
        let bits_stored = bits_stored
            .filter(|bits| (1..=bits_allocated).contains(bits))
            .unwrap_or(bits_allocated);
        let high_bit = high_bit
            .filter(|bit| (bits_stored - 1..bits_allocated).contains(bit))
            .unwrap_or(bits_stored - 1);
        Self {
            bits_allocated,
            bits_stored,
            high_bit,
            signed,
        }
    }

    fn from_info(info: &DicomImageInfo, bits_allocated: u32) -> Self {
        Self::new(
            bits_allocated,
            info.bits_stored,
            info.high_bit,
            info.pixel_representation.unwrap_or(0) == 1,
        )
    }
}

fn stored_pixel_value(frame: &[u8], index: usize, layout: PixelLayout) -> f64 {
    let raw: u64 = match layout.bits_allocated {
        8 => frame[index] as u64,
        16 => {
            let offset = index * 2;
            u16::from_le_bytes([frame[offset], frame[offset + 1]]) as u64
        }
        32 => {
            let offset = index * 4;
            u32::from_le_bytes([
                frame[offset],
                frame[offset + 1],
                frame[offset + 2],
                frame[offset + 3],
            ]) as u64
        }
        _ => return 0.0,
    };

    // Keep only bits (high_bit - bits_stored + 1)..=high_bit; the rest may hold overlays or garbage.
    let shift = layout.high_bit + 1 - layout.bits_stored;
    let value = (raw >> shift) & ((1u64 << layout.bits_stored) - 1);
    if layout.signed && value & (1u64 << (layout.bits_stored - 1)) != 0 {
        value as f64 - (1u64 << layout.bits_stored) as f64
    } else {
        value as f64
    }
}

fn frame_min_max(
    frame: &[u8],
    layout: PixelLayout,
    slope: f64,
    intercept: f64,
    sample_count: usize,
) -> (f64, f64) {
    let mut min_value = f64::INFINITY;
    let mut max_value = f64::NEG_INFINITY;

    for index in 0..sample_count {
        let stored = stored_pixel_value(frame, index, layout) * slope + intercept;
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
        let frame = [0x78, 0x56, 0x34, 0x12, 0xFF, 0xFF, 0xFF, 0xFF];

        let layout = PixelLayout::new(32, None, None, false);
        assert_eq!(stored_pixel_value(&frame, 0, layout), 0x1234_5678 as f64);
        assert_eq!(stored_pixel_value(&frame, 1, layout), u32::MAX as f64);
    }

    #[test]
    fn decodes_signed_32_bit_pixels() {
        let frame = [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x80];

        let layout = PixelLayout::new(32, Some(32), Some(31), true);
        assert_eq!(stored_pixel_value(&frame, 0, layout), -1.0);
        assert_eq!(stored_pixel_value(&frame, 1, layout), i32::MIN as f64);
    }

    #[test]
    fn masks_bits_above_bits_stored() {
        // 12-bit unsigned in 16-bit samples with garbage (e.g. overlay) in the upper nibble.
        let frame = [0xFF, 0xFF, 0x34, 0xA2];
        let layout = PixelLayout::new(16, Some(12), Some(11), false);
        assert_eq!(stored_pixel_value(&frame, 0, layout), 0x0FFF as f64);
        assert_eq!(stored_pixel_value(&frame, 1, layout), 0x0234 as f64);
        assert_eq!(
            frame_min_max(&frame, layout, 1.0, 0.0, 2),
            (0x234 as f64, 0xFFF as f64)
        );
    }

    #[test]
    fn sign_extends_from_bits_stored() {
        // 12-bit signed: 0x0FFF is -1 and 0x0800 is -2048 regardless of the upper nibble.
        let frame = [0xFF, 0xFF, 0x00, 0x58, 0xFF, 0x07];
        let layout = PixelLayout::new(16, Some(12), Some(11), true);
        assert_eq!(stored_pixel_value(&frame, 0, layout), -1.0);
        assert_eq!(stored_pixel_value(&frame, 1, layout), -2048.0);
        assert_eq!(stored_pixel_value(&frame, 2, layout), 2047.0);
    }

    #[test]
    fn honors_high_bit_offset() {
        // 12 bits stored at bits 15..=4 (High Bit 15).
        let frame = 0xABC0u16.to_le_bytes();
        let layout = PixelLayout::new(16, Some(12), Some(15), false);
        assert_eq!(stored_pixel_value(&frame, 0, layout), 0xABC as f64);
    }

    #[test]
    fn invalid_bits_stored_or_high_bit_fall_back_to_full_sample() {
        assert_eq!(PixelLayout::new(16, Some(0), None, false).bits_stored, 16);
        assert_eq!(PixelLayout::new(16, Some(20), None, false).bits_stored, 16);
        let layout = PixelLayout::new(16, Some(12), Some(16), false);
        assert_eq!((layout.bits_stored, layout.high_bit), (12, 11));
        let layout = PixelLayout::new(16, Some(12), Some(5), false);
        assert_eq!((layout.bits_stored, layout.high_bit), (12, 11));
    }

    #[test]
    fn applies_dose_grid_scaling_to_min_max() {
        let frame = [0x00, 0x00, 0x00, 0x00, 0x10, 0x27, 0x00, 0x00];

        let (min, max) = frame_min_max(
            &frame,
            PixelLayout::new(32, None, None, false),
            0.001,
            0.0,
            2,
        );

        assert_eq!(min, 0.0);
        assert_eq!(max, 10.0);
    }

    #[test]
    fn infers_private_un_text_vr_from_value_shape() {
        assert_eq!(infer_text_vr("HF1"), Some("CS"));
        assert_eq!(infer_text_vr("d10"), Some("SH"));
        assert_eq!(infer_text_vr("LongerThanSixteenChars"), Some("LO"));
        assert_eq!(infer_text_vr(&"A".repeat(65)), Some("LT"));
        assert_eq!(infer_text_vr(&"A".repeat(10_240)), Some("LT"));
        assert_eq!(infer_text_vr(&"A".repeat(10_241)), None);
        assert_eq!(infer_text_vr(&format!("{}\\B", "A".repeat(65))), None);
    }

    #[test]
    fn private_un_ascii_display_accepts_up_to_10240_bytes() {
        for length in [66, 256, 257, 10_240] {
            let text = "a".repeat(length);
            let value = Value::Primitive(PrimitiveValue::U8(text.clone().into_bytes().into()));
            let (display, inferred) = display_element_value(Tag(0x0019, 0x1001), VR::UN, &value);
            assert_eq!(display, text);
            assert_eq!(inferred.as_deref(), Some("LT?"));
        }
        let oversized = Value::Primitive(PrimitiveValue::U8(vec![b'A'; 10_241].into()));
        assert!(private_un_text_value(&oversized).is_none());
    }

    #[test]
    fn private_un_ascii_display_rejects_structure_and_non_printable_bytes() {
        for bytes in [
            vec![0xFE, 0xFF, 0x00, 0xE0],
            b"A\nB".to_vec(),
            b"A\tB".to_vec(),
            b"A\0B".to_vec(),
            vec![0xC3, 0xA9],
            vec![],
            vec![b' ', 0],
        ] {
            let value = Value::Primitive(PrimitiveValue::U8(bytes.into()));
            assert!(private_un_text_value(&value).is_none());
        }
        let value = Value::Primitive(PrimitiveValue::U8(b"Readable text \0".to_vec().into()));
        assert_eq!(
            private_un_text_value(&value).as_deref(),
            Some("Readable text")
        );
    }

    #[test]
    fn inferred_private_un_text_remains_read_only_and_preserves_bytes_on_save() {
        let tag = Tag(0x0019, 0x1001);
        let bytes = [vec![b'A'; 10240], vec![b' ', 0]].concat();
        let mut obj = InMemDicomObject::new_empty();
        obj.put_str(Tag(0x0019, 0x0010), VR::LO, "VENDOR");
        obj.put(dicom_core::DataElement::new(
            tag,
            VR::UN,
            PrimitiveValue::U8(bytes.clone().into()),
        ));
        let nodes = object_to_nodes(&obj, vec![]);
        let node = nodes
            .iter()
            .find(|node| matches!(node, DicomNode::Element { tag, .. } if tag == "(0019,1001)"))
            .unwrap();
        assert!(
            matches!(node, DicomNode::Element { vr, inferred_vr: Some(inferred), editable: false, .. } if vr == "UN" && inferred == "LT?")
        );
        let syntaxes = [
            uids::IMPLICIT_VR_LITTLE_ENDIAN,
            uids::EXPLICIT_VR_LITTLE_ENDIAN,
        ];
        let before = syntaxes.map(|uid| {
            let mut encoded = Vec::new();
            obj.write_dataset_with_ts(&mut encoded, TransferSyntaxRegistry.get(uid).unwrap())
                .unwrap();
            encoded
        });
        apply_nodes_to_object(&mut obj, &nodes).unwrap();
        assert_eq!(obj.get(tag).unwrap().vr(), VR::UN);
        assert_eq!(
            obj.get(tag).unwrap().value().to_bytes().unwrap().as_ref(),
            bytes.as_slice()
        );
        for (uid, expected) in syntaxes.into_iter().zip(before) {
            let mut encoded = Vec::new();
            obj.write_dataset_with_ts(&mut encoded, TransferSyntaxRegistry.get(uid).unwrap())
                .unwrap();
            assert_eq!(encoded, expected);
        }
    }

    /// PS3.5 Annex H.3.1, JIS X 0208 with ASCII.
    const YAMADA_IR87: &[u8] =
        b"Yamada^Tarou=\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B=\x1b$B$d$^$@\x1b(B^\x1b$B$?$m$&\x1b(B";
    /// "前立腺" (prostate) in JIS X 0208.
    const PROSTATE_IR87: &[u8] = b"\x1b$BA0N)A#\x1b(B";
    /// "ﾊﾝ" in JIS X 0201 Katakana (ISO_IR 13). Not valid UTF-8.
    const HAN_IR13: &[u8] = b"\xca\xdd";
    const RTSTRUCT_SOP_CLASS: &str = "1.2.840.10008.5.1.4.1.1.481.3";

    fn explicit_element(group: u16, element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
        let mut value = value.to_vec();
        if value.len() % 2 == 1 {
            value.push(if vr == b"UI" { 0 } else { b' ' });
        }
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_le_bytes());
        out.extend_from_slice(&element.to_le_bytes());
        out.extend_from_slice(vr);
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(&value);
        out
    }

    fn implicit_element(group: u16, element: u16, value: &[u8]) -> Vec<u8> {
        let mut value = value.to_vec();
        if value.len() % 2 == 1 {
            value.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_le_bytes());
        out.extend_from_slice(&element.to_le_bytes());
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(&value);
        out
    }

    fn sequence(group: u16, element: u16, items: &[Vec<u8>], explicit: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_le_bytes());
        out.extend_from_slice(&element.to_le_bytes());
        if explicit {
            out.extend_from_slice(b"SQ\0\0");
        }
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        for item in items {
            out.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0]);
            out.extend_from_slice(&u32::MAX.to_le_bytes());
            out.extend_from_slice(item);
            out.extend_from_slice(&[0xFE, 0xFF, 0x0D, 0xE0, 0, 0, 0, 0]);
        }
        out.extend_from_slice(&[0xFE, 0xFF, 0xDD, 0xE0, 0, 0, 0, 0]);
        out
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("taglet-{}-{name}", std::process::id()))
    }

    /// Write an Explicit VR Little Endian Part 10 file whose dataset starts with
    /// Specific Character Set (when given), SOP Class/Instance UID and Modality.
    fn write_part10(path: &std::path::Path, charset: Option<&[u8]>, rest: &[u8]) {
        let mut dataset = Vec::new();
        if let Some(charset) = charset {
            dataset.extend(explicit_element(0x0008, 0x0005, b"CS", charset));
        }
        dataset.extend(explicit_element(
            0x0008,
            0x0016,
            b"UI",
            RTSTRUCT_SOP_CLASS.as_bytes(),
        ));
        dataset.extend(explicit_element(0x0008, 0x0018, b"UI", b"1.2.3.4"));
        dataset.extend(explicit_element(0x0008, 0x0060, b"CS", b"RTSTRUCT"));
        dataset.extend_from_slice(rest);

        let meta = FileMetaTableBuilder::new()
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(RTSTRUCT_SOP_CLASS)
            .media_storage_sop_instance_uid("1.2.3.4")
            .build()
            .unwrap();
        let mut file = Vec::new();
        file.extend_from_slice(&[0; 128]);
        file.extend_from_slice(b"DICM");
        meta.write(&mut file).unwrap();
        file.extend(dataset);
        std::fs::write(path, file).unwrap();
    }

    fn roi_sequence(item_charset: Option<&[u8]>, roi_name: &[u8]) -> Vec<u8> {
        let mut item = Vec::new();
        if let Some(charset) = item_charset {
            item.extend(explicit_element(0x0008, 0x0005, b"CS", charset));
        }
        item.extend(explicit_element(0x3006, 0x0022, b"IS", b"1"));
        item.extend(explicit_element(0x3006, 0x0026, b"LO", roi_name));
        sequence(0x3006, 0x0020, &[item], true)
    }

    /// An RT Structure Set with Japanese text in the dataset and in a sequence item.
    fn write_iso2022_rtstruct(path: &std::path::Path, charset: &str) {
        let rest = [
            explicit_element(0x0010, 0x0010, b"PN", YAMADA_IR87),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            roi_sequence(None, PROSTATE_IR87),
        ]
        .concat();
        write_part10(path, Some(charset.as_bytes()), &rest);
    }

    fn find_element<'a>(nodes: &'a [DicomNode], wanted: &str) -> Option<&'a DicomNode> {
        nodes.iter().find_map(|node| match node {
            DicomNode::Element { tag, .. } if tag == wanted => Some(node),
            DicomNode::Sequence { items, .. } => {
                items.iter().find_map(|item| find_element(item, wanted))
            }
            _ => None,
        })
    }

    fn value_and_editable(nodes: &[DicomNode], wanted: &str) -> (String, bool) {
        match find_element(nodes, wanted) {
            Some(DicomNode::Element {
                value, editable, ..
            }) => (value.clone(), *editable),
            _ => panic!("{wanted} not found"),
        }
    }

    fn has_warning(content: &DicomFileContent, warning: &str) -> bool {
        content
            .warnings
            .iter()
            .any(|candidate| candidate == warning)
    }

    /// Open, apply the displayed nodes unchanged and save like `save_to_path` does.
    fn save_unchanged(source: &std::path::Path) -> Result<Vec<u8>, String> {
        let source_text = source.to_str().unwrap();
        let nodes = open_nodes(source_text)?.nodes;
        save_nodes(source, &nodes)
    }

    /// Apply nodes and save like `save_to_path` does.
    fn save_nodes(source: &std::path::Path, nodes: &[DicomNode]) -> Result<Vec<u8>, String> {
        let source_text = source.to_str().unwrap();
        let text = TextValues::try_read(source_text)?;
        let mut obj = open_full_object(source_text)?;
        let destination = source.with_extension("saved.dcm");
        let saved = save_object_with_nodes(&mut obj, nodes, &text, destination.to_str().unwrap());
        if saved.is_err() {
            std::fs::remove_file(&destination).ok();
        }
        saved?;
        let written = std::fs::read(&destination).map_err(|error| error.to_string())?;
        std::fs::remove_file(&destination).ok();
        Ok(written)
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    #[test]
    fn displays_iso2022_japanese_text_editable() {
        for charset in [
            "\\ISO 2022 IR 87",
            "ISO 2022 IR 87",
            "ISO 2022 IR 6\\ISO 2022 IR 87",
        ] {
            let path = temp_path("iso2022-display.dcm");
            write_iso2022_rtstruct(&path, charset);
            let content = open_nodes(path.to_str().unwrap()).unwrap();
            std::fs::remove_file(&path).ok();

            assert_eq!(
                value_and_editable(&content.nodes, "(0010,0010)"),
                ("Yamada^Tarou=山田^太郎=やまだ^たろう".to_string(), true),
                "{charset:?}"
            );
            assert_eq!(
                value_and_editable(&content.nodes, "(3006,0026)"),
                ("前立腺".to_string(), true),
                "{charset:?}"
            );
            // ASCII-only text stays editable.
            assert_eq!(
                value_and_editable(&content.nodes, "(0010,0020)"),
                ("ID001".to_string(), true)
            );
            assert!(!has_warning(&content, TEXT_READ_ONLY_WARNING));
            assert!(!has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
        }
    }

    #[test]
    fn rt_struct_roi_names_use_text_values() {
        let path = temp_path("iso2022-roi.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let path_text = path.to_str().unwrap();
        let obj = open_full_object(path_text).unwrap();
        let data = rt_struct_data(&obj, &TextValues::read(path_text)).unwrap();
        std::fs::remove_file(&path).ok();

        let names = data
            .info
            .rois
            .iter()
            .map(|roi| roi.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![Some("前立腺".to_string())]);
    }

    #[test]
    fn saving_iso2022_file_preserves_original_bytes() {
        let path = temp_path("iso2022-save.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let written = save_unchanged(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(contains(&written, YAMADA_IR87));
        assert!(contains(&written, PROSTATE_IR87));
        assert!(contains(&written, b"\\ISO 2022 IR 87"));
    }

    #[test]
    fn raw_dataset_fallback_decodes_iso2022_text() {
        let path = temp_path("iso2022-raw.dcm");
        let dataset = [
            implicit_element(0x0008, 0x0005, b"\\ISO 2022 IR 87"),
            implicit_element(0x0008, 0x0060, b"RTSTRUCT"),
            implicit_element(0x0010, 0x0010, YAMADA_IR87),
        ]
        .concat();
        std::fs::write(&path, dataset).unwrap();
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(0010,0010)"),
            ("Yamada^Tarou=山田^太郎=やまだ^たろう".to_string(), true)
        );
        assert!(!has_warning(&content, TEXT_READ_ONLY_WARNING));
    }

    #[test]
    fn files_without_japanese_charset_are_unchanged() {
        let path = temp_path("latin1.dcm");
        let rest = [
            explicit_element(0x0010, 0x0010, b"PN", b"Caf\xe9^Ren\xe9"),
            roi_sequence(None, b"PTV"),
        ]
        .concat();
        write_part10(&path, Some(b"ISO_IR 100"), &rest);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(0010,0010)"),
            ("Café^René".to_string(), true)
        );
        assert!(content.warnings.is_empty(), "{:?}", content.warnings);
    }

    /// Review P1: an item character set that dicom-rs does not know, under a parent
    /// character set whose codec cannot carry the item's bytes.
    #[test]
    fn item_charset_under_utf8_parent_is_shown_and_blocks_saving() {
        let path = temp_path("item-ir13-under-utf8.dcm");
        let rest = [
            explicit_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
            roi_sequence(Some(b"ISO_IR 13"), HAN_IR13),
        ]
        .concat();
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        let saved = save_unchanged(&path);
        std::fs::remove_file(&path).ok();

        // Editable, as splicing can write it (#8); the full writer still refuses.
        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&content.nodes, "(0010,0010)"),
            ("山田^太郎".to_string(), true)
        );
        assert!(!has_warning(&content, TEXT_READ_ONLY_WARNING));
        assert!(!has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
        let error = saved.unwrap_err();
        assert!(
            error.contains("(3006,0020) / Item#0 / (3006,0026)"),
            "{error}"
        );
    }

    /// dicom-rs keeps an item's character set for the rest of the stream. Text after the
    /// sequence is shown with the parent's character set and still saved byte for byte.
    #[test]
    fn item_charset_does_not_leak_into_following_parent_elements() {
        let path = temp_path("item-latin1-under-utf8.dcm");
        let reviewer = "鈴木^花子".as_bytes();
        let rest = [
            roi_sequence(Some(b"ISO_IR 100"), b"Caf\xe9"),
            explicit_element(0x300E, 0x0008, b"PN", reviewer),
        ]
        .concat();
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        let written = save_unchanged(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("Café".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&content.nodes, "(300E,0008)"),
            ("鈴木^花子".to_string(), true)
        );
        assert!(!has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
        assert!(contains(&written, reviewer));
        assert!(contains(&written, b"Caf\xe9"));
    }

    /// A character set given only inside a sequence item is decoded, and its text stays
    /// editable because dicom-rs reads and writes it byte for byte.
    #[test]
    fn decodes_a_charset_declared_only_in_an_item() {
        let path = temp_path("item-only-charset.dcm");
        write_part10(&path, None, &roi_sequence(Some(b"ISO_IR 13"), HAN_IR13));
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
        assert!(!has_warning(&content, TEXT_READ_ONLY_WARNING));

        let raw_path = temp_path("item-only-charset-raw.dcm");
        let mut item = implicit_element(0x0008, 0x0005, b"ISO_IR 13");
        item.extend(implicit_element(0x3006, 0x0026, HAN_IR13));
        let dataset = [
            implicit_element(0x0008, 0x0060, b"RTSTRUCT"),
            sequence(0x3006, 0x0020, &[item], false),
        ]
        .concat();
        std::fs::write(&raw_path, dataset).unwrap();
        let content = open_nodes(raw_path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&raw_path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
        assert!(!has_warning(&content, TEXT_READ_ONLY_WARNING));
    }

    /// Review P2: JIS X 0201 Romaji yen sign and overline without escape sequences.
    #[test]
    fn displays_jis_roman_yen_sign_and_overline() {
        let path = temp_path("ir13-st.dcm");
        let rest = explicit_element(0x3006, 0x0006, b"ST", b"100\\~");
        write_part10(&path, Some(b"ISO_IR 13"), &rest);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        let written = save_unchanged(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0006)"),
            ("100¥‾".to_string(), true)
        );
        assert!(contains(&written, b"100\\~"));
    }

    fn explicit_long_element(group: u16, element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
        let mut value = value.to_vec();
        if value.len() % 2 == 1 {
            value.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_le_bytes());
        out.extend_from_slice(&element.to_le_bytes());
        out.extend_from_slice(vr);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(&value);
        out
    }

    fn find_element_mut<'a>(
        nodes: &'a mut [DicomNode],
        wanted_path: &[&str],
    ) -> Option<&'a mut String> {
        nodes.iter_mut().find_map(|node| match node {
            DicomNode::Element { path, value, .. } if path == wanted_path => Some(value),
            DicomNode::Sequence { items, .. } => items
                .iter_mut()
                .find_map(|item| find_element_mut(item, wanted_path)),
            _ => None,
        })
    }

    /// Review P1 (2nd round): an unedited multi-valued Specific Character Set in an item
    /// must be saved as read, so that dicom-rs switches codecs the same way on writing.
    #[test]
    fn unedited_multi_valued_item_charset_is_saved_as_read() {
        let path = temp_path("item-ir100-ir87-under-utf8.dcm");
        let roi_name = b"Caf\xe9 \x1b$B;3ED\x1b(B";
        let rest = roi_sequence(Some(b"ISO 2022 IR 100\\ISO 2022 IR 87"), roi_name);
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        let written = save_unchanged(&path);
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("Café 山田".to_string(), true)
        );
        let written = written.unwrap();
        assert!(contains(&written, roi_name));
        assert!(contains(&written, b"ISO 2022 IR 100\\ISO 2022 IR 87"));
    }

    /// Saving is checked against the declarations that will be written.
    #[test]
    fn editing_an_item_charset_blocks_saving_read_only_text() {
        let path = temp_path("item-charset-edited.dcm");
        let rest = roi_sequence(Some(b"ISO 2022 IR 100\\ISO 2022 IR 87"), b"Caf\xe9");
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(0008,0005)"]).unwrap() =
            "ISO_IR 192".to_string();
        let saved = save_nodes(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(
            error.contains("(3006,0020) / Item#0 / (3006,0026)"),
            "{error}"
        );
    }

    /// Review P2 (2nd round): values larger than 16 MiB are checked too.
    #[test]
    fn large_text_values_are_checked_before_saving() {
        let path = temp_path("item-large-ut.dcm");
        let mut value = vec![b'A'; 16 * 1024 * 1024 + 2];
        value.extend_from_slice(HAN_IR13);
        let mut item = explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13");
        item.extend(explicit_long_element(0x0040, 0xA160, b"UT", &value));
        let rest = sequence(0x0040, 0xA730, &[item], true);
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let saved = save_unchanged(&path);
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(
            error.contains("(0040,A730) / Item#0 / (0040,A160)"),
            "{error}"
        );
    }

    fn remove_element(nodes: &mut Vec<DicomNode>, tag_to_remove: &str) {
        nodes
            .retain(|node| !matches!(node, DicomNode::Element { tag, .. } if tag == tag_to_remove));
    }

    /// Review P1 (3rd round): changing or removing a declaration so that unedited
    /// read-only text would mean something else blocks saving, including in items that
    /// inherit the declaration.
    #[test]
    fn changing_the_declaration_of_unedited_text_blocks_saving() {
        let path = temp_path("ir13-declaration-edited.dcm");
        write_part10(&path, Some(b"ISO_IR 13"), &roi_sequence(None, HAN_IR13));
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        assert_eq!(
            value_and_editable(&opened, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );

        let mut changed = opened.clone();
        *find_element_mut(&mut changed, &["(0008,0005)"]).unwrap() = "ISO_IR 100".to_string();
        let changed_result = save_nodes(&path, &changed);

        let mut removed = opened.clone();
        remove_element(&mut removed, "(0008,0005)");
        let removed_result = save_nodes(&path, &removed);

        let unchanged_result = save_nodes(&path, &opened);
        std::fs::remove_file(&path).ok();

        for result in [changed_result, removed_result] {
            let error = result.unwrap_err();
            assert!(
                error.contains("(3006,0020) / Item#0 / (3006,0026)"),
                "{error}"
            );
        }
        assert!(contains(&unchanged_result.unwrap(), HAN_IR13));
    }

    /// Review P2 (3rd round): a rejected save leaves the cached object untouched.
    #[test]
    fn rejected_save_does_not_change_the_object() {
        let path = temp_path("rejected-save.dcm");
        let rest = roi_sequence(Some(b"ISO 2022 IR 100\\ISO 2022 IR 87"), b"Caf\xe9");
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(0008,0005)"]).unwrap() =
            "ISO_IR 192".to_string();
        *find_element_mut(&mut nodes, &["(0008,0060)"]).unwrap() = "CT".to_string();
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        let destination = temp_path("rejected-save-destination.dcm");
        let result = save_object_with_nodes(&mut obj, &nodes, &text, destination.to_str().unwrap());
        std::fs::remove_file(&path).ok();

        assert!(result.is_err());
        assert!(!destination.exists());
        assert_eq!(
            get_string(&obj, tags::MODALITY).as_deref(),
            Some("RTSTRUCT")
        );
    }

    /// Review P2 (4th round): edited text must read back as entered under the declared
    /// character set.
    #[test]
    fn edited_text_that_would_read_back_differently_blocks_saving() {
        let path = temp_path("ir13-edited-text.dcm");
        write_part10(&path, Some(b"ISO_IR 13"), &roi_sequence(None, b"PTV"));
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let roi_name = ["(3006,0020)", "Item#0", "(3006,0026)"];

        let mut pound = opened.clone();
        *find_element_mut(&mut pound, &roi_name).unwrap() = "£".to_string();
        let pound_result = save_nodes(&path, &pound);

        let mut ascii = opened.clone();
        *find_element_mut(&mut ascii, &roi_name).unwrap() = "PTV2".to_string();
        let ascii_result = save_nodes(&path, &ascii);
        std::fs::remove_file(&path).ok();

        let error = pound_result.unwrap_err();
        assert!(
            error.contains("(3006,0020) / Item#0 / (3006,0026)"),
            "{error}"
        );
        assert!(contains(&ascii_result.unwrap(), b"PTV2"));
    }

    /// Review P1 (4th round): saving over a file never leaves it truncated.
    #[test]
    fn failed_save_keeps_the_existing_destination_file() {
        let dir = temp_test_dir("overwrite");
        let path = dir.join("ir13-overwrite.dcm");
        let rest = explicit_element(0x0010, 0x0020, b"LO", b"ID001");
        write_part10(&path, Some(b"ISO_IR 13"), &rest);
        let path_text = path.to_str().unwrap();
        let before = std::fs::read(&path).unwrap();

        // Text the declared character set cannot hold is rejected before writing.
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "患者".to_string();
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        assert!(save_object_with_nodes(&mut obj, &nodes, &text, path_text).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);

        std::fs::remove_file(&path).ok();
        assert!(dir_is_empty(&dir));
        std::fs::remove_dir(&dir).ok();
    }

    /// A directory used by one test only, so that it can be checked for leftovers.
    fn temp_test_dir(name: &str) -> std::path::PathBuf {
        let dir = temp_path(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Whether `dir` is empty, i.e. no temporary file was left behind.
    fn dir_is_empty(dir: &std::path::Path) -> bool {
        std::fs::read_dir(dir).unwrap().next().is_none()
    }

    /// Review P1 (5th round): errors from the final buffered write are not lost.
    #[test]
    fn write_errors_from_dropped_buffers_are_reported() {
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("disk full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = ErrorRecordingWriter {
            inner: FailingWriter,
            error: None,
        };
        // Like dicom-rs: buffer the output and drop the buffer without flushing it.
        {
            let mut buffered = std::io::BufWriter::new(&mut writer);
            std::io::Write::write_all(&mut buffered, b"DICM").unwrap();
        }
        assert_eq!(writer.error.as_deref(), Some("disk full"));
    }

    /// Review P1 (5th round): saving over a file keeps its access permissions.
    #[cfg(unix)]
    #[test]
    fn saving_keeps_the_permissions_of_the_replaced_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_test_dir("permissions");
        let path = dir.join("permissions.dcm");
        write_part10(&path, Some(b"ISO_IR 100"), &roi_sequence(None, b"PTV"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let path_text = path.to_str().unwrap();
        let nodes = open_nodes(path_text).unwrap().nodes;
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        save_object_with_nodes(&mut obj, &nodes, &text, path_text).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        std::fs::remove_file(&path).ok();

        assert_eq!(mode, 0o600);
        assert!(dir_is_empty(&dir));
        std::fs::remove_dir(&dir).ok();
    }

    /// Review P2 (6th round): files with names close to the 255-byte limit can be saved.
    #[test]
    fn saving_over_a_file_with_a_long_name() {
        let dir = temp_test_dir("long-name");
        let path = dir.join(format!("{}.dcm", "a".repeat(246)));
        write_part10(&path, Some(b"ISO_IR 100"), &roi_sequence(None, b"PTV"));
        let path_text = path.to_str().unwrap();
        let nodes = open_nodes(path_text).unwrap().nodes;
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        let saved = save_object_with_nodes(&mut obj, &nodes, &text, path_text);
        let written = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        saved.unwrap();
        assert!(contains(&written, b"PTV"));
        assert!(dir_is_empty(&dir));
        std::fs::remove_dir(&dir).ok();
    }

    /// Review P2 (5th round): an unrelated file with a temporary-looking name is kept.
    #[test]
    fn saving_does_not_touch_other_files() {
        let path = temp_path("unrelated.dcm");
        write_part10(&path, Some(b"ISO_IR 100"), &roi_sequence(None, b"PTV"));
        let unrelated = path.with_file_name(format!(
            ".{}.taglet-saving",
            path.file_name().unwrap().to_string_lossy()
        ));
        std::fs::write(&unrelated, b"keep me").unwrap();
        let saved = save_unchanged(&path);
        let kept = std::fs::read(&unrelated);
        std::fs::remove_file(&unrelated).ok();
        std::fs::remove_file(&path).ok();

        saved.unwrap();
        assert_eq!(kept.unwrap(), b"keep me");
    }

    /// Save `nodes` over a copy of `source` and reopen it.
    fn save_and_reopen(
        source: &std::path::Path,
        nodes: &[DicomNode],
    ) -> Result<(Vec<u8>, DicomFileContent), String> {
        let source_text = source.to_str().unwrap();
        let text = TextValues::try_read(source_text)?;
        let mut obj = open_full_object(source_text)?;
        let destination = source.with_extension("reopened.dcm");
        let destination_text = destination.to_str().unwrap();
        let result =
            save_object_with_nodes(&mut obj, nodes, &text, destination_text).and_then(|()| {
                let written = std::fs::read(&destination).map_err(|error| error.to_string())?;
                Ok((written, open_nodes(destination_text)?))
            });
        std::fs::remove_file(&destination).ok();
        result
    }

    /// Phase 2: Japanese text is edited and saved with ISO 2022 escape sequences.
    #[test]
    fn edited_japanese_text_is_saved_in_jis() {
        let path = temp_path("phase2-edit.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let name = "Suzuki^Hanako=鈴木^花子=すずき^はなこ";
        *find_element_mut(&mut nodes, &["(0010,0010)"]).unwrap() = name.to_string();
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "膀胱".to_string();
        let saved = save_and_reopen(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let (written, reopened) = saved.unwrap();
        let charset = TextCharset::from_specific_character_set("\\ISO 2022 IR 87");
        assert!(contains(&written, &charset.encode(VR::PN, name).unwrap()));
        assert!(contains(&written, b"\x1b$Bg/fy\x1b(B"));
        assert_eq!(
            value_and_editable(&reopened.nodes, "(0010,0010)"),
            (name.to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened.nodes, "(3006,0026)"),
            ("膀胱".to_string(), true)
        );
    }

    /// Phase 2: half-width katakana and kanji are saved as in PS3.5 Annex H.3.2.
    #[test]
    fn edited_text_matches_annex_h_3_2() {
        let path = temp_path("phase2-h32.dcm");
        let rest = explicit_element(0x0010, 0x0010, b"PN", b"Yamada^Tarou");
        write_part10(&path, Some(b"ISO 2022 IR 13\\ISO 2022 IR 87"), &rest);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0010)"]).unwrap() =
            "ﾔﾏﾀﾞ^ﾀﾛｳ=山田^太郎=やまだ^たろう".to_string();
        let saved = save_and_reopen(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let h32: &[u8] = b"\xd4\xcf\xc0\xde^\xc0\xdb\xb3=\x1b$B;3ED\x1b(J^\x1b$BB@O:\x1b(J=\x1b$B$d$^$@\x1b(J^\x1b$B$?$m$&\x1b(J";
        assert!(contains(&saved.unwrap().0, h32));
    }

    /// Phase 2: an added element and multiple values, one with a 0x5C byte in a kanji.
    #[test]
    fn added_and_multi_valued_japanese_text_is_saved() {
        let path = temp_path("phase2-added.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let (kanji, _) = encoding_rs::EUC_JP.decode_without_bom_handling(&[0xBF, 0xDC]);
        let values = format!("頭部{kanji}\\ABC");
        nodes.push(DicomNode::Element {
            tag: "(0008,1030)".to_string(),
            vr: "LO".to_string(),
            inferred_vr: None,
            description: String::new(),
            value: values.clone(),
            length: 0,
            path: vec!["(0008,1030)".to_string()],
            editable: true,
        });
        let saved = save_and_reopen(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let (_, reopened) = saved.unwrap();
        assert_eq!(
            value_and_editable(&reopened.nodes, "(0008,1030)"),
            (values, true)
        );
    }

    /// Phase 2: text the declared character sets cannot hold is not saved.
    #[test]
    fn unencodable_edits_block_saving() {
        let path = temp_path("phase2-unencodable.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let results = ["😀", "ｶﾅ", "丂"].map(|text| {
            let mut nodes = opened.clone();
            *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = text.to_string();
            save_and_reopen(&path, &nodes)
        });
        std::fs::remove_file(&path).ok();

        for result in results {
            let error = result.unwrap_err();
            assert!(error.contains("(0010,0020)"), "{error}");
        }
    }

    /// Text where dicom-rs does not write byte for byte cannot be saved by the full
    /// writer in a Japanese character set; splicing saves it (#8).
    #[test]
    fn japanese_edits_under_a_utf8_codec_need_splicing() {
        let path = temp_path("phase2-utf8-parent.dcm");
        let rest = roi_sequence(Some(b"\\ISO 2022 IR 87"), PROSTATE_IR87);
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let roi_name = ["(3006,0020)", "Item#0", "(3006,0026)"];
        let shown = value_and_editable(&nodes, "(3006,0026)");
        // Force an edit as if the value were editable.
        if let Some(DicomNode::Element {
            editable, value, ..
        }) = nodes.iter_mut().find_map(|node| match node {
            DicomNode::Sequence { items, .. } => items[0]
                .iter_mut()
                .find(|node| matches!(node, DicomNode::Element { path, .. } if path == &roi_name)),
            _ => None,
        }) {
            *editable = true;
            *value = "膀胱".to_string();
        }
        let full = save_and_reopen(&path, &nodes);
        let spliced = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert_eq!(shown, ("前立腺".to_string(), true));
        assert!(full
            .unwrap_err()
            .contains("(3006,0020) / Item#0 / (3006,0026)"));
        assert!(contains(&spliced.unwrap(), b"\x1b$Bg/fy\x1b(B"));
    }

    /// Review on #9: saving again right after a save works, also with kanji whose JIS
    /// second byte is 0x5C (e.g. 本, 宮, ボ).
    #[test]
    fn saving_twice_keeps_kanji_with_a_backslash_byte() {
        let dir = temp_test_dir("save-twice");
        let path = dir.join("save-twice.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "宮本\\ABC".to_string();
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "ボーラス".to_string();
        let mut obj = open_full_object(path_text).unwrap();

        let first = TextValues::try_read(path_text).unwrap();
        save_object_with_nodes(&mut obj, &nodes, &first, path_text).unwrap();
        let after_first = std::fs::read(&path).unwrap();
        let second = TextValues::try_read(path_text).unwrap();
        let result = save_object_with_nodes(&mut obj, &nodes, &second, path_text);
        let after_second = std::fs::read(&path).unwrap();
        let reopened = open_nodes(path_text).unwrap().nodes;
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        result.unwrap();
        assert_eq!(after_second, after_first);
        assert_eq!(
            value_and_editable(&reopened, "(0010,0020)"),
            ("宮本\\ABC".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ボーラス".to_string(), true)
        );
    }

    /// Review on #9: kanji in the first (alphabetic) Person Name group is rejected with
    /// guidance; after '=' it is saved.
    #[test]
    fn kanji_in_the_first_person_name_group_blocks_saving() {
        let path = temp_path("pn-first-group.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let save_name = |name: &str| {
            let mut nodes = opened.clone();
            *find_element_mut(&mut nodes, &["(0010,0010)"]).unwrap() = name.to_string();
            save_and_reopen(&path, &nodes)
        };
        let first_group = save_name("山田^太郎");
        let second_group = save_name("=山田^太郎");
        std::fs::remove_file(&path).ok();

        let error = first_group.unwrap_err();
        assert!(error.contains("(0010,0010)"), "{error}");
        assert!(error.contains("first (alphabetic) name group"), "{error}");
        assert_eq!(
            value_and_editable(&second_group.unwrap().1.nodes, "(0010,0010)"),
            ("=山田^太郎".to_string(), true)
        );
    }

    /// Review on #9: a single-valued "ISO_IR 13" gets no escape sequences.
    #[test]
    fn single_valued_ir13_rejects_characters_needing_escape_sequences() {
        let path = temp_path("ir13-tilde.dcm");
        write_part10(&path, Some(b"ISO_IR 13"), &roi_sequence(None, b"PTV"));
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "PTV~".to_string();
        let saved = save_and_reopen(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(error.contains("single-valued"), "{error}");
    }

    /// Review on #9: an edit that equals dicom-rs's string for the original bytes is
    /// still treated as an edit (here: rejected, as ISO_IR 13 cannot hold it).
    #[test]
    fn edits_equal_to_dicom_rs_strings_are_not_ignored() {
        let path = temp_path("ir13-latin1-lookalike.dcm");
        write_part10(&path, Some(b"ISO_IR 13"), &roi_sequence(None, HAN_IR13));
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "\u{CA}\u{DD}".to_string();
        let saved = save_and_reopen(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(
            error.contains("(3006,0020) / Item#0 / (3006,0026)"),
            "{error}"
        );
    }

    /// Review on #9: odd-length Japanese text, padded on disk, can be saved again from
    /// the cached object.
    #[test]
    fn saving_odd_length_japanese_text_twice() {
        let dir = temp_test_dir("odd-length");
        let path = dir.join("odd-length.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "山A".to_string();
        let mut obj = open_full_object(path_text).unwrap();

        let first = TextValues::try_read(path_text).unwrap();
        save_object_with_nodes(&mut obj, &nodes, &first, path_text).unwrap();
        let after_first = std::fs::read(&path).unwrap();
        let second = TextValues::try_read(path_text).unwrap();
        let result = save_object_with_nodes(&mut obj, &nodes, &second, path_text);
        let after_second = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        result.unwrap();
        assert_eq!(after_second, after_first);
        assert!(contains(&after_first, b"\x1b$B;3\x1b(BA "));
    }

    /// Review on #9: values longer than a 16-bit length allows are rejected before the
    /// file is replaced.
    #[test]
    fn values_too_long_for_explicit_vr_block_saving() {
        let dir = temp_test_dir("too-long");
        let path = dir.join("too-long.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let path_text = path.to_str().unwrap();
        let before = std::fs::read(&path).unwrap();
        let opened = open_nodes(path_text).unwrap().nodes;
        let add = |vr: &str, tag: &str, value: String| {
            let mut nodes = opened.clone();
            nodes.push(DicomNode::Element {
                tag: tag.to_string(),
                vr: vr.to_string(),
                inferred_vr: None,
                description: String::new(),
                value,
                length: 0,
                path: vec![tag.to_string()],
                editable: true,
            });
            nodes
        };
        let too_long = add("PN", "(0008,1048)", vec!["=山"; 6554].join("\\"));
        let fits = add("LT", "(0008,4000)", "山".repeat(10_000));
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        let rejected = save_object_with_nodes(&mut obj, &too_long, &text, path_text);
        let after_rejected = std::fs::read(&path).unwrap();
        let accepted = save_object_with_nodes(&mut obj, &fits, &text, path_text);
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        let error = rejected.unwrap_err();
        assert!(error.contains("(0008,1048)"), "{error}");
        assert_eq!(after_rejected, before);
        accepted.unwrap();
    }

    // ---- Saving by splicing (#8) ----

    fn defined_sequence(group: u16, element: u16, items: &[Vec<u8>]) -> Vec<u8> {
        let mut content = Vec::new();
        for item in items {
            content.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0]);
            content.extend_from_slice(&(item.len() as u32).to_le_bytes());
            content.extend_from_slice(item);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_le_bytes());
        out.extend_from_slice(&element.to_le_bytes());
        out.extend_from_slice(b"SQ\0\0");
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend(content);
        out
    }

    /// Write a Part 10 file with the given transfer syntax and data set bytes.
    fn write_part10_with(path: &std::path::Path, transfer_syntax: &str, dataset: &[u8]) {
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax(transfer_syntax)
            .media_storage_sop_class_uid(RTSTRUCT_SOP_CLASS)
            .media_storage_sop_instance_uid("1.2.3.4")
            .build()
            .unwrap();
        let mut file = Vec::new();
        file.extend_from_slice(&[0; 128]);
        file.extend_from_slice(b"DICM");
        meta.write(&mut file).unwrap();
        file.extend_from_slice(dataset);
        std::fs::write(path, file).unwrap();
    }

    fn base_elements(charset: &[u8]) -> Vec<u8> {
        [
            explicit_element(0x0008, 0x0005, b"CS", charset),
            explicit_element(0x0008, 0x0016, b"UI", RTSTRUCT_SOP_CLASS.as_bytes()),
            explicit_element(0x0008, 0x0018, b"UI", b"1.2.3.4"),
            explicit_element(0x0008, 0x0060, b"CS", b"RTSTRUCT"),
        ]
        .concat()
    }

    /// A file the full writer refuses: UTF-8 overall, with an ISO_IR 13 item whose
    /// bytes dicom-rs cannot write back. Uses a defined-length and an undefined-length
    /// sequence, and native pixel data.
    fn write_mixed_charset_file(path: &std::path::Path) -> Vec<u8> {
        let item = [
            explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
            explicit_element(0x3006, 0x0022, b"IS", b"1"),
            explicit_element(0x3006, 0x0026, b"LO", HAN_IR13),
        ]
        .concat();
        let mut pixel = Vec::new();
        pixel.extend_from_slice(&[0xE0, 0x7F, 0x10, 0x00]);
        pixel.extend_from_slice(b"OW\0\0");
        pixel.extend_from_slice(&8_u32.to_le_bytes());
        pixel.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            defined_sequence(0x3006, 0x0020, &[item]),
            sequence(
                0x3006,
                0x0039,
                &[explicit_element(0x3006, 0x0084, b"IS", b"1")],
                true,
            ),
            pixel,
        ]
        .concat();
        write_part10_with(path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        std::fs::read(path).unwrap()
    }

    fn save_file(source: &std::path::Path, nodes: &[DicomNode]) -> Result<Vec<u8>, String> {
        let source_text = source.to_str().unwrap();
        let text = TextValues::try_read(source_text)?;
        let mut obj = open_full_object(source_text)?;
        let destination = source.with_extension("spliced.dcm");
        let result = save_file_with_nodes(
            source_text,
            &mut obj,
            nodes,
            &text,
            destination.to_str().unwrap(),
            splice::file_stamp(source_text),
            false,
        )
        .and_then(|_| std::fs::read(&destination).map_err(|error| error.to_string()));
        std::fs::remove_file(&destination).ok();
        result
    }

    #[test]
    fn splice_without_edits_keeps_the_file_byte_for_byte() {
        let path = temp_path("splice-identity.dcm");
        let original = write_mixed_charset_file(&path);
        let nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert_eq!(saved.unwrap(), original);
    }

    /// #8 completion criterion: a file the full writer refuses can be saved after
    /// editing another value, keeping the item's original bytes.
    #[test]
    fn splice_saves_a_file_the_full_writer_refuses() {
        let path = temp_path("splice-mixed.dcm");
        let original = write_mixed_charset_file(&path);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0010)"]).unwrap() = "鈴木^花子".to_string();

        // The full writer refuses this file.
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        let destination = temp_path("splice-mixed-full.dcm");
        assert!(
            save_object_with_nodes(&mut obj, &nodes, &text, destination.to_str().unwrap()).is_err()
        );

        let saved = save_file(&path, &nodes);
        let reopened_path = temp_path("splice-mixed-reopened.dcm");
        let saved = saved.unwrap();
        std::fs::write(&reopened_path, &saved).unwrap();
        let reopened = open_nodes(reopened_path.to_str().unwrap()).unwrap().nodes;
        std::fs::remove_file(&reopened_path).ok();
        std::fs::remove_file(&path).ok();

        // Everything from the sequence on is unchanged; only the name and its length
        // field differ before it.
        let item_start = original
            .windows(4)
            .position(|window| window == [0x06, 0x30, 0x20, 0x00])
            .unwrap();
        let shift = "鈴木^花子".len() as isize - "山田^太郎".len() as isize;
        let saved_start = (item_start as isize + shift) as usize;
        assert_eq!(&saved[saved_start..], &original[item_start..]);
        assert!(contains(&saved, "鈴木^花子".as_bytes()));
        assert_eq!(
            value_and_editable(&reopened, "(0010,0010)"),
            ("鈴木^花子".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
    }

    #[test]
    fn splice_updates_lengths_of_defined_length_items() {
        let path = temp_path("splice-defined.dcm");
        let item = [
            explicit_element(0x3006, 0x0022, b"IS", b"1"),
            explicit_element(0x3006, 0x0026, b"LO", PROSTATE_IR87),
        ]
        .concat();
        let dataset = [
            base_elements(b"\\ISO 2022 IR 87"),
            explicit_element(0x0010, 0x0010, b"PN", YAMADA_IR87),
            defined_sequence(0x3006, 0x0020, &[item]),
            explicit_element(0x300E, 0x0002, b"CS", b"APPROVED"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "前立腺 PTV 70Gy".to_string();
        let saved = save_file(&path, &nodes).unwrap();
        let reopened_path = temp_path("splice-defined-reopened.dcm");
        std::fs::write(&reopened_path, &saved).unwrap();
        let reopened = open_nodes(reopened_path.to_str().unwrap()).unwrap().nodes;
        std::fs::remove_file(&reopened_path).ok();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("前立腺 PTV 70Gy".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(300E,0002)"),
            ("APPROVED".to_string(), true)
        );
        assert!(contains(&saved, YAMADA_IR87));
    }

    #[test]
    fn splice_keeps_encapsulated_pixel_data() {
        let path = temp_path("splice-encapsulated.dcm");
        let mut pixel = Vec::new();
        pixel.extend_from_slice(&[0xE0, 0x7F, 0x10, 0x00]);
        pixel.extend_from_slice(b"OB\0\0");
        pixel.extend_from_slice(&u32::MAX.to_le_bytes());
        // Basic offset table (empty) and one fragment.
        pixel.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0, 0, 0, 0, 0]);
        pixel.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0, 4, 0, 0, 0, 0xFF, 0xD8, 0xFF, 0xD9]);
        pixel.extend_from_slice(&[0xFE, 0xFF, 0xDD, 0xE0, 0, 0, 0, 0]);
        let dataset = [
            base_elements(b"\\ISO 2022 IR 87"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            pixel.clone(),
        ]
        .concat();
        write_part10_with(&path, "1.2.840.10008.1.2.4.50", &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "患者01".to_string();
        let path_text = path.to_str().unwrap();
        let planned = plan_splice(
            path_text,
            &open_full_object(path_text).unwrap(),
            &nodes,
            &TextValues::try_read(path_text).unwrap(),
            splice::file_stamp(path_text).unwrap(),
        )
        .unwrap();
        assert!(planned.is_some(), "saving should splice");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert!(saved.ends_with(&pixel));
        assert!(contains(&saved, b"\x1b$B45<T\x1b(B01"));
    }

    #[test]
    fn added_elements_are_spliced() {
        let path = temp_path("splice-fallback.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;

        // Adding an element.
        let mut added = opened.clone();
        added.push(DicomNode::Element {
            tag: "(0008,1030)".to_string(),
            vr: "LO".to_string(),
            inferred_vr: None,
            description: String::new(),
            value: "頭部".to_string(),
            length: 0,
            path: vec!["(0008,1030)".to_string()],
            editable: true,
        });
        let path_text = path.to_str().unwrap();
        let object = open_full_object(path_text).unwrap();
        let text = TextValues::try_read(path_text).unwrap();
        let stamp = splice::file_stamp(path_text).unwrap();
        assert!(plan_splice(path_text, &object, &added, &text, stamp)
            .unwrap()
            .is_some());
        let added_result = save_file(&path, &added);
        std::fs::remove_file(&path).ok();

        assert!(contains(&added_result.unwrap(), b"\x1b$BF,It\x1b(B"));
    }

    // ---- Changing Specific Character Set: the text is converted ----

    fn set_value(nodes: &mut [DicomNode], path: &[&str], value: &str) {
        *find_element_mut(nodes, path).unwrap() = value.to_string();
    }

    #[test]
    fn changing_the_character_set_converts_text_to_it() {
        let path = temp_path("convert-to-utf8.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "ISO_IR 192");
        // An edit in the same save is written in the new character set too.
        set_value(&mut nodes, &["(0010,0020)"], "患者01");
        let spliced = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(spliced);
        let saved = saved.unwrap();
        assert!(
            !saved.contains(&0x1B),
            "no ISO 2022 escape sequences remain"
        );
        assert!(contains(
            &saved,
            "Yamada^Tarou=山田^太郎=やまだ^たろう".as_bytes()
        ));
        assert!(contains(&saved, "前立腺".as_bytes()));
        assert!(contains(&saved, "患者01".as_bytes()));
        let reopened = reopen("convert-to-utf8-reopened.dcm", &saved);
        assert_eq!(
            value_and_editable(&reopened, "(0010,0010)").0,
            "Yamada^Tarou=山田^太郎=やまだ^たろう"
        );
        assert_eq!(value_and_editable(&reopened, "(3006,0026)").0, "前立腺");
        assert_eq!(value_and_editable(&reopened, "(0010,0020)").0, "患者01");
    }

    #[test]
    fn converting_to_iso_2022_follows_its_rules() {
        let path = temp_path("convert-to-jis.dcm");
        let rest = [
            explicit_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
            roi_sequence(None, "前立腺".as_bytes()),
        ]
        .concat();
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let original = std::fs::read(&path).unwrap();
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "\\ISO 2022 IR 87");
        // Kanji cannot be in the first (alphabetic) name group with code extensions.
        let first_group = save_file(&path, &nodes);
        set_value(&mut nodes, &["(0010,0010)"], "Yamada^Taro=山田^太郎");
        let saved = save_file(&path, &nodes);
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let error = first_group.unwrap_err();
        assert!(
            error.contains("(0010,0010)") && error.contains("first (alphabetic) name group"),
            "{error}"
        );
        let saved = saved.unwrap();
        assert!(contains(&saved, PROSTATE_IR87));
        assert!(contains(
            &saved,
            b"Yamada^Taro=\x1b$B;3ED\x1b(B^\x1b$BB@O:\x1b(B"
        ));
        let reopened = reopen("convert-to-jis-reopened.dcm", &saved);
        assert_eq!(value_and_editable(&reopened, "(3006,0026)").0, "前立腺");
        assert_eq!(after, original);
    }

    /// Items declaring their own character set keep it when the data set's changes, and
    /// changing an item's converts only that item.
    #[test]
    fn character_set_changes_follow_items() {
        let path = temp_path("convert-items.dcm");
        write_mixed_charset_file(&path);
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let mut root = opened.clone();
        set_value(&mut root, &["(0008,0005)"], "\\ISO 2022 IR 87");
        set_value(&mut root, &["(0010,0010)"], "Suzuki^Hanako=鈴木^花子");
        let root_saved = save_file(&path, &root);
        let mut item = opened.clone();
        set_value(
            &mut item,
            &["(3006,0020)", "Item#0", "(0008,0005)"],
            "ISO_IR 192",
        );
        let item_saved = save_file(&path, &item);
        std::fs::remove_file(&path).ok();

        let root_saved = root_saved.unwrap();
        assert!(contains(&root_saved, b"\x1b$BNkLZ\x1b(B^\x1b$B2V;R\x1b(B"));
        assert!(
            contains(&root_saved, HAN_IR13),
            "the ISO_IR 13 item is kept"
        );
        let item_saved = item_saved.unwrap();
        assert!(!contains(&item_saved, HAN_IR13));
        assert!(contains(&item_saved, "ﾊﾝ".as_bytes()));
        assert!(contains(&item_saved, "山田^太郎".as_bytes()));
        let reopened = reopen("convert-items-reopened.dcm", &item_saved);
        assert_eq!(value_and_editable(&reopened, "(3006,0026)").0, "ﾊﾝ");
    }

    #[test]
    fn text_that_cannot_be_converted_blocks_saving() {
        let path = temp_path("convert-fails.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let original = std::fs::read(&path).unwrap();
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        // Removing the declaration leaves the default repertoire (ASCII).
        let removed = opened
            .iter()
            .filter(|node| !matches!(node, DicomNode::Element { tag, .. } if tag == "(0008,0005)"))
            .cloned()
            .collect::<Vec<_>>();
        let removed = save_file(&path, &removed);
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let error = removed.unwrap_err();
        assert!(error.contains("(0010,0010)"), "{error}");
        assert!(
            error.contains("(3006,0020) / Item#0 / (3006,0026)"),
            "{error}"
        );
        assert!(error.contains("cannot be written"), "{error}");
        assert_eq!(after, original);
    }

    /// Text in items after Pixel Data is not read; when it would take the converted
    /// character set, saving stops instead of leaving it in the old one.
    #[test]
    fn unread_text_after_pixel_data_blocks_conversion() {
        let path = temp_path("convert-trailing.dcm");
        let mut pixel = Vec::new();
        pixel.extend_from_slice(&[0xE0, 0x7F, 0x10, 0x00]);
        pixel.extend_from_slice(b"OW\0\0");
        pixel.extend_from_slice(&4_u32.to_le_bytes());
        pixel.extend_from_slice(&[1, 2, 3, 4]);
        let rest = [
            explicit_element(0x0010, 0x0010, b"PN", YAMADA_IR87),
            pixel,
            explicit_element(0x7FE1, 0x0010, b"LO", b"VENDOR"),
            sequence(
                0x7FE1,
                0x1001,
                &[explicit_element(0x7FE1, 0x1002, b"LO", PROSTATE_IR87)],
                true,
            ),
        ]
        .concat();
        write_part10(&path, Some(b"\\ISO 2022 IR 87"), &rest);
        let original = std::fs::read(&path).unwrap();
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0010)"], "Yamada^Tarou");
        let unchanged_charset = save_file(&path, &nodes);
        set_value(&mut nodes, &["(0008,0005)"], "ISO_IR 192");
        let converted = save_file(&path, &nodes);
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(unchanged_charset.is_ok());
        let error = converted.unwrap_err();
        assert!(
            error.contains("(7FE1,1001) / Item#0 / (7FE1,1002) (it is not read"),
            "{error}"
        );
        assert_eq!(after, original);
    }

    #[test]
    fn unknown_declarations_are_left_to_the_full_writer_even_when_read_alike() {
        let path = temp_path("convert-unknown-alike.dcm");
        write_part10(
            &path,
            None,
            &explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        );
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        nodes.insert(0, element_node("(0008,0005)", "CS", "ISO IR 192", &[]));
        let spliced = plans_splice(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
    }

    /// Declarations with terms Taglet does not know are not converted from, while
    /// known spellings it does not write (a lone "ISO 2022 IR 87") are.
    #[test]
    fn declarations_taglet_cannot_read_are_not_converted_from() {
        let path = temp_path("convert-from-unknown.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87\\ISO 2022 IR 149");
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "\\ISO 2022 IR 87");
        let unknown = plans_splice(&path, &nodes);
        write_iso2022_rtstruct(&path, "ISO 2022 IR 87");
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "ISO_IR 192");
        let lone = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!unknown);
        assert!(contains(&lone.unwrap(), "前立腺".as_bytes()));
    }

    #[test]
    fn bytes_invalid_in_the_current_character_set_are_not_converted() {
        let path = temp_path("convert-invalid.dcm");
        let rest = explicit_element(0x0010, 0x0020, b"LO", b"ID\xff\xfe");
        write_part10(&path, Some(b"ISO_IR 192"), &rest);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "ISO_IR 100");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(
            error.contains("(0010,0020)")
                && error.contains("not valid in the current character set"),
            "{error}"
        );
    }

    /// A declaration dicom-rs does not know (here a misspelling of ISO_IR 192) is not
    /// taken as the character set it resembles: the text is neither left unconverted
    /// nor garbled.
    #[test]
    fn misspelled_declarations_do_not_garble_text() {
        let path = temp_path("convert-misspelled.dcm");
        write_part10(
            &path,
            Some(b"ISO_IR 192"),
            &explicit_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
        );
        let original = std::fs::read(&path).unwrap();
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "ISO IR 192");
        let saved = save_file(&path, &nodes);
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(saved.is_err());
        assert_eq!(after, original);
    }

    /// ISO 2022 values may designate their G1 set themselves (here ESC ) I).
    #[test]
    fn text_designating_its_own_g1_set_is_converted() {
        let path = temp_path("convert-g1.dcm");
        write_part10(
            &path,
            Some(b"\\ISO 2022 IR 13"),
            &explicit_element(0x0010, 0x0020, b"LO", b"\x1b)I\xca\xdd"),
        );
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        assert_eq!(value_and_editable(&nodes, "(0010,0020)").0, "ﾊﾝ");
        set_value(&mut nodes, &["(0008,0005)"], "ISO_IR 192");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert!(contains(&saved, "ﾊﾝ".as_bytes()));
        let reopened = reopen("convert-g1-reopened.dcm", &saved);
        assert_eq!(value_and_editable(&reopened, "(0010,0020)").0, "ﾊﾝ");
    }

    #[test]
    fn character_sets_taglet_cannot_write_are_left_to_the_full_writer() {
        let path = temp_path("convert-unsupported.dcm");
        write_part10(
            &path,
            Some(b"ISO_IR 100"),
            &explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        );
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0005)"], "\\ISO 2022 IR 149");
        let spliced = plans_splice(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
    }

    #[test]
    fn implicit_vr_text_is_converted() {
        let path = temp_path("convert-implicit.dcm");
        write_implicit_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(
            &mut nodes,
            &["(3006,0020)", "Item#0", "(0008,0005)"],
            "ISO_IR 192",
        );
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert!(!contains(&saved, HAN_IR13));
        let reopened = reopen("convert-implicit-reopened.dcm", &saved);
        assert_eq!(value_and_editable(&reopened, "(3006,0026)").0, "ﾊﾝ");
        assert_eq!(value_and_editable(&reopened, "(0010,0010)").0, "山田^太郎");
    }

    #[test]
    fn spliced_values_are_checked() {
        let path = temp_path("splice-checks.dcm");
        let original = write_mixed_charset_file(&path);
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let save_with = |path_in_nodes: &[&str], value: String| {
            let mut nodes = opened.clone();
            *find_element_mut(&mut nodes, path_in_nodes).unwrap() = value;
            save_file(&path, &nodes)
        };
        let too_long = save_with(&["(0010,0010)"], "山".repeat(30_000));
        // Non-ASCII text in a CS value is left to the full writer, as before.
        let mut not_ascii = opened.clone();
        *find_element_mut(&mut not_ascii, &["(0008,0060)"]).unwrap() = "ＲＴ".to_string();
        let path_text = path.to_str().unwrap();
        let planned = plan_splice(
            path_text,
            &open_full_object(path_text).unwrap(),
            &not_ascii,
            &TextValues::try_read(path_text).unwrap(),
            splice::file_stamp(path_text).unwrap(),
        )
        .unwrap();
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(too_long.unwrap_err().contains("(0010,0010)"));
        assert!(planned.is_none());
        assert_eq!(after, original);
    }

    // ---- Implicit VR Little Endian ----

    fn implicit_defined_sequence(group: u16, element: u16, items: &[Vec<u8>]) -> Vec<u8> {
        let mut content = Vec::new();
        for item in items {
            content.extend_from_slice(&[0xFE, 0xFF, 0x00, 0xE0]);
            content.extend_from_slice(&(item.len() as u32).to_le_bytes());
            content.extend_from_slice(item);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_le_bytes());
        out.extend_from_slice(&element.to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend(content);
        out
    }

    fn implicit_uid(group: u16, element: u16, uid: &str) -> Vec<u8> {
        let mut value = uid.as_bytes().to_vec();
        if value.len() % 2 == 1 {
            value.push(0);
        }
        implicit_element(group, element, &value)
    }

    /// The Implicit VR counterpart of `write_mixed_charset_file`: UTF-8 overall, with an
    /// ISO_IR 13 item in a defined-length sequence, an undefined-length sequence and
    /// native pixel data.
    fn write_implicit_mixed_charset_file(path: &std::path::Path) -> Vec<u8> {
        let item = [
            implicit_element(0x0008, 0x0005, b"ISO_IR 13"),
            implicit_element(0x3006, 0x0022, b"1"),
            implicit_element(0x3006, 0x0026, HAN_IR13),
        ]
        .concat();
        let dataset = [
            implicit_element(0x0008, 0x0005, b"ISO_IR 192"),
            implicit_uid(0x0008, 0x0016, RTSTRUCT_SOP_CLASS),
            implicit_uid(0x0008, 0x0018, "1.2.3.4"),
            implicit_element(0x0008, 0x0060, b"RTSTRUCT"),
            implicit_element(0x0010, 0x0010, "山田^太郎".as_bytes()),
            implicit_element(0x0010, 0x0020, b"ID001"),
            implicit_defined_sequence(0x3006, 0x0020, &[item]),
            sequence(
                0x3006,
                0x0039,
                &[implicit_element(0x3006, 0x0084, b"1")],
                false,
            ),
            implicit_element(0x7FE0, 0x0010, &[1, 2, 3, 4, 5, 6, 7, 8]),
        ]
        .concat();
        write_part10_with(path, uids::IMPLICIT_VR_LITTLE_ENDIAN, &dataset);
        std::fs::read(path).unwrap()
    }

    fn plans_splice(path: &std::path::Path, nodes: &[DicomNode]) -> bool {
        let path_text = path.to_str().unwrap();
        plan_splice(
            path_text,
            &open_full_object(path_text).unwrap(),
            &with_file_item_paths(nodes, &[]),
            &TextValues::try_read(path_text).unwrap(),
            splice::file_stamp(path_text).unwrap(),
        )
        .unwrap()
        .is_some()
    }

    fn reopen(name: &str, bytes: &[u8]) -> Vec<DicomNode> {
        let path = temp_path(name);
        std::fs::write(&path, bytes).unwrap();
        let nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        std::fs::remove_file(&path).ok();
        nodes
    }

    #[test]
    fn implicit_vr_splice_without_edits_keeps_the_file_byte_for_byte() {
        let path = temp_path("implicit-identity.dcm");
        let original = write_implicit_mixed_charset_file(&path);
        let nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert_eq!(saved.unwrap(), original);
    }

    #[test]
    fn implicit_vr_edits_are_spliced() {
        let path = temp_path("implicit-edit.dcm");
        let original = write_implicit_mixed_charset_file(&path);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0010)"]).unwrap() = "鈴木^花子".to_string();

        // The full writer refuses this file.
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        let destination = temp_path("implicit-edit-full.dcm");
        assert!(
            save_object_with_nodes(&mut obj, &nodes, &text, destination.to_str().unwrap()).is_err()
        );
        assert!(plans_splice(&path, &nodes));
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        let reopened = reopen("implicit-edit-reopened.dcm", &saved);
        // Only the name differs: both names are 13 bytes, padded to 14, so even the
        // length field is unchanged. Changed lengths are covered by the next test.
        let name = "山田^太郎".as_bytes();
        let name_start = original
            .windows(name.len())
            .position(|window| window == name)
            .unwrap();
        assert_eq!(&saved[..name_start], &original[..name_start]);
        assert_eq!(&saved[name_start..name_start + 14], "鈴木^花子 ".as_bytes());
        assert_eq!(&saved[name_start + 14..], &original[name_start + 14..]);
        assert_eq!(
            value_and_editable(&reopened, "(0010,0010)"),
            ("鈴木^花子".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
    }

    #[test]
    fn implicit_vr_splice_updates_lengths_of_defined_length_items() {
        let path = temp_path("implicit-defined.dcm");
        write_implicit_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "ﾊﾝﾀﾞ PTV".to_string();
        assert!(plans_splice(&path, &nodes));
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let reopened = reopen("implicit-defined-reopened.dcm", &saved.unwrap());
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ﾊﾝﾀﾞ PTV".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(3006,0084)"),
            ("1".to_string(), true)
        );
    }

    #[test]
    fn implicit_vr_elements_are_added_and_removed_by_splicing() {
        let path = temp_path("implicit-add-remove.dcm");
        write_implicit_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        nodes
            .retain(|node| !matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0020)"));
        let position = nodes
            .iter()
            .position(|node| matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0010)"))
            .unwrap();
        nodes.insert(position, element_node("(0008,1030)", "LO", "頭部", &[]));
        // Implicit VR has 32-bit lengths, so long values fit.
        let long_name = "山".repeat(30_000);
        *find_element_mut(&mut nodes, &["(0010,0010)"]).unwrap() = long_name.clone();
        assert!(plans_splice(&path, &nodes));
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        let mut added = vec![0x08, 0x00, 0x30, 0x10];
        added.extend_from_slice(&6_u32.to_le_bytes());
        added.extend_from_slice("頭部".as_bytes());
        assert!(contains(&saved, &added));
        assert!(!contains(&saved, b"ID001"));
        let reopened = reopen("implicit-add-remove-reopened.dcm", &saved);
        assert_eq!(
            value_and_editable(&reopened, "(0008,1030)"),
            ("頭部".to_string(), true)
        );
        assert_eq!(value_and_editable(&reopened, "(0010,0010)").0, long_name);
        assert!(find_element(&reopened, "(0010,0020)").is_none());
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
    }

    /// Implicit VR files do not store VRs, so an element added with a VR other than the
    /// dictionary's would read back wrong; as a sequence, the file could not be opened.
    #[test]
    fn implicit_vr_elements_must_use_the_dictionary_vr() {
        let path = temp_path("implicit-vr-check.dcm");
        let original = write_implicit_mixed_charset_file(&path);
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let save_with = |added: DicomNode| {
            let mut nodes = opened.clone();
            nodes.insert(0, added);
            save_file(&path, &nodes)
        };
        let as_sequence = save_with(element_node("(0008,1140)", "LO", "abc", &[]));
        let as_text = save_with(element_node("(0028,0010)", "IS", "512", &[]));
        let in_item = {
            let mut nodes = opened.clone();
            let Some(DicomNode::Sequence { items, .. }) = nodes.iter_mut().find(
                |node| matches!(node, DicomNode::Sequence { tag, .. } if tag == "(3006,0020)"),
            ) else {
                panic!("sequence not found");
            };
            items[0].insert(
                0,
                element_node("(0008,1140)", "LO", "abc", &["(3006,0020)", "Item#0"]),
            );
            save_file(&path, &nodes)
        };
        let matching = save_with(element_node("(0008,1030)", "LO", "head", &[]));
        let private = save_with(element_node("(0009,0010)", "LO", "VENDOR", &[]));
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let as_sequence = as_sequence.unwrap_err();
        assert!(as_sequence.contains("(0008,1140)"), "{as_sequence}");
        assert!(as_sequence.contains("SQ"), "{as_sequence}");
        assert!(as_text.unwrap_err().contains("readers take it as US"));
        assert!(in_item
            .unwrap_err()
            .contains("(3006,0020) / Item#0 / (0008,1140)"));
        assert!(matching.is_ok());
        assert!(private.is_ok());
        assert_eq!(after, original);
    }

    /// dicom-rs reads US or SS elements from Implicit VR files as SS only after a
    /// non-zero Pixel Representation, so only that VR is accepted. (Elements with
    /// numeric VRs cannot be added yet, so this is checked without saving.)
    #[test]
    fn implicit_vr_us_or_ss_elements_follow_pixel_representation() {
        let check = |pixel_representation: Option<&str>, vr: &str| {
            let mut nodes = Vec::new();
            // Before Pixel Representation in the list, after it in tag order.
            nodes.push(element_node("(0028,0106)", vr, "1", &[]));
            if let Some(representation) = pixel_representation {
                nodes.push(element_node("(0028,0103)", "US", representation, &[]));
            }
            ensure_vrs_match_dictionary(&InMemDicomObject::new_empty(), &nodes)
        };
        // Pixel Representation in an earlier item still applies, as dicom-rs reads it.
        let mut in_items = vec![DicomNode::Sequence {
            tag: "(0008,1115)".to_string(),
            description: String::new(),
            length: 0,
            items: vec![vec![element_node(
                "(0028,0103)",
                "US",
                "1",
                &["(0008,1115)", "Item#0"],
            )]],
            path: vec!["(0008,1115)".to_string()],
            item_sources: None,
        }];
        in_items.push(element_node("(0028,0106)", "SS", "-1", &[]));
        let signed_after_item =
            ensure_vrs_match_dictionary(&InMemDicomObject::new_empty(), &in_items);

        let error = check(None, "SS").unwrap_err();
        assert!(
            error.contains("(0028,0106) has VR SS") && error.contains("as US"),
            "{error}"
        );
        assert!(check(Some("0"), "SS").unwrap_err().contains("as US"));
        assert!(check(Some("1"), "US")
            .unwrap_err()
            .contains("as SS after Pixel Representation 1"));
        assert!(check(None, "US").is_ok());
        assert!(check(Some("0"), "US").is_ok());
        assert!(check(Some("1"), "SS").is_ok());
        assert!(signed_after_item.is_ok());
    }

    /// An unchanged US or SS element no longer matches after Pixel Representation
    /// changes, and a sequence at a tag the dictionary does not define as SQ would be
    /// read as another VR.
    #[test]
    fn implicit_vr_check_covers_unchanged_us_or_ss_elements_and_sequences() {
        let mut obj = InMemDicomObject::new_empty();
        obj.put(DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(1_u16),
        ));
        obj.put(DataElement::new(
            Tag(0x0028, 0x0106),
            VR::SS,
            PrimitiveValue::from(-1_i16),
        ));
        obj.put(DataElement::new(
            Tag(0x0008, 0x1030),
            VR::SQ,
            Value::from(dicom_core::value::DataSetSequence::from(Vec::<
                InMemDicomObject,
            >::new(
            ))),
        ));
        let nodes_with = |representation: &str, sequence_tag: &str| {
            vec![
                element_node("(0028,0103)", "US", representation, &[]),
                element_node("(0028,0106)", "SS", "-1", &[]),
                DicomNode::Sequence {
                    tag: sequence_tag.to_string(),
                    description: String::new(),
                    length: 0,
                    items: vec![vec![]],
                    path: vec![sequence_tag.to_string()],
                    item_sources: None,
                },
            ]
        };

        // As read: the sequence already in the file is left alone.
        assert!(ensure_vrs_match_dictionary(&obj, &nodes_with("1", "(0008,1030)")).is_ok());
        let changed = ensure_vrs_match_dictionary(&obj, &nodes_with("0", "(0008,1030)"));
        assert!(changed.unwrap_err().contains("(0028,0106) has VR SS"));
        let added = ensure_vrs_match_dictionary(&obj, &nodes_with("1", "(0008,103E)")).unwrap_err();
        assert!(
            added.contains("(0008,103E) is a sequence") && added.contains("as LO"),
            "{added}"
        );
        // Unchanged, US after Pixel Representation 1 (as read from a file whose tags
        // are out of order) is left alone.
        let mut out_of_order = obj.clone();
        out_of_order.put(DataElement::new(
            Tag(0x0028, 0x0106),
            VR::US,
            PrimitiveValue::from(1_u16),
        ));
        let mut unchanged = nodes_with("1", "(0008,1030)");
        unchanged[1] = element_node("(0028,0106)", "US", "1", &[]);
        assert!(ensure_vrs_match_dictionary(&out_of_order, &unchanged).is_ok());
        // Removing Pixel Representation also rechecks them.
        let removed = ensure_vrs_match_dictionary(&obj, &nodes_with("1", "(0008,1030)")[1..]);
        assert!(removed.unwrap_err().contains("(0028,0106) has VR SS"));
        // Sequences at SQ tags and private tags can be added.
        assert!(ensure_vrs_match_dictionary(&obj, &nodes_with("1", "(0008,1115)")).is_ok());
        assert!(ensure_vrs_match_dictionary(&obj, &nodes_with("1", "(0009,1001)")).is_ok());
    }

    /// A US or SS element stored before Pixel Representation reads as US, and an
    /// unchanged copy of such a file must still save.
    #[test]
    fn implicit_vr_files_with_us_or_ss_before_pixel_representation_save() {
        let path = temp_path("implicit-xs-order.dcm");
        let dataset = [
            implicit_uid(0x0008, 0x0016, RTSTRUCT_SOP_CLASS),
            implicit_uid(0x0008, 0x0018, "1.2.3.4"),
            implicit_element(0x0010, 0x0010, b"Yamada^Tarou"),
            implicit_element(0x0028, 0x0106, &1_u16.to_le_bytes()),
            implicit_element(0x0028, 0x0103, &1_u16.to_le_bytes()),
        ]
        .concat();
        write_part10_with(&path, uids::IMPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let original = std::fs::read(&path).unwrap();
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let unchanged = save_file(&path, &opened);
        let mut edited = opened.clone();
        *find_element_mut(&mut edited, &["(0010,0010)"]).unwrap() = "Suzuki^Hanako".to_string();
        let edited = save_file(&path, &edited);
        std::fs::remove_file(&path).ok();

        assert_eq!(unchanged.unwrap(), original);
        assert!(contains(&edited.unwrap(), b"Suzuki^Hanako"));
    }

    /// Files that dicom-rs writes as Implicit VR, with private elements and nested
    /// sequences, are indexed the way dicom-rs reads them.
    #[test]
    fn implicit_vr_files_written_by_dicom_rs_are_spliced() {
        let mut inner = InMemDicomObject::new_empty();
        inner.put_str(Tag(0x3006, 0x0084), VR::IS, "1");
        let mut item = InMemDicomObject::new_empty();
        item.put_str(Tag(0x3006, 0x0022), VR::IS, "1");
        item.put_str(Tag(0x3006, 0x0026), VR::LO, "PTV");
        item.put(DataElement::new(
            Tag(0x3006, 0x0039),
            VR::SQ,
            Value::from(dicom_core::value::DataSetSequence::from(vec![inner])),
        ));
        let mut obj = InMemDicomObject::new_empty();
        obj.put_str(Tag(0x0008, 0x0005), VR::CS, "ISO_IR 192");
        obj.put_str(Tag(0x0008, 0x0016), VR::UI, RTSTRUCT_SOP_CLASS);
        obj.put_str(Tag(0x0008, 0x0018), VR::UI, "1.2.3.4");
        obj.put_str(Tag(0x0009, 0x0010), VR::LO, "VENDOR");
        obj.put_str(Tag(0x0009, 0x1001), VR::LO, "private");
        obj.put_str(Tag(0x0010, 0x0010), VR::PN, "Yamada^Tarou");
        obj.put(DataElement::new(
            Tag(0x3006, 0x0020),
            VR::SQ,
            Value::from(dicom_core::value::DataSetSequence::from(vec![item])),
        ));
        let mut dataset = Vec::new();
        obj.write_dataset_with_ts(
            &mut dataset,
            TransferSyntaxRegistry
                .get(uids::IMPLICIT_VR_LITTLE_ENDIAN)
                .unwrap(),
        )
        .unwrap();
        let path = temp_path("implicit-dicom-rs.dcm");
        write_part10_with(&path, uids::IMPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let original = std::fs::read(&path).unwrap();
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let unchanged = save_file(&path, &opened);
        let mut edited = opened.clone();
        *find_element_mut(
            &mut edited,
            &[
                "(3006,0020)",
                "Item#0",
                "(3006,0039)",
                "Item#0",
                "(3006,0084)",
            ],
        )
        .unwrap() = "2".to_string();
        *find_element_mut(&mut edited, &["(0010,0010)"]).unwrap() = "山田^太郎".to_string();
        let edited_plans_splice = plans_splice(&path, &edited);
        let saved = save_file(&path, &edited);
        std::fs::remove_file(&path).ok();

        assert_eq!(unchanged.unwrap(), original);
        assert!(edited_plans_splice);
        let reopened = reopen("implicit-dicom-rs-reopened.dcm", &saved.unwrap());
        assert_eq!(value_and_editable(&reopened, "(3006,0084)").0, "2");
        assert_eq!(value_and_editable(&reopened, "(0010,0010)").0, "山田^太郎");
        assert_eq!(value_and_editable(&reopened, "(0009,1001)").0, "private");
    }

    #[test]
    fn splice_transfer_syntaxes() {
        for uid in [
            uids::IMPLICIT_VR_LITTLE_ENDIAN,
            uids::EXPLICIT_VR_LITTLE_ENDIAN,
            "1.2.840.10008.1.2.4.50",
            "1.2.840.10008.1.2.4.90",
            "1.2.840.10008.1.2.5",
            "1.2.840.10008.1.2.2",
            uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN,
        ] {
            assert!(splice::supports_transfer_syntax(uid), "{uid}");
        }
        for uid in ["1.2.840.10008.1.2.4.95", "1.2.840.10008.1.20"] {
            assert!(!splice::supports_transfer_syntax(uid), "{uid}");
        }
    }

    fn plan_for(path: &std::path::Path, nodes: &[DicomNode]) -> bool {
        let path_text = path.to_str().unwrap();
        plan_splice(
            path_text,
            &open_full_object(path_text).unwrap(),
            &with_file_item_paths(nodes, &[]),
            &TextValues::try_read(path_text).unwrap(),
            splice::file_stamp(path_text).unwrap(),
        )
        .unwrap()
        .is_some()
    }

    /// Top-level elements after Pixel Data's tag, such as (FFFC,FFFC) padding, are not
    /// shown as nodes; splicing keeps them, also in files without Pixel Data.
    #[test]
    fn trailing_elements_are_kept_when_splicing() {
        let path = temp_path("splice-trailing.dcm");
        let padding = explicit_long_element(0xFFFC, 0xFFFC, b"OB", &[0, 0]);
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            padding.clone(),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let shown = find_element(&nodes, "(FFFC,FFFC)").is_some();
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "ID002".to_string();
        let planned = plan_for(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!shown);
        assert!(planned);
        assert!(saved.unwrap().ends_with(&padding));
    }

    /// The full writer repairs file meta UIDs that disagree with the data set.
    #[test]
    fn mismatched_file_meta_is_not_spliced() {
        let path = temp_path("splice-meta.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        ]
        .concat();
        // The data set says 1.2.3.4; make the meta say something else.
        let meta = FileMetaTableBuilder::new()
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
            .media_storage_sop_class_uid(RTSTRUCT_SOP_CLASS)
            .media_storage_sop_instance_uid("9.9.9")
            .build()
            .unwrap();
        let mut file = vec![0_u8; 128];
        file.extend_from_slice(b"DICM");
        meta.write(&mut file).unwrap();
        file.extend(dataset);
        std::fs::write(&path, file).unwrap();
        let nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let planned = plan_for(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!planned);
    }

    /// Elements with VR bytes that are not a known VR are not rewritten by splicing.
    #[test]
    fn unknown_vrs_are_not_spliced() {
        let path = temp_path("splice-unknown-vr.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let index = splice::FileIndex::read(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        let path_vec = vec!["(0010,0020)".to_string()];
        assert_eq!(indexed_element_vr(index.nodes(), &path_vec), Some(VR::LO));
        assert_eq!(
            indexed_element_vr(index.nodes(), &["(0010,0030)".to_string()]),
            None
        );
    }

    /// An element whose VR changed (removed and added again) is written by the full
    /// writer with its new VR.
    #[test]
    fn changed_vrs_are_not_spliced() {
        let path = temp_path("splice-vr-change.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        if let Some(DicomNode::Element { vr, value, .. }) = nodes
            .iter_mut()
            .find(|node| matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0020)"))
        {
            *vr = "SH".to_string();
            *value = "ID002".to_string();
        }
        let planned = plan_for(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!planned);
    }

    /// Saving through the full writer keeps top-level elements after Pixel Data's tag in
    /// files without Pixel Data; they are not shown as nodes, so they were not removed.
    #[test]
    fn full_writer_keeps_trailing_elements_without_pixel_data() {
        let path = temp_path("full-writer-trailing.dcm");
        let padding = explicit_long_element(0xFFFC, 0xFFFC, b"OB", &[0, 0]);
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            padding.clone(),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        // An added element splicing cannot write (not text) needs the full writer.
        let mut node = element_node("(0028,0010)", "US", "512", &[]);
        if let DicomNode::Element { editable, .. } = &mut node {
            *editable = false;
        }
        nodes.push(node);
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "ID002".to_string();
        let planned = plan_for(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!planned);
        let saved = saved.unwrap();
        assert!(contains(&saved, b"ID002"));
        assert!(contains(&saved, &[0xFC, 0xFF, 0xFC, 0xFF]));
    }

    /// Raw data sets without file meta information show trailing elements as nodes, so
    /// removing one of them removes it. (When all of them are removed, they cannot be
    /// told apart from elements that were never shown, and are kept.)
    #[test]
    fn removing_a_shown_trailing_element_removes_it() {
        let path = temp_path("raw-trailing.dcm");
        let dataset = [
            implicit_element(0x0008, 0x0060, b"RTSTRUCT"),
            implicit_element(0x0010, 0x0020, b"ID001"),
            implicit_element(0x7FE1, 0x0010, b"VENDOR"),
            implicit_element(0xFFFC, 0xFFFC, b"\0\0"),
        ]
        .concat();
        std::fs::write(&path, dataset).unwrap();
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        let shown = find_element(&nodes, "(FFFC,FFFC)").is_some();
        remove_element(&mut nodes, "(FFFC,FFFC)");
        let mut obj = open_full_object(path_text).unwrap();
        apply_nodes_to_object_with_text(&mut obj, &nodes, &TextValues::default(), true).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(shown);
        assert!(obj.get(Tag(0xFFFC, 0xFFFC)).is_none());
        assert!(obj.get(Tag(0x7FE1, 0x0010)).is_some());
    }

    /// Review on #10: edits to a signed file are refused; saving it unchanged is not.
    #[test]
    fn signed_files_cannot_be_saved_with_edits() {
        let path = temp_path("signed.dcm");
        let signature = sequence(
            0xFFFA,
            0xFFFA,
            &[explicit_element(0x0400, 0x0015, b"CS", b"SHA256")],
            true,
        );
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0010, b"PN", b"Original"),
            signature,
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let original = std::fs::read(&path).unwrap();
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let unchanged = save_file(&path, &opened);
        let mut edited = opened.clone();
        *find_element_mut(&mut edited, &["(0010,0010)"]).unwrap() = "Edited".to_string();
        let edited_result = save_file(&path, &edited);
        let mut removed = opened.clone();
        remove_element(&mut removed, "(0010,0010)");
        let removed_result = save_file(&path, &removed);
        std::fs::remove_file(&path).ok();

        assert_eq!(unchanged.unwrap(), original);
        assert!(edited_result.unwrap_err().contains("digital signatures"));
        assert!(removed_result.unwrap_err().contains("digital signatures"));
    }

    /// Review on #10: adding an element after Pixel Data's tag does not remove unshown
    /// trailing elements.
    #[test]
    fn adding_a_trailing_element_keeps_unshown_ones() {
        let path = temp_path("full-writer-add-trailing.dcm");
        let padding = explicit_long_element(0xFFFC, 0xFFFC, b"OB", &[0, 0]);
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            padding,
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        nodes.push(DicomNode::Element {
            tag: "(8001,0010)".to_string(),
            vr: "LO".to_string(),
            inferred_vr: None,
            description: String::new(),
            value: "VENDOR".to_string(),
            length: 0,
            path: vec!["(8001,0010)".to_string()],
            editable: true,
        });
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert!(contains(&saved, b"VENDOR"));
        assert!(contains(&saved, &[0xFC, 0xFF, 0xFC, 0xFF]));
    }

    /// Review on #10: a file changed after the stamp was taken is not overwritten, by
    /// splicing or by the full writer.
    #[test]
    fn saving_fails_when_the_file_changed_after_opening() {
        let dir = temp_test_dir("changed-while-saving");
        let path = dir.join("changed.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let path_text = path.to_str().unwrap();
        let opened_stamp = splice::file_stamp(path_text);
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(0010,0020)"]).unwrap() = "LocalID".to_string();
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        // Another program changes the file (here: its length) after it was opened.
        let mut changed = std::fs::read(&path).unwrap();
        changed.extend_from_slice(&explicit_long_element(0xFFFC, 0xFFFC, b"OB", &[0, 0]));
        std::fs::write(&path, &changed).unwrap();

        let spliced = save_file_with_nodes(
            path_text,
            &mut obj,
            &nodes,
            &text,
            path_text,
            opened_stamp,
            false,
        );
        let mut added = nodes.clone();
        added.push(DicomNode::Element {
            tag: "(0008,1030)".to_string(),
            vr: "LO".to_string(),
            inferred_vr: None,
            description: String::new(),
            value: "HEAD".to_string(),
            length: 0,
            path: vec!["(0008,1030)".to_string()],
            editable: true,
        });
        let full = save_file_with_nodes(
            path_text,
            &mut obj,
            &added,
            &text,
            path_text,
            opened_stamp,
            false,
        );
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        assert_eq!(spliced.unwrap_err(), FILE_CHANGED_ON_DISK);
        assert_eq!(full.unwrap_err(), FILE_CHANGED_ON_DISK);
        assert_eq!(after, changed);
    }

    /// Review on #10: changing only the VR of an element (same value) is saved.
    #[test]
    fn changing_only_the_vr_is_saved() {
        let path = temp_path("vr-only.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        if let Some(DicomNode::Element { vr, .. }) = nodes
            .iter_mut()
            .find(|node| matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0020)"))
        {
            *vr = "SH".to_string();
        }
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&[0x10, 0x00, 0x20, 0x00]);
        expected.extend_from_slice(b"SH");
        assert!(contains(&saved, &expected));
    }

    /// Review on #10: an empty sequence added after Pixel Data's tag is not spliced
    /// (Part 10 files are read up to Pixel Data), and the full writer refuses it
    /// rather than dropping it. The editor does not offer such tags for sequences.
    #[test]
    fn added_sequences_after_pixel_data_are_not_saved() {
        let path = temp_path("splice-add-trailing-sequence.dcm");
        write_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        nodes.push(DicomNode::Sequence {
            tag: "(FFFA,FFFA)".to_string(),
            description: String::new(),
            length: 0,
            path: vec!["(FFFA,FFFA)".to_string()],
            items: Vec::new(),
            item_sources: None,
        });
        let planned = plan_for(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!planned);
        assert!(saved.unwrap_err().contains("adding sequences or items"));
    }

    /// Review on #10: after adding an element after Pixel Data's tag and saving, saving
    /// again keeps the file's unshown trailing elements.
    #[test]
    fn saving_twice_after_adding_a_trailing_element_keeps_unshown_ones() {
        let dir = temp_test_dir("add-trailing-twice");
        let path = dir.join("add-trailing-twice.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            explicit_long_element(0xFFFC, 0xFFFC, b"OB", &[0, 0]),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        nodes.push(DicomNode::Element {
            tag: "(8001,0010)".to_string(),
            vr: "LO".to_string(),
            inferred_vr: None,
            description: String::new(),
            value: "VENDOR".to_string(),
            length: 0,
            path: vec!["(8001,0010)".to_string()],
            editable: true,
        });
        let mut obj = open_full_object(path_text).unwrap();
        let first_text = TextValues::try_read(path_text).unwrap();
        let first_stamp = splice::file_stamp(path_text);
        let first = save_file_with_nodes(
            path_text,
            &mut obj,
            &nodes,
            &first_text,
            path_text,
            first_stamp,
            false,
        );
        let second_text = TextValues::try_read(path_text).unwrap();
        let second_stamp = splice::file_stamp(path_text);
        let second = save_file_with_nodes(
            path_text,
            &mut obj,
            &nodes,
            &second_text,
            path_text,
            second_stamp,
            false,
        );
        let saved = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();

        first.unwrap();
        second.unwrap();
        assert!(contains(&saved, b"VENDOR"));
        assert!(contains(&saved, &[0xFC, 0xFF, 0xFC, 0xFF]));
    }

    /// #8: values dicom-rs cannot write back are editable and saved by splicing, here
    /// half-width katakana in an ISO_IR 13 item under a UTF-8 data set.
    #[test]
    fn splice_saves_edits_to_values_dicom_rs_cannot_write() {
        let path = temp_path("splice-edit-read-only.dcm");
        write_mixed_charset_file(&path);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        *find_element_mut(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"]).unwrap() =
            "ｶﾝｿﾞｳ".to_string();
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        // JIS X 0201 katakana bytes, written as they are.
        assert!(contains(&saved, b"\xb6\xdd\xbf\xde\xb3 "));
        let reopened_path = temp_path("splice-edit-read-only-reopened.dcm");
        std::fs::write(&reopened_path, &saved).unwrap();
        let reopened = open_nodes(reopened_path.to_str().unwrap()).unwrap().nodes;
        std::fs::remove_file(&reopened_path).ok();
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ｶﾝｿﾞｳ".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(0010,0010)"),
            ("山田^太郎".to_string(), true)
        );
    }

    /// Raw data sets without file meta information cannot be spliced, so values dicom-rs
    /// cannot write back stay read-only there.
    #[test]
    fn values_dicom_rs_cannot_write_stay_read_only_without_splicing() {
        let path = temp_path("raw-read-only.dcm");
        let mut item = implicit_element(0x0008, 0x0005, b"ISO_IR 13");
        item.extend(implicit_element(0x3006, 0x0026, HAN_IR13));
        let dataset = [
            implicit_element(0x0008, 0x0005, b"ISO_IR 192"),
            implicit_element(0x0008, 0x0060, b"RTSTRUCT"),
            sequence(0x3006, 0x0020, &[item], false),
        ]
        .concat();
        std::fs::write(&path, dataset).unwrap();
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), false)
        );
        assert!(has_warning(&content, TEXT_READ_ONLY_WARNING));
        assert!(has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
    }

    fn element_node(tag: &str, vr: &str, value: &str, parent: &[&str]) -> DicomNode {
        let mut path = parent
            .iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>();
        path.push(tag.to_string());
        DicomNode::Element {
            tag: tag.to_string(),
            vr: vr.to_string(),
            inferred_vr: None,
            description: String::new(),
            value: value.to_string(),
            length: 0,
            path,
            editable: true,
        }
    }

    fn item_nodes_mut<'a>(
        nodes: &'a mut [DicomNode],
        sequence: &str,
        index: usize,
    ) -> &'a mut Vec<DicomNode> {
        nodes
            .iter_mut()
            .find_map(|node| match node {
                DicomNode::Sequence { tag, items, .. } if tag == sequence => items.get_mut(index),
                _ => None,
            })
            .unwrap()
    }

    /// #8: adding and removing plain elements is spliced, also inside defined-length
    /// items, and keeps the bytes of everything else.
    #[test]
    fn splice_adds_and_removes_elements() {
        let path = temp_path("splice-add-remove.dcm");
        let original = write_mixed_charset_file(&path);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        // Top level: add a study description (UTF-8), remove the patient ID.
        nodes.push(element_node("(0008,1030)", "LO", "頭部CT", &[]));
        remove_element(&mut nodes, "(0010,0020)");
        // In the ISO_IR 13 item: add an ROI description in half-width katakana.
        item_nodes_mut(&mut nodes, "(3006,0020)", 0).push(element_node(
            "(3006,0028)",
            "ST",
            "ｶﾝｿﾞｳ",
            &["(3006,0020)", "Item#0"],
        ));
        let planned = plan_for(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned, "saving should splice");
        let saved = saved.unwrap();
        let reopened_path = temp_path("splice-add-remove-reopened.dcm");
        std::fs::write(&reopened_path, &saved).unwrap();
        let reopened = open_nodes(reopened_path.to_str().unwrap()).unwrap().nodes;
        std::fs::remove_file(&reopened_path).ok();

        assert_eq!(
            value_and_editable(&reopened, "(0008,1030)"),
            ("頭部CT".to_string(), true)
        );
        assert!(find_element(&reopened, "(0010,0020)").is_none());
        assert_eq!(
            value_and_editable(&reopened, "(3006,0028)"),
            ("ｶﾝｿﾞｳ".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&reopened, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
        assert!(contains(&saved, b"\xb6\xdd\xbf\xde\xb3 "));
        // The pixel data and the undefined-length sequence are kept byte for byte.
        let tail_start = original
            .windows(4)
            .position(|window| window == [0x06, 0x30, 0x39, 0x00])
            .unwrap();
        assert!(saved.ends_with(&original[tail_start..]));
    }

    // ---- Adding sequences and items (#8) ----

    /// Append an item made of `nodes` to the sequence at `sequence_path`, as the editor
    /// does (`item_sources` records it as new).
    fn append_item(
        nodes: &mut [DicomNode],
        sequence_path: &[&str],
        make: &dyn Fn(&[&str]) -> Vec<DicomNode>,
    ) {
        for node in nodes.iter_mut() {
            if let DicomNode::Sequence {
                path,
                items,
                item_sources,
                ..
            } = node
            {
                if path == sequence_path {
                    let sources =
                        item_sources.get_or_insert_with(|| (0..items.len()).map(Some).collect());
                    let item_path = format!("Item#{}", items.len());
                    let mut parent = sequence_path.to_vec();
                    parent.push(&item_path);
                    items.push(make(&parent));
                    sources.push(None);
                    return;
                }
                for item in items {
                    append_item(item, sequence_path, make);
                }
            }
        }
    }

    fn new_sequence(tag: &str, parent: &[&str], items: Vec<Vec<DicomNode>>) -> DicomNode {
        let mut path = parent
            .iter()
            .map(|segment| segment.to_string())
            .collect::<Vec<_>>();
        path.push(tag.to_string());
        DicomNode::Sequence {
            tag: tag.to_string(),
            description: String::new(),
            length: 0,
            item_sources: Some(vec![None; items.len()]),
            items,
            path,
        }
    }

    #[test]
    fn added_sequences_are_spliced() {
        let path = temp_path("add-sequence.dcm");
        write_iso2022_rtstruct(&path, "\\ISO 2022 IR 87");
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let item = |parent: &[&str]| {
            vec![
                element_node("(3006,0082)", "IS", "1", parent),
                element_node("(3006,0085)", "SH", "前立腺", parent),
            ]
        };
        nodes.push(new_sequence(
            "(3006,0080)",
            &[],
            vec![item(&["(3006,0080)", "Item#0"])],
        ));
        let spliced = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(spliced);
        let saved = saved.unwrap();
        assert!(contains(&saved, PROSTATE_IR87));
        let reopened = reopen("add-sequence-reopened.dcm", &saved);
        let items = sequence_items_of(&reopened, "(3006,0080)");
        assert_eq!(items.len(), 1);
        assert_eq!(values_in(&items[0], "(3006,0085)"), "前立腺");
        assert_eq!(values_in(&items[0], "(3006,0082)"), "1");
        assert_eq!(
            values_in(&reopened, "(0010,0010)"),
            "Yamada^Tarou=山田^太郎=やまだ^たろう"
        );
    }

    #[test]
    fn added_items_are_spliced_after_the_others() {
        for defined_length in [true, false] {
            let path = temp_path(&format!("add-item-{defined_length}.dcm"));
            write_three_rois(&path, defined_length);
            let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
            append_item(&mut nodes, &["(3006,0020)"], &|parent| {
                vec![
                    element_node("(3006,0022)", "IS", "4", parent),
                    element_node("(3006,0026)", "LO", "精嚢", parent),
                ]
            });
            // An item from the file can be removed in the same save.
            remove_item(&mut nodes, &["(3006,0020)"], 0);
            let spliced = plans_splice(&path, &nodes);
            let saved = save_file(&path, &nodes);
            std::fs::remove_file(&path).ok();

            assert!(spliced, "defined length: {defined_length}");
            let reopened = reopen("add-item-reopened.dcm", &saved.unwrap());
            let items = sequence_items_of(&reopened, "(3006,0020)");
            let names = items
                .iter()
                .map(|item| values_in(item, "(3006,0026)"))
                .collect::<Vec<_>>();
            assert_eq!(names, ["直腸", "膀胱", "精嚢"]);
            assert_eq!(values_in(&items[2], "(3006,0022)"), "4");
            assert_eq!(values_in(&reopened, "(300E,0002)"), "APPROVED");
        }
    }

    /// An added item declaring its own character set is written in it.
    #[test]
    fn added_items_use_their_own_character_set() {
        let path = temp_path("add-item-charset.dcm");
        write_three_rois(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        append_item(&mut nodes, &["(3006,0020)"], &|parent| {
            vec![
                element_node("(0008,0005)", "CS", "ISO_IR 192", parent),
                element_node("(3006,0026)", "LO", "精嚢", parent),
            ]
        });
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert!(contains(&saved, "精嚢".as_bytes()));
        let reopened = reopen("add-item-charset-reopened.dcm", &saved);
        let items = sequence_items_of(&reopened, "(3006,0020)");
        assert_eq!(values_in(&items[3], "(3006,0026)"), "精嚢");
        assert_eq!(values_in(&items[0], "(3006,0026)"), "前立腺");
    }

    #[test]
    fn implicit_vr_items_are_added() {
        let path = temp_path("add-item-implicit.dcm");
        write_implicit_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        append_item(&mut nodes, &["(3006,0039)"], &|parent| {
            vec![element_node("(3006,0084)", "IS", "2", parent)]
        });
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let reopened = reopen("add-item-implicit-reopened.dcm", &saved.unwrap());
        let items = sequence_items_of(&reopened, "(3006,0039)");
        assert_eq!(items.len(), 2);
        assert_eq!(values_in(&items[1], "(3006,0084)"), "2");
        assert_eq!(values_in(&reopened, "(3006,0026)"), "ﾊﾝ");
    }

    #[test]
    fn group_lengths_follow_added_items() {
        let path = temp_path("add-item-group-length.dcm");
        write_group_length_file(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        append_item(&mut nodes, &["(3006,0020)"], &|parent| {
            vec![element_node("(3006,0022)", "IS", "2", parent)]
        });
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert_eq!(wrong_group_lengths(&saved), Vec::<String>::new());
        let reopened = reopen("add-item-group-length-reopened.dcm", &saved);
        assert_eq!(sequence_items_of(&reopened, "(3006,0020)").len(), 2);
    }

    /// Values that cannot be written in added items are named, as items are shown.
    #[test]
    fn values_that_cannot_be_written_in_added_items_are_reported() {
        let path = temp_path("add-item-bad-value.dcm");
        write_three_rois(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        append_item(&mut nodes, &["(3006,0020)"], &|parent| {
            vec![element_node("(3006,0022)", "IS", "１", parent)]
        });
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(
            error.contains("(3006,0020) / Item#3 (new) / (3006,0022) (a IS value must be ASCII"),
            "{error}"
        );
    }

    /// Added items holding elements splicing cannot write (not text) are not spliced,
    /// and the full writer refuses them rather than dropping them.
    #[test]
    fn added_items_with_non_text_elements_are_refused() {
        let path = temp_path("add-item-non-text.dcm");
        let original = write_three_rois(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        append_item(&mut nodes, &["(3006,0020)"], &|parent| {
            vec![element_node("(3006,0024)", "US", "1", parent)]
        });
        let spliced = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
        assert!(saved.unwrap_err().contains("adding sequences or items"));
        assert_eq!(after, original);
    }

    // ---- Group lengths (#8) ----

    /// A group length element (gggg,0000) for `elements` of that group, followed by
    /// them.
    fn with_group_length(group: u16, elements: &[u8], explicit: bool) -> Vec<u8> {
        let length = (elements.len() as u32).to_le_bytes();
        let mut out = if explicit {
            explicit_element(group, 0x0000, b"UL", &length)
        } else {
            implicit_element(group, 0x0000, &length)
        };
        out.extend_from_slice(elements);
        out
    }

    /// Group length elements in a saved file whose value is not the length of the
    /// elements of their group that follow, in the same data set or item.
    fn wrong_group_lengths(saved: &[u8]) -> Vec<String> {
        fn check(
            nodes: &[splice::IndexNode],
            saved: &[u8],
            big_endian: bool,
            parent: &str,
            wrong: &mut Vec<String>,
        ) {
            let bounds = |node: &splice::IndexNode| match node {
                splice::IndexNode::Element { start, end, .. }
                | splice::IndexNode::Sequence { start, end, .. }
                | splice::IndexNode::Opaque { start, end, .. } => (*start as usize, *end as usize),
            };
            for node in nodes {
                let tag = node.tag();
                if tag.element() == 0x0000 {
                    let (_, end) = bounds(node);
                    let bytes = saved[end - 4..end].try_into().unwrap();
                    let value = if big_endian {
                        u32::from_be_bytes(bytes)
                    } else {
                        u32::from_le_bytes(bytes)
                    };
                    let actual: usize = nodes
                        .iter()
                        .filter(|other| {
                            other.tag().group() == tag.group() && other.tag().element() != 0
                        })
                        .map(|other| {
                            let (start, end) = bounds(other);
                            end - start
                        })
                        .sum();
                    if value as usize != actual {
                        wrong.push(format!(
                            "{parent}{} = {value}, not {actual}",
                            format_tag(tag)
                        ));
                    }
                }
                if let splice::IndexNode::Sequence { items, .. } = node {
                    for (index, item) in items.iter().enumerate() {
                        let path = format!("{parent}{} / Item#{index} / ", format_tag(tag));
                        check(item.nodes(), saved, big_endian, &path, wrong);
                    }
                }
            }
        }
        // Tests run in parallel: a file of its own for each check.
        static CHECKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let check_number = CHECKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = temp_path(&format!("group-length-check-{check_number}.dcm"));
        std::fs::write(&path, saved).unwrap();
        let index = splice::FileIndex::read(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        let mut wrong = Vec::new();
        check(
            index.nodes(),
            saved,
            index.encoding().big_endian,
            "",
            &mut wrong,
        );
        wrong
    }

    /// A file with group lengths for every group, in the data set and in an item.
    fn write_group_length_file(path: &std::path::Path, explicit: bool) -> Vec<u8> {
        let element = |group: u16, number: u16, vr: &[u8; 2], value: &[u8]| {
            if explicit {
                explicit_element(group, number, vr, value)
            } else {
                implicit_element(group, number, value)
            }
        };
        let item = [
            with_group_length(
                0x0008,
                &element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
                explicit,
            ),
            with_group_length(
                0x3006,
                &[
                    element(0x3006, 0x0022, b"IS", b"1"),
                    element(0x3006, 0x0026, b"LO", HAN_IR13),
                ]
                .concat(),
                explicit,
            ),
        ]
        .concat();
        let rois = if explicit {
            defined_sequence(0x3006, 0x0020, &[item])
        } else {
            implicit_defined_sequence(0x3006, 0x0020, &[item])
        };
        let dataset = [
            with_group_length(
                0x0008,
                &[
                    element(0x0008, 0x0005, b"CS", b"ISO_IR 192"),
                    element(
                        0x0008,
                        0x0016,
                        b"UI",
                        format!("{RTSTRUCT_SOP_CLASS}\0").as_bytes(),
                    ),
                    element(0x0008, 0x0018, b"UI", b"1.2.3.4\0"),
                    element(0x0008, 0x0060, b"CS", b"RTSTRUCT"),
                ]
                .concat(),
                explicit,
            ),
            with_group_length(
                0x0010,
                &[
                    element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
                    element(0x0010, 0x0020, b"LO", b"ID001"),
                ]
                .concat(),
                explicit,
            ),
            with_group_length(0x3006, &rois, explicit),
        ]
        .concat();
        let syntax = if explicit {
            uids::EXPLICIT_VR_LITTLE_ENDIAN
        } else {
            uids::IMPLICIT_VR_LITTLE_ENDIAN
        };
        write_part10_with(path, syntax, &dataset);
        std::fs::read(path).unwrap()
    }

    #[test]
    fn group_lengths_are_updated_by_splicing() {
        for explicit in [true, false] {
            let path = temp_path(&format!("group-length-splice-{explicit}.dcm"));
            let original = write_group_length_file(&path, explicit);
            assert!(wrong_group_lengths(&original).is_empty());
            let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
            set_value(&mut nodes, &["(0010,0010)"], "鈴木^花子子");
            nodes.retain(
                |node| !matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0020)"),
            );
            let position = nodes
                .iter()
                .position(
                    |node| matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0000)"),
                )
                .unwrap();
            nodes.insert(position, element_node("(0008,1030)", "LO", "頭部", &[]));
            set_value(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"], "ﾊﾝﾀﾞ");
            let spliced = plans_splice(&path, &nodes);
            let saved = save_file(&path, &nodes);
            std::fs::remove_file(&path).ok();

            assert!(spliced, "explicit: {explicit}");
            let saved = saved.unwrap();
            assert_eq!(
                wrong_group_lengths(&saved),
                Vec::<String>::new(),
                "explicit: {explicit}"
            );
            let reopened = reopen("group-length-splice-reopened.dcm", &saved);
            assert_eq!(values_in(&reopened, "(0010,0010)"), "鈴木^花子子");
            assert_eq!(values_in(&reopened, "(0008,1030)"), "頭部");
            assert!(find_element(&reopened, "(0010,0020)").is_none());
            assert_eq!(values_in(&reopened, "(3006,0026)"), "ﾊﾝﾀﾞ");
        }
    }

    /// Groups that do not change keep their group length as it is, even when wrong.
    #[test]
    fn untouched_group_lengths_are_kept() {
        let path = temp_path("group-length-untouched.dcm");
        let dataset = [
            explicit_element(0x0008, 0x0000, b"UL", &[0, 0, 0, 0]),
            base_elements(b"ISO_IR 192"),
            with_group_length(
                0x0010,
                &explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
                true,
            ),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0020)"], "ID-00002");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let wrong = wrong_group_lengths(&saved.unwrap());
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].starts_with("(0008,0000) = 0"), "{wrong:?}");
    }

    /// Group lengths that are not 4-byte UL values (here a zero-length OB, whose
    /// header would otherwise be mistaken for a UL value) are left to the full writer.
    #[test]
    fn irregular_group_lengths_are_not_spliced() {
        let path = temp_path("group-length-irregular.dcm");
        let mut ob = Vec::new();
        ob.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]);
        ob.extend_from_slice(b"OB\0\0");
        ob.extend_from_slice(&0_u32.to_le_bytes());
        let dataset = [
            base_elements(b"ISO_IR 192"),
            ob,
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0020)"], "ID-00002");
        let spliced = plans_splice(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
    }

    /// A group runs until an element of another group: elements of the group found
    /// later (tags out of order) do not count.
    #[test]
    fn group_lengths_stop_at_the_next_group() {
        let path = temp_path("group-length-gap.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            with_group_length(
                0x0010,
                &explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
                true,
            ),
            explicit_element(0x0018, 0x0015, b"CS", b"HEAD"),
            explicit_element(0x0010, 0x0030, b"DA", b"19700101"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0020)"], "ID-00002");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        let expected = explicit_element(0x0010, 0x0020, b"LO", b"ID-00002").len() as u32;
        let group_length = [0x10, 0x00, 0x00, 0x00, b'U', b'L', 4, 0];
        let at = saved
            .windows(8)
            .position(|window| window == group_length)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(saved[at + 8..at + 12].try_into().unwrap()),
            expected
        );
    }

    /// Files with irregular group lengths are not treated as spliceable when opened,
    /// so text the full writer cannot write back stays read-only.
    #[test]
    fn irregular_group_lengths_keep_unwritable_text_read_only() {
        let path = temp_path("group-length-irregular-open.dcm");
        let mut ob = Vec::new();
        ob.extend_from_slice(&[0x08, 0x00, 0x00, 0x00]);
        ob.extend_from_slice(b"OB\0\0");
        ob.extend_from_slice(&0_u32.to_le_bytes());
        let item = [
            explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
            explicit_element(0x3006, 0x0026, b"LO", HAN_IR13),
        ]
        .concat();
        let dataset = [
            ob,
            base_elements(b"ISO_IR 192"),
            defined_sequence(0x3006, 0x0020, &[item]),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), false)
        );
        assert!(has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
    }

    /// In a file with tags out of order, a group length only counts the elements of
    /// its group that follow it.
    #[test]
    fn group_lengths_count_the_elements_after_them() {
        let path = temp_path("group-length-unordered.dcm");
        let before = explicit_element(0x0010, 0x0030, b"DA", b"19700101");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            before.clone(),
            with_group_length(
                0x0010,
                &explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
                true,
            ),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0020)"], "ID-00002");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        let expected = explicit_element(0x0010, 0x0020, b"LO", b"ID-00002").len() as u32;
        let group_length = [0x10, 0x00, 0x00, 0x00, b'U', b'L', 4, 0];
        let at = saved
            .windows(8)
            .position(|window| window == group_length)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(saved[at + 8..at + 12].try_into().unwrap()),
            expected
        );
    }

    // ---- Changing SOP UIDs by splicing (#8) ----

    /// The saved file's file meta information, and whether its group length matches
    /// the elements that follow it.
    fn saved_meta(name: &str, saved: &[u8]) -> (FileMetaTable, bool) {
        let path = temp_path(name);
        std::fs::write(&path, saved).unwrap();
        let meta = open_full_object(path.to_str().unwrap())
            .unwrap()
            .meta()
            .clone();
        std::fs::remove_file(&path).ok();
        // Group length: from after (0002,0000) to the first element of another group.
        let group_length = u32::from_le_bytes(saved[140..144].try_into().unwrap()) as usize;
        let mut offset = 144;
        while u16::from_le_bytes(saved[offset..offset + 2].try_into().unwrap()) == 0x0002 {
            let vr = &saved[offset + 4..offset + 6];
            let (header, length) = if matches!(vr, b"OB" | b"OW" | b"UN" | b"UT" | b"SQ") {
                (
                    12,
                    u32::from_le_bytes(saved[offset + 8..offset + 12].try_into().unwrap()) as usize,
                )
            } else {
                (
                    8,
                    u16::from_le_bytes(saved[offset + 6..offset + 8].try_into().unwrap()) as usize,
                )
            };
            offset += header + length;
        }
        (meta, offset - 144 == group_length)
    }

    #[test]
    fn sop_uid_changes_are_spliced_into_the_file_meta() {
        let instance = "1.2.392.200036.9116.2.5.1.48.1221418410.1702345678.123456";
        let class = "1.2.840.10008.5.1.4.1.1.481.5";
        let path = temp_path("sop-uid-splice.dcm");
        let original = write_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0018)"], instance);
        set_value(&mut nodes, &["(0008,0016)"], class);
        let spliced = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        // The full writer refuses this file, so this only saves by splicing.
        assert!(spliced);
        let saved = saved.unwrap();
        let (meta, group_length_matches) = saved_meta("sop-uid-splice-meta.dcm", &saved);
        assert_eq!(
            meta.media_storage_sop_instance_uid.trim_end_matches('\0'),
            instance
        );
        assert_eq!(
            meta.media_storage_sop_class_uid.trim_end_matches('\0'),
            class
        );
        assert!(group_length_matches);
        assert_eq!(saved[..132], original[..132]);
        let reopened = reopen("sop-uid-splice-reopened.dcm", &saved);
        assert_eq!(values_in(&reopened, "(0008,0018)"), instance);
        assert_eq!(values_in(&reopened, "(3006,0026)"), "ﾊﾝ");
    }

    #[test]
    fn implicit_vr_sop_uid_changes_are_spliced() {
        let path = temp_path("sop-uid-implicit.dcm");
        write_implicit_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0008,0018)"], "1.2.3.4.5");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        let (meta, group_length_matches) = saved_meta("sop-uid-implicit-meta.dcm", &saved);
        assert_eq!(
            meta.media_storage_sop_instance_uid.trim_end_matches('\0'),
            "1.2.3.4.5"
        );
        assert!(group_length_matches);
    }

    /// Removing a SOP UID leaves the file meta information to the full writer.
    #[test]
    fn removed_sop_uids_are_not_spliced() {
        let path = temp_path("sop-uid-removed.dcm");
        write_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        nodes
            .retain(|node| !matches!(node, DicomNode::Element { tag, .. } if tag == "(0008,0018)"));
        let spliced = plans_splice(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
    }

    // ---- Removing sequences and items (#8) ----

    /// Remove the item at `index` from the sequence at `sequence_path`, as the editor
    /// does: later items move up (their paths follow their new positions) and
    /// `item_sources` keeps which of the file's items each one shows.
    fn remove_item(nodes: &mut [DicomNode], sequence_path: &[&str], index: usize) {
        fn renumber(nodes: &mut [DicomNode], depth: usize, item: &str) {
            for node in nodes {
                let (DicomNode::Element { path, .. } | DicomNode::Sequence { path, .. }) = node;
                path[depth] = item.to_string();
                if let DicomNode::Sequence { items, .. } = node {
                    for nested in items {
                        renumber(nested, depth, item);
                    }
                }
            }
        }
        for node in nodes.iter_mut() {
            if let DicomNode::Sequence {
                path,
                items,
                item_sources,
                ..
            } = node
            {
                if path == sequence_path {
                    let sources =
                        item_sources.get_or_insert_with(|| (0..items.len()).map(Some).collect());
                    items.remove(index);
                    sources.remove(index);
                    let depth = path.len();
                    for (position, item) in items.iter_mut().enumerate().skip(index) {
                        renumber(item, depth, &format!("Item#{position}"));
                    }
                    return;
                }
                for item in items {
                    remove_item(item, sequence_path, index);
                }
            }
        }
    }

    fn roi_item(number: &[u8], name: &[u8]) -> Vec<u8> {
        [
            explicit_element(0x3006, 0x0022, b"IS", number),
            explicit_element(0x3006, 0x0026, b"LO", name),
        ]
        .concat()
    }

    /// A file with three ROIs in a sequence of the given kind, followed by another
    /// element.
    fn write_three_rois(path: &std::path::Path, defined_length: bool) -> Vec<u8> {
        let items = [
            roi_item(b"1", PROSTATE_IR87),
            roi_item(b"2", b"\x1b$BD>D2\x1b(B"),
            roi_item(b"3", b"\x1b$Bg/fy\x1b(B"),
        ];
        let rois = if defined_length {
            defined_sequence(0x3006, 0x0020, &items)
        } else {
            sequence(0x3006, 0x0020, &items, true)
        };
        let dataset = [
            base_elements(b"\\ISO 2022 IR 87"),
            explicit_element(0x0010, 0x0010, b"PN", YAMADA_IR87),
            rois,
            explicit_element(0x300E, 0x0002, b"CS", b"APPROVED"),
        ]
        .concat();
        write_part10_with(path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        std::fs::read(path).unwrap()
    }

    fn sequence_items_of<'a>(nodes: &'a [DicomNode], tag: &str) -> &'a [Vec<DicomNode>] {
        nodes
            .iter()
            .find_map(|node| match node {
                DicomNode::Sequence {
                    tag: found, items, ..
                } if found == tag => Some(items.as_slice()),
                _ => None,
            })
            .unwrap()
    }

    fn values_in(item: &[DicomNode], tag: &str) -> String {
        value_and_editable(item, tag).0
    }

    #[test]
    fn removed_items_are_spliced_out() {
        for defined_length in [true, false] {
            let path = temp_path(&format!("remove-item-{defined_length}.dcm"));
            let original = write_three_rois(&path, defined_length);
            let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
            remove_item(&mut nodes, &["(3006,0020)"], 1);
            // The third ROI is now shown second; editing it must edit the file's third.
            set_value(
                &mut nodes,
                &["(3006,0020)", "Item#1", "(3006,0026)"],
                "膀胱 壁",
            );
            let spliced = plans_splice(&path, &nodes);
            let saved = save_file(&path, &nodes);
            std::fs::remove_file(&path).ok();

            assert!(spliced, "defined length: {defined_length}");
            let saved = saved.unwrap();
            assert!(saved.len() < original.len());
            assert!(
                !contains(&saved, b"\x1b$BD>D2\x1b(B"),
                "the second ROI is gone"
            );
            let reopened = reopen("remove-item-reopened.dcm", &saved);
            let items = sequence_items_of(&reopened, "(3006,0020)");
            assert_eq!(items.len(), 2);
            assert_eq!(values_in(&items[0], "(3006,0022)"), "1");
            assert_eq!(values_in(&items[0], "(3006,0026)"), "前立腺");
            assert_eq!(values_in(&items[1], "(3006,0022)"), "3");
            assert_eq!(values_in(&items[1], "(3006,0026)"), "膀胱 壁");
            assert_eq!(values_in(&reopened, "(300E,0002)"), "APPROVED");
        }
    }

    #[test]
    fn removed_sequences_are_spliced_out() {
        let path = temp_path("remove-sequence.dcm");
        let original = write_mixed_charset_file(&path);
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let mut removed = opened.clone();
        removed.retain(
            |node| !matches!(node, DicomNode::Sequence { tag, .. } if tag == "(3006,0039)"),
        );
        // Removing every item leaves an empty sequence.
        let mut emptied = opened.clone();
        remove_item(&mut emptied, &["(3006,0039)"], 0);
        let removed_planned = plan_for(&path, &removed);
        let removed_saved = save_file(&path, &removed);
        let emptied_saved = save_file(&path, &emptied);
        std::fs::remove_file(&path).ok();

        assert!(removed_planned);
        let removed_saved = removed_saved.unwrap();
        let start = original
            .windows(4)
            .position(|window| window == [0x06, 0x30, 0x39, 0x00])
            .unwrap();
        let pixel = original
            .windows(4)
            .position(|window| window == [0xE0, 0x7F, 0x10, 0x00])
            .unwrap();
        assert_eq!(&removed_saved[..start], &original[..start]);
        assert_eq!(&removed_saved[start..], &original[pixel..]);
        let reopened = reopen("remove-sequence-reopened.dcm", &removed_saved);
        assert!(!reopened
            .iter()
            .any(|node| matches!(node, DicomNode::Sequence { tag, .. } if tag == "(3006,0039)")));
        assert_eq!(values_in(&reopened, "(3006,0026)"), "ﾊﾝ");
        let reopened = reopen("remove-sequence-emptied.dcm", &emptied_saved.unwrap());
        assert!(sequence_items_of(&reopened, "(3006,0039)").is_empty());
    }

    #[test]
    fn removed_items_are_not_converted() {
        let path = temp_path("remove-item-convert.dcm");
        write_three_rois(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        remove_item(&mut nodes, &["(3006,0020)"], 0);
        set_value(&mut nodes, &["(0008,0005)"], "ISO_IR 192");
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let saved = saved.unwrap();
        assert!(!saved.contains(&0x1B));
        let reopened = reopen("remove-item-convert-reopened.dcm", &saved);
        let items = sequence_items_of(&reopened, "(3006,0020)");
        assert_eq!(items.len(), 2);
        assert_eq!(values_in(&items[0], "(3006,0026)"), "直腸");
        assert_eq!(values_in(&items[1], "(3006,0026)"), "膀胱");
    }

    /// The full writer pairs items by position, so removing items needs splicing.
    #[test]
    fn the_full_writer_refuses_removed_items() {
        let path = temp_path("remove-item-full.dcm");
        write_three_rois(&path, true);
        let path_text = path.to_str().unwrap();
        let mut nodes = open_nodes(path_text).unwrap().nodes;
        remove_item(&mut nodes, &["(3006,0020)"], 1);
        let text = TextValues::try_read(path_text).unwrap();
        let mut obj = open_full_object(path_text).unwrap();
        let destination = temp_path("remove-item-full-out.dcm");
        let result = save_object_with_nodes(&mut obj, &nodes, &text, destination.to_str().unwrap());
        std::fs::remove_file(&path).ok();

        assert!(result.unwrap_err().contains("removing items"));
        assert!(!destination.exists());
    }

    /// Items missing from a sequence without `item_sources` are not taken as removed.
    #[test]
    fn missing_items_without_sources_are_not_removed() {
        let path = temp_path("missing-items.dcm");
        write_three_rois(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        if let Some(DicomNode::Sequence { items, .. }) = nodes
            .iter_mut()
            .find(|node| matches!(node, DicomNode::Sequence { tag, .. } if tag == "(3006,0020)"))
        {
            items.truncate(1);
        }
        let spliced = plans_splice(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
    }

    /// Items can be removed but not reordered: splicing writes them in the file's order.
    #[test]
    fn reordered_items_are_not_spliced() {
        let path = temp_path("reorder-items.dcm");
        write_three_rois(&path, true);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        remove_item(&mut nodes, &["(3006,0020)"], 1);
        if let Some(DicomNode::Sequence {
            items,
            item_sources,
            ..
        }) = nodes
            .iter_mut()
            .find(|node| matches!(node, DicomNode::Sequence { tag, .. } if tag == "(3006,0020)"))
        {
            items.swap(0, 1);
            item_sources.as_mut().unwrap().swap(0, 1);
            for (position, item) in items.iter_mut().enumerate() {
                for node in item.iter_mut() {
                    if let DicomNode::Element { path, .. } = node {
                        path[1] = format!("Item#{position}");
                    }
                }
            }
        }
        let spliced = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!spliced);
        assert!(saved.unwrap_err().contains("removing items"));
    }

    /// Removing the last items leaves `item_sources` in order (or empty); the full
    /// writer still notices them by count.
    #[test]
    fn the_full_writer_refuses_removed_last_items() {
        let path = temp_path("remove-last-item-full.dcm");
        write_three_rois(&path, true);
        let path_text = path.to_str().unwrap();
        let opened = open_nodes(path_text).unwrap().nodes;
        let mut last = opened.clone();
        remove_item(&mut last, &["(3006,0020)"], 2);
        let mut all = opened.clone();
        for _ in 0..3 {
            remove_item(&mut all, &["(3006,0020)"], 0);
        }
        let text = TextValues::try_read(path_text).unwrap();
        let destination = temp_path("remove-last-item-full-out.dcm");
        let results = [last, all].map(|nodes| {
            let mut obj = open_full_object(path_text).unwrap();
            save_object_with_nodes(&mut obj, &nodes, &text, destination.to_str().unwrap())
        });
        std::fs::remove_file(&path).ok();

        for result in results {
            assert!(result.unwrap_err().contains("removing items"));
        }
        assert!(!destination.exists());
    }

    #[test]
    fn implicit_vr_items_are_removed() {
        let path = temp_path("remove-item-implicit.dcm");
        write_implicit_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        remove_item(&mut nodes, &["(3006,0020)"], 0);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        let reopened = reopen("remove-item-implicit-reopened.dcm", &saved.unwrap());
        assert!(sequence_items_of(&reopened, "(3006,0020)").is_empty());
        assert_eq!(values_in(&reopened, "(3006,0084)"), "1");
    }

    /// Review: added nodes that are not edited text values are left to the full writer
    /// instead of being dropped by splicing.
    #[test]
    fn added_nodes_splicing_cannot_write_use_the_full_writer() {
        let path = temp_path("splice-added-non-text.dcm");
        write_mixed_charset_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let mut node = element_node("(0028,0010)", "US", "512", &[]);
        if let DicomNode::Element { editable, .. } = &mut node {
            *editable = false;
        }
        nodes.push(node);
        let planned = plan_for(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(!planned);
    }

    /// Files with group lengths are spliced (#8), so values the full writer cannot
    /// write back stay editable.
    #[test]
    fn files_with_group_lengths_are_spliceable() {
        let path = temp_path("group-length.dcm");
        let item = [
            explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
            explicit_element(0x3006, 0x0026, b"LO", HAN_IR13),
        ]
        .concat();
        let dataset = [
            with_group_length(0x0008, &base_elements(b"ISO_IR 192"), true),
            defined_sequence(0x3006, 0x0020, &[item]),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
        assert!(!has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
    }

    /// Review: files with repeated or unordered tags are not spliced.
    #[test]
    fn repeated_tags_are_not_indexed_for_splicing() {
        let path = temp_path("repeated-tag.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID002"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let index = splice::FileIndex::read(path.to_str().unwrap());
        std::fs::remove_file(&path).ok();

        assert!(index.unwrap_err().contains("repeated"));
    }

    /// Review on #12: files whose tags are out of order can still have values edited
    /// and elements removed by splicing; adding elements uses the full writer.
    #[test]
    fn unordered_tags_allow_value_edits_but_not_insertions() {
        let path = temp_path("unordered-tags.dcm");
        let item = [
            explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
            explicit_element(0x3006, 0x0026, b"LO", HAN_IR13),
        ]
        .concat();
        let dataset = [
            base_elements(b"ISO_IR 192"),
            // (0010,0020) before (0010,0010): out of order.
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            explicit_element(0x0010, 0x0010, b"PN", b"Yamada^Taro"),
            defined_sequence(0x3006, 0x0020, &[item]),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let mut edited = opened.clone();
        *find_element_mut(&mut edited, &["(0010,0020)"]).unwrap() = "ID002".to_string();
        let edited_planned = plan_for(&path, &edited);
        let edited_saved = save_file(&path, &edited);
        let mut added = opened.clone();
        added.push(element_node("(0008,1030)", "LO", "HEAD", &[]));
        let added_planned = plan_for(&path, &added);
        std::fs::remove_file(&path).ok();

        assert!(edited_planned);
        let saved = edited_saved.unwrap();
        assert!(contains(&saved, b"ID002"));
        assert!(contains(&saved, HAN_IR13));
        assert!(!added_planned);
    }

    /// Review on #12: nodes whose paths do not match their positions (e.g. an item
    /// moved) are not spliced, since paths address the file's elements.
    #[test]
    fn moved_items_are_not_spliced() {
        let path = temp_path("moved-items.dcm");
        let first = [
            explicit_element(0x3006, 0x0022, b"IS", b"1"),
            explicit_element(0x3006, 0x0026, b"LO", b"PTV"),
        ]
        .concat();
        let second = [
            explicit_element(0x3006, 0x0022, b"IS", b"2"),
            explicit_element(0x3006, 0x0026, b"LO", b"CTV"),
        ]
        .concat();
        let dataset = [
            base_elements(b"ISO_IR 192"),
            defined_sequence(0x3006, 0x0020, &[first, second]),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let unchanged = plan_for(&path, &nodes);
        if let Some(DicomNode::Sequence { items, .. }) = nodes
            .iter_mut()
            .find(|node| matches!(node, DicomNode::Sequence { tag, .. } if tag == "(3006,0020)"))
        {
            items.swap(0, 1);
        }
        let swapped = plan_for(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(unchanged);
        assert!(!swapped);
    }

    /// Review on #12: a file with tags out of order and text the full writer cannot
    /// write warns that adding elements fails.
    #[test]
    fn unordered_files_with_unwritable_text_warn_about_adding() {
        let path = temp_path("unordered-warning.dcm");
        let item = [
            explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
            explicit_element(0x3006, 0x0026, b"LO", HAN_IR13),
        ]
        .concat();
        let dataset = [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            explicit_element(0x0010, 0x0010, b"PN", b"Yamada^Taro"),
            defined_sequence(0x3006, 0x0020, &[item]),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).ok();

        assert!(has_warning(&content, TEXT_ADD_BLOCKED_WARNING));
        assert!(!has_warning(&content, TEXT_SAVE_BLOCKED_WARNING));
        assert_eq!(
            value_and_editable(&content.nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
    }

    // ---- Explicit VR Big Endian ----

    fn big_endian_element(group: u16, element: u16, vr: &[u8; 2], value: &[u8]) -> Vec<u8> {
        let mut value = value.to_vec();
        if value.len() % 2 == 1 {
            value.push(if vr == b"UI" { 0 } else { b' ' });
        }
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_be_bytes());
        out.extend_from_slice(&element.to_be_bytes());
        out.extend_from_slice(vr);
        if matches!(vr, b"OB" | b"OW" | b"UN" | b"UT") {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        } else {
            out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(&value);
        out
    }

    fn big_endian_item(content: &[u8], defined_length: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0xFF, 0xFE, 0xE0, 0x00]);
        if defined_length {
            out.extend_from_slice(&(content.len() as u32).to_be_bytes());
            out.extend_from_slice(content);
        } else {
            out.extend_from_slice(&u32::MAX.to_be_bytes());
            out.extend_from_slice(content);
            out.extend_from_slice(&[0xFF, 0xFE, 0xE0, 0x0D, 0, 0, 0, 0]);
        }
        out
    }

    fn big_endian_sequence(
        group: u16,
        element: u16,
        items: &[Vec<u8>],
        defined_length: bool,
    ) -> Vec<u8> {
        let content = items
            .iter()
            .flat_map(|item| big_endian_item(item, defined_length))
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        out.extend_from_slice(&group.to_be_bytes());
        out.extend_from_slice(&element.to_be_bytes());
        out.extend_from_slice(b"SQ\0\0");
        if defined_length {
            out.extend_from_slice(&(content.len() as u32).to_be_bytes());
            out.extend(content);
        } else {
            out.extend_from_slice(&u32::MAX.to_be_bytes());
            out.extend(content);
            out.extend_from_slice(&[0xFF, 0xFE, 0xE0, 0xDD, 0, 0, 0, 0]);
        }
        out
    }

    /// An Explicit VR Big Endian file: UTF-8 text, a group length, a US value, a
    /// defined-length and an undefined-length sequence, and native pixel data.
    fn write_big_endian_file(path: &std::path::Path) -> Vec<u8> {
        let patient = [
            big_endian_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
            big_endian_element(0x0010, 0x0020, b"LO", b"ID001"),
        ]
        .concat();
        let roi = [
            big_endian_element(0x3006, 0x0022, b"IS", b"1"),
            big_endian_element(0x3006, 0x0026, b"LO", "前立腺".as_bytes()),
        ]
        .concat();
        let dataset = [
            big_endian_element(0x0008, 0x0005, b"CS", b"ISO_IR 192"),
            big_endian_element(0x0008, 0x0016, b"UI", RTSTRUCT_SOP_CLASS.as_bytes()),
            big_endian_element(0x0008, 0x0018, b"UI", b"1.2.3.4"),
            big_endian_element(0x0008, 0x0060, b"CS", b"RTSTRUCT"),
            big_endian_element(0x0010, 0x0000, b"UL", &(patient.len() as u32).to_be_bytes()),
            patient,
            big_endian_element(0x0028, 0x0010, b"US", &512_u16.to_be_bytes()),
            big_endian_sequence(0x3006, 0x0020, &[roi], true),
            big_endian_sequence(
                0x3006,
                0x0039,
                &[big_endian_element(0x3006, 0x0084, b"IS", b"1")],
                false,
            ),
            big_endian_element(0x7FE0, 0x0010, b"OW", &[1, 2, 3, 4]),
        ]
        .concat();
        write_part10_with(path, "1.2.840.10008.1.2.2", &dataset);
        std::fs::read(path).unwrap()
    }

    #[test]
    fn big_endian_files_are_read() {
        let path = temp_path("big-endian-read.dcm");
        write_big_endian_file(&path);
        let content = open_nodes(path.to_str().unwrap()).unwrap();
        let index = splice::FileIndex::read(path.to_str().unwrap());
        std::fs::remove_file(&path).ok();

        assert!(index.unwrap().encoding().big_endian);
        let nodes = content.nodes;
        assert_eq!(
            value_and_editable(&nodes, "(0010,0010)"),
            ("山田^太郎".to_string(), true)
        );
        assert_eq!(value_and_editable(&nodes, "(0028,0010)").0, "512");
        assert_eq!(values_in(&nodes, "(3006,0026)"), "前立腺");
        assert_eq!(values_in(&nodes, "(3006,0084)"), "1");
    }

    #[test]
    fn big_endian_splice_without_edits_keeps_the_file_byte_for_byte() {
        let path = temp_path("big-endian-identity.dcm");
        let original = write_big_endian_file(&path);
        let nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let planned = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned);
        assert_eq!(saved.unwrap(), original);
    }

    /// Edits, added and removed elements, added sequences and items, group lengths,
    /// lengths of defined-length items and SOP UID changes, all in big endian (the file
    /// meta information stays little endian).
    #[test]
    fn big_endian_edits_are_spliced() {
        let path = temp_path("big-endian-edit.dcm");
        write_big_endian_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0010)"], "鈴木^花子子");
        set_value(&mut nodes, &["(0008,0018)"], "1.2.3.4.5");
        set_value(
            &mut nodes,
            &["(3006,0020)", "Item#0", "(3006,0026)"],
            "前立腺 PTV",
        );
        remove_element(&mut nodes, "(0010,0020)");
        let position = nodes
            .iter()
            .position(|node| matches!(node, DicomNode::Element { tag, .. } if tag == "(0010,0000)"))
            .unwrap();
        nodes.insert(position, element_node("(0008,1030)", "LO", "頭部", &[]));
        nodes.insert(
            position + 1,
            new_sequence(
                "(0008,1032)",
                &[],
                vec![vec![element_node(
                    "(0008,0104)",
                    "LO",
                    "胸部",
                    &["(0008,1032)", "Item#0"],
                )]],
            ),
        );
        append_item(&mut nodes, &["(3006,0039)"], &|parent| {
            vec![element_node("(3006,0084)", "IS", "2", parent)]
        });
        let planned = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned, "saving should splice");
        let saved = saved.unwrap();
        assert_eq!(wrong_group_lengths(&saved), Vec::<String>::new());
        // Text is written as is, with big-endian headers.
        let mut added = vec![0x00, 0x08, 0x10, 0x30, b'L', b'O', 0x00, 0x06];
        added.extend_from_slice("頭部".as_bytes());
        assert!(contains(&saved, &added));
        assert!(
            saved.ends_with(&[0x7F, 0xE0, 0x00, 0x10, b'O', b'W', 0, 0, 0, 0, 0, 4, 1, 2, 3, 4])
        );
        assert!(!contains(&saved, b"ID001"));
        let reopened = reopen("big-endian-edit-reopened.dcm", &saved);
        assert_eq!(values_in(&reopened, "(0010,0010)"), "鈴木^花子子");
        assert_eq!(values_in(&reopened, "(0008,1030)"), "頭部");
        assert_eq!(values_in(&reopened, "(0008,0104)"), "胸部");
        assert_eq!(values_in(&reopened, "(0028,0010)"), "512");
        assert!(find_element(&reopened, "(0010,0020)").is_none());
        assert_eq!(values_in(&reopened, "(3006,0026)"), "前立腺 PTV");
        let items = sequence_items_of(&reopened, "(3006,0039)");
        assert_eq!(items.len(), 2);
        assert_eq!(values_in(&items[1], "(3006,0084)"), "2");
        let saved_path = temp_path("big-endian-edit-meta.dcm");
        std::fs::write(&saved_path, &saved).unwrap();
        let object = open_full_object(saved_path.to_str().unwrap());
        std::fs::remove_file(&saved_path).ok();
        let object = object.unwrap();
        assert_eq!(
            object
                .meta()
                .media_storage_sop_instance_uid()
                .trim_end_matches('\0'),
            "1.2.3.4.5"
        );
        assert_eq!(
            object.meta().transfer_syntax().trim_end_matches('\0'),
            "1.2.840.10008.1.2.2"
        );
    }

    // ---- Deflated Explicit VR Little Endian ----

    /// The data set of a deflated file before deflating: UTF-8 text, an ISO_IR 13 item
    /// in a defined-length sequence, an undefined-length sequence and native pixel data.
    fn deflated_dataset() -> Vec<u8> {
        let item = [
            explicit_element(0x0008, 0x0005, b"CS", b"ISO_IR 13"),
            explicit_element(0x3006, 0x0022, b"IS", b"1"),
            explicit_element(0x3006, 0x0026, b"LO", HAN_IR13),
        ]
        .concat();
        let mut pixel_data = vec![0xE0, 0x7F, 0x10, 0x00, b'O', b'W', 0, 0];
        pixel_data.extend_from_slice(&8_u32.to_le_bytes());
        pixel_data.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        [
            base_elements(b"ISO_IR 192"),
            explicit_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            defined_sequence(0x3006, 0x0020, &[item]),
            sequence(
                0x3006,
                0x0039,
                &[explicit_element(0x3006, 0x0084, b"IS", b"1")],
                true,
            ),
            pixel_data,
        ]
        .concat()
    }

    fn write_deflated_file(path: &std::path::Path) -> Vec<u8> {
        let deflated = deflate::deflate(&deflated_dataset()).unwrap();
        write_part10_with(path, uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN, &deflated);
        std::fs::read(path).unwrap()
    }

    /// The data set of a Part 10 file, inflated when deflated.
    fn dataset_of(file: &[u8]) -> Vec<u8> {
        let mut reader = std::io::Cursor::new(&file[128..]);
        let meta = FileMetaTable::from_reader(&mut reader).unwrap();
        let rest = &file[128 + reader.position() as usize..];
        if deflate::is_deflated(meta.transfer_syntax()) {
            let mut inflated = Vec::new();
            deflate::inflate_into(&mut &rest[..], &mut inflated).unwrap();
            inflated
        } else {
            rest.to_vec()
        }
    }

    #[test]
    fn deflated_files_are_read() {
        let path = temp_path("deflated-read.dcm");
        write_deflated_file(&path);
        let path_text = path.to_str().unwrap();
        let content = open_nodes(path_text).unwrap();
        let full = open_full_object(path_text);
        std::fs::remove_file(&path).ok();

        assert!(content.warnings.is_empty(), "{:?}", content.warnings);
        let nodes = content.nodes;
        assert_eq!(
            value_and_editable(&nodes, "(0010,0010)"),
            ("山田^太郎".to_string(), true)
        );
        assert_eq!(
            value_and_editable(&nodes, "(3006,0026)"),
            ("ﾊﾝ".to_string(), true)
        );
        assert_eq!(values_in(&nodes, "(3006,0084)"), "1");
        let full = full.unwrap();
        assert_eq!(
            full.meta().transfer_syntax(),
            uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN
        );
        assert_eq!(
            full.element(PIXEL_DATA)
                .unwrap()
                .to_bytes()
                .unwrap()
                .as_ref(),
            &[1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn deflated_splice_without_edits_keeps_the_file_byte_for_byte() {
        let path = temp_path("deflated-identity.dcm");
        let original = write_deflated_file(&path);
        let nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        let planned = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned);
        assert_eq!(saved.unwrap(), original);
    }

    /// Edits to a deflated file give the same data set as the same edits to the file
    /// not deflated, deflated again, with the file meta information kept.
    #[test]
    fn deflated_edits_are_spliced() {
        let edit = |nodes: &mut Vec<DicomNode>| {
            set_value(nodes, &["(0010,0010)"], "鈴木^花子子");
            set_value(nodes, &["(0008,0018)"], "1.2.3.4.5");
            set_value(nodes, &["(3006,0020)", "Item#0", "(3006,0026)"], "ﾊﾝﾀﾞ PTV");
            remove_element(nodes, "(0010,0020)");
            nodes.insert(0, element_node("(0008,1030)", "LO", "頭部", &[]));
            nodes.insert(
                1,
                new_sequence(
                    "(0008,1032)",
                    &[],
                    vec![vec![element_node(
                        "(0008,0104)",
                        "LO",
                        "胸部",
                        &["(0008,1032)", "Item#0"],
                    )]],
                ),
            );
            append_item(nodes, &["(3006,0039)"], &|parent| {
                vec![element_node("(3006,0084)", "IS", "2", parent)]
            });
        };
        let plain_path = temp_path("deflated-edit-plain.dcm");
        write_part10_with(
            &plain_path,
            uids::EXPLICIT_VR_LITTLE_ENDIAN,
            &deflated_dataset(),
        );
        let mut plain_nodes = open_nodes(plain_path.to_str().unwrap()).unwrap().nodes;
        edit(&mut plain_nodes);
        let plain_saved = save_file(&plain_path, &plain_nodes);
        std::fs::remove_file(&plain_path).ok();

        let path = temp_path("deflated-edit.dcm");
        let original = write_deflated_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        edit(&mut nodes);
        let planned = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned, "saving should splice");
        let saved = saved.unwrap();
        assert_eq!(saved.len() % 2, 0);
        assert_eq!(dataset_of(&saved), dataset_of(&plain_saved.unwrap()));
        // The file meta information is as before, apart from the SOP Instance UID.
        let meta_length = original.len() - write_deflated_dataset_length();
        assert_eq!(&saved[..128], &original[..128]);
        assert!(contains(&saved[..meta_length + 2], b"1.2.3.4.5"));
        let reopened = reopen("deflated-edit-reopened.dcm", &saved);
        assert_eq!(values_in(&reopened, "(0010,0010)"), "鈴木^花子子");
        assert_eq!(values_in(&reopened, "(3006,0026)"), "ﾊﾝﾀﾞ PTV");
        assert_eq!(values_in(&reopened, "(0008,0104)"), "胸部");
        assert_eq!(sequence_items_of(&reopened, "(3006,0039)").len(), 2);
        let saved_path = temp_path("deflated-edit-meta.dcm");
        std::fs::write(&saved_path, &saved).unwrap();
        let object = open_full_object(saved_path.to_str().unwrap());
        std::fs::remove_file(&saved_path).ok();
        let object = object.unwrap();
        assert_eq!(
            object.meta().transfer_syntax(),
            uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN
        );
        assert_eq!(
            object
                .meta()
                .media_storage_sop_instance_uid()
                .trim_end_matches('\0'),
            "1.2.3.4.5"
        );
    }

    fn write_deflated_dataset_length() -> usize {
        deflate::deflate(&deflated_dataset()).unwrap().len()
    }

    /// dicom-rs cannot write deflated data sets, so changes splicing cannot save fail
    /// with a message, and the file is left as it was.
    #[test]
    fn deflated_changes_needing_the_full_writer_are_refused() {
        let path = temp_path("deflated-refused.dcm");
        let original = write_deflated_file(&path);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        nodes.insert(0, element_node("(0008,0000)", "UL", "4", &[]));
        let saved = save_file(&path, &nodes);
        let after = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let error = saved.unwrap_err();
        assert!(error.contains("deflated"), "{error}");
        assert_eq!(after, original);
    }

    /// Review on #26: a cut-short Deflate stream is not read as a shorter data set
    /// (which saving would write back), and data after the stream is not dropped.
    #[test]
    fn deflated_streams_must_end() {
        let deflated = deflate::deflate(&deflated_dataset()).unwrap();
        for (name, bytes, expected) in [
            (
                "cut",
                deflated[..deflated.len() - 6].to_vec(),
                "ends unexpectedly",
            ),
            (
                "trailing",
                [deflated.as_slice(), b"XY"].concat(),
                "Unexpected data after",
            ),
        ] {
            let path = temp_path(&format!("deflated-{name}.dcm"));
            write_part10_with(&path, uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN, &bytes);
            let path_text = path.to_str().unwrap();
            let opened = open_nodes(path_text);
            let index = splice::FileIndex::read(path_text);
            let text = TextValues::try_read(path_text);
            std::fs::remove_file(&path).ok();

            let error = opened.err().unwrap_or_default();
            assert!(error.contains(expected), "{name}: {error}");
            assert!(index.is_err(), "{name}");
            assert!(text.is_err(), "{name}");
        }
        // Zero padding after the stream is fine.
        let path = temp_path("deflated-padding.dcm");
        write_part10_with(
            &path,
            uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN,
            &[deflated.as_slice(), &[0, 0]].concat(),
        );
        let opened = open_nodes(path.to_str().unwrap());
        std::fs::remove_file(&path).ok();
        assert_eq!(values_in(&opened.unwrap().nodes, "(0010,0020)"), "ID001");
    }

    /// Review on #26: deflated files are read up to where Pixel Data starts in the file,
    /// like other files, so that a broken Pixel Data or elements after it (with tags out
    /// of order) do not change the tree.
    #[test]
    fn deflated_files_are_read_up_to_pixel_data() {
        let mut broken_pixel_data = vec![0xE0, 0x7F, 0x10, 0x00, b'O', b'W', 0, 0];
        broken_pixel_data.extend_from_slice(&1000_u32.to_le_bytes());
        broken_pixel_data.extend_from_slice(&[1, 2, 3, 4]);
        let mut pixel_data = vec![0xE0, 0x7F, 0x10, 0x00, b'O', b'W', 0, 0];
        pixel_data.extend_from_slice(&4_u32.to_le_bytes());
        pixel_data.extend_from_slice(&[1, 2, 3, 4]);
        for (name, tail) in [
            ("broken", broken_pixel_data),
            (
                "unordered",
                [
                    pixel_data,
                    explicit_element(0x0010, 0x0030, b"DA", b"20000101"),
                ]
                .concat(),
            ),
        ] {
            let dataset = [
                base_elements(b"ISO_IR 192"),
                explicit_element(0x0010, 0x0010, b"PN", "山田^太郎".as_bytes()),
                tail,
            ]
            .concat();
            let path = temp_path(&format!("deflated-pixel-{name}.dcm"));
            write_part10_with(
                &path,
                uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN,
                &deflate::deflate(&dataset).unwrap(),
            );
            let opened = open_nodes(path.to_str().unwrap());
            std::fs::remove_file(&path).ok();

            let nodes = opened.unwrap().nodes;
            assert_eq!(values_in(&nodes, "(0010,0010)"), "山田^太郎", "{name}");
            assert!(find_element(&nodes, "(0010,0030)").is_none(), "{name}");
        }
    }

    /// #15: the full writer writes elements in tag order, so an unchanged US element
    /// stored before Pixel Representation 1 would read back as SS. Saves that need it
    /// fail instead; spliced saves keep the order and still work.
    #[test]
    fn implicit_vr_full_writer_keeps_us_or_ss_readings() {
        let path = temp_path("implicit-xs-reorder.dcm");
        let dataset = [
            implicit_uid(0x0008, 0x0016, RTSTRUCT_SOP_CLASS),
            implicit_uid(0x0008, 0x0018, "1.2.3.4"),
            implicit_element(0x0010, 0x0010, b"Yamada^Tarou"),
            implicit_element(0x0028, 0x0106, &u16::MAX.to_le_bytes()),
            implicit_element(0x0028, 0x0103, &1_u16.to_le_bytes()),
        ]
        .concat();
        write_part10_with(&path, uids::IMPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let original = std::fs::read(&path).unwrap();
        let opened = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        assert_eq!(values_in(&opened, "(0028,0106)"), "65535");
        // Adding an element to a file with tags out of order needs the full writer.
        let mut added = opened.clone();
        added.insert(0, element_node("(0008,1030)", "LO", "Head", &[]));
        let reordered = save_file(&path, &added);
        let after = std::fs::read(&path).unwrap();
        let mut edited = opened.clone();
        set_value(&mut edited, &["(0010,0010)"], "Suzuki^Hanako");
        let spliced = save_file(&path, &edited);
        std::fs::remove_file(&path).ok();

        let error = reordered.unwrap_err();
        assert!(error.contains("(0028,0106)"), "{error}");
        assert!(error.contains("tag order"), "{error}");
        assert_eq!(after, original);
        let reopened = reopen("implicit-xs-reorder-reopened.dcm", &spliced.unwrap());
        assert_eq!(values_in(&reopened, "(0028,0106)"), "65535");
        assert_eq!(values_in(&reopened, "(0010,0010)"), "Suzuki^Hanako");
    }

    /// #23: with tags out of order, a group split by an element of another group is one
    /// group once that element is removed, and its length counts both parts.
    #[test]
    fn group_lengths_join_parts_when_the_elements_between_are_removed() {
        let path = temp_path("group-length-join.dcm");
        let dataset = [
            base_elements(b"ISO_IR 192"),
            with_group_length(
                0x0010,
                &explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
                true,
            ),
            explicit_element(0x0018, 0x0015, b"CS", b"HEAD"),
            explicit_element(0x0010, 0x0030, b"DA", b"19700101"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        remove_element(&mut nodes, "(0018,0015)");
        let planned = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned);
        let saved = saved.unwrap();
        assert!(!contains(&saved, b"HEAD"));
        assert_eq!(wrong_group_lengths(&saved), Vec::<String>::new());
    }

    /// #23: editing only the part of a split group after another group's element does
    /// not rewrite the group length of the first part.
    #[test]
    fn group_lengths_of_other_parts_are_kept() {
        let path = temp_path("group-length-other-part.dcm");
        // A group length that does not match its group, to see whether it is rewritten.
        let mut wrong = vec![0x10, 0x00, 0x00, 0x00, b'U', b'L', 4, 0];
        wrong.extend_from_slice(&999_u32.to_le_bytes());
        let dataset = [
            base_elements(b"ISO_IR 192"),
            wrong.clone(),
            explicit_element(0x0010, 0x0020, b"LO", b"ID001"),
            explicit_element(0x0018, 0x0015, b"CS", b"HEAD"),
            explicit_element(0x0010, 0x0030, b"DA", b"19700101"),
        ]
        .concat();
        write_part10_with(&path, uids::EXPLICIT_VR_LITTLE_ENDIAN, &dataset);
        let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
        set_value(&mut nodes, &["(0010,0030)"], "19800101");
        let planned = plans_splice(&path, &nodes);
        let saved = save_file(&path, &nodes);
        std::fs::remove_file(&path).ok();

        assert!(planned);
        let saved = saved.unwrap();
        assert!(contains(&saved, &wrong));
        assert!(contains(&saved, b"19800101"));
    }

    /// #13: items moved to other positions, as `item_sources` records them, are not
    /// spliced or written by position (which would mix their values with other items'),
    /// with or without other edits; the save fails and the file is kept.
    #[test]
    fn moved_items_are_refused() {
        for edit in [false, true] {
            let path = temp_path(&format!("move-item-{edit}.dcm"));
            let original = write_three_rois(&path, true);
            let mut nodes = open_nodes(path.to_str().unwrap()).unwrap().nodes;
            for node in nodes.iter_mut() {
                if let DicomNode::Sequence {
                    tag,
                    items,
                    item_sources,
                    ..
                } = node
                {
                    if tag == "(3006,0020)" {
                        items.swap(0, 1);
                        *item_sources = Some(vec![Some(1), Some(0), Some(2)]);
                        for (position, item) in items.iter_mut().enumerate() {
                            for node in item.iter_mut() {
                                let (DicomNode::Element { path, .. }
                                | DicomNode::Sequence { path, .. }) = node;
                                path[1] = format!("Item#{position}");
                            }
                        }
                    }
                }
            }
            if edit {
                set_value(&mut nodes, &["(3006,0020)", "Item#0", "(3006,0026)"], "PTV");
            }
            let saved = save_file(&path, &nodes);
            let after = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).ok();

            assert!(saved.is_err(), "edit: {edit}");
            assert_eq!(after, original);
        }
    }
}
