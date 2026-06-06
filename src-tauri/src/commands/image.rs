use tauri::State;

use crate::commands::file::DicomStore;
use crate::dicom::model::{DicomFrameImage, DicomFramePixels, DicomImageInfo};
use crate::dicom::parser::{frame_image, frame_pixels, image_info};

#[tauri::command]
pub fn get_dicom_image_info(store: State<'_, DicomStore>) -> Result<DicomImageInfo, String> {
    store.with_current_full_object(image_info)
}

#[tauri::command]
pub fn get_dicom_frame_image(
    frame_index: u32,
    window_center: Option<f64>,
    window_width: Option<f64>,
    store: State<'_, DicomStore>,
) -> Result<DicomFrameImage, String> {
    store.with_current_full_object(|object| {
        frame_image(object, frame_index, window_center, window_width)
    })
}

#[tauri::command]
pub fn get_dicom_frame_pixels(
    frame_index: u32,
    store: State<'_, DicomStore>,
) -> Result<DicomFramePixels, String> {
    store.with_current_full_object(|object| frame_pixels(object, frame_index))
}
