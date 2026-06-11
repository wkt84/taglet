use tauri::State;

use crate::commands::file::DicomStore;
use crate::dicom::model::{RtStructInfo, RtStructSliceContours};
use crate::dicom::parser::{rt_struct_info, rt_struct_slice_contours};

#[tauri::command]
pub fn get_rt_struct_info(store: State<'_, DicomStore>) -> Result<RtStructInfo, String> {
    store.with_current_full_object(rt_struct_info)
}

#[tauri::command]
pub fn get_rt_struct_slice_contours(
    z: f64,
    roi_numbers: Vec<i32>,
    store: State<'_, DicomStore>,
) -> Result<RtStructSliceContours, String> {
    store.with_current_full_object(|object| rt_struct_slice_contours(object, z, roi_numbers))
}
