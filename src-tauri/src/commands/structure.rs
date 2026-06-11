use tauri::State;

use crate::commands::file::DicomStore;
use crate::dicom::model::{RtStructInfo, RtStructSliceContours};
use crate::dicom::parser::rt_struct_slice_contours_from_data;

#[tauri::command]
pub fn get_rt_struct_info(store: State<'_, DicomStore>) -> Result<RtStructInfo, String> {
    store.with_current_rt_struct_data(|data| Ok(data.info.clone()))
}

#[tauri::command]
pub fn get_rt_struct_slice_contours(
    z: f64,
    roi_numbers: Vec<i32>,
    store: State<'_, DicomStore>,
) -> Result<RtStructSliceContours, String> {
    store.with_current_rt_struct_data(|data| {
        rt_struct_slice_contours_from_data(data, z, roi_numbers)
    })
}
