use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind")]
pub enum DicomNode {
    Element {
        tag: String,
        vr: String,
        description: String,
        value: String,
        length: u32,
        path: Vec<String>,
        editable: bool,
    },
    Sequence {
        tag: String,
        description: String,
        length: u32,
        path: Vec<String>,
        items: Vec<Vec<DicomNode>>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DicomTagInfo {
    pub tag: String,
    pub vr: String,
    pub description: String,
    pub editable: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DicomImageInfo {
    pub has_pixel_data: bool,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    pub modality: Option<String>,
    pub sop_class_uid: Option<String>,
    pub transfer_syntax_uid: Option<String>,
    pub rows: Option<u32>,
    pub columns: Option<u32>,
    pub samples_per_pixel: Option<u32>,
    pub photometric_interpretation: Option<String>,
    pub bits_allocated: Option<u32>,
    pub bits_stored: Option<u32>,
    pub high_bit: Option<u32>,
    pub pixel_representation: Option<u32>,
    pub number_of_frames: u32,
    pub window_center: Vec<f64>,
    pub window_width: Vec<f64>,
    pub rescale_intercept: Option<f64>,
    pub rescale_slope: Option<f64>,
    pub dose_grid_scaling: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DicomFrameImage {
    pub width: u32,
    pub height: u32,
    pub frame_index: u32,
    pub rgba_base64: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DicomFramePixels {
    pub width: u32,
    pub height: u32,
    pub frame_index: u32,
    pub bits_allocated: u32,
    pub pixel_representation: u32,
    pub photometric_interpretation: Option<String>,
    pub rescale_intercept: f64,
    pub rescale_slope: f64,
    pub dose_grid_scaling: Option<f64>,
    pub pixel_base64: String,
    pub min_value: f64,
    pub max_value: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RtPlanBevInfo {
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    pub modality: Option<String>,
    pub beams: Vec<RtPlanBeam>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RtPlanBeam {
    pub beam_index: usize,
    pub beam_number: Option<i32>,
    pub beam_name: Option<String>,
    pub devices: Vec<RtPlanBeamLimitingDeviceDefinition>,
    pub control_points: Vec<RtPlanControlPoint>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RtPlanBeamLimitingDeviceDefinition {
    pub device_type: String,
    pub number_of_pairs: Option<u32>,
    pub leaf_position_boundaries: Vec<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RtPlanControlPoint {
    pub control_point_index: usize,
    pub nominal_index: Option<i32>,
    pub gantry_angle: Option<f64>,
    pub gantry_angle_inherited: bool,
    pub collimator_angle: Option<f64>,
    pub collimator_angle_inherited: bool,
    pub couch_angle: Option<f64>,
    pub couch_angle_inherited: bool,
    pub devices: Vec<RtPlanBeamLimitingDevicePosition>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RtPlanBeamLimitingDevicePosition {
    pub device_type: String,
    pub positions: Vec<f64>,
    pub inherited: bool,
}

impl ValidationResult {
    pub fn valid() -> Self {
        Self {
            valid: true,
            message: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            valid: false,
            message: Some(message.into()),
        }
    }
}
