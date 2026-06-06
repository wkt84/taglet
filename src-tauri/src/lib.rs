mod commands;
mod dicom;

use commands::file::{open_dicom_file, save_dicom_file, save_dicom_file_as, DicomStore};
use commands::image::{get_dicom_frame_image, get_dicom_frame_pixels, get_dicom_image_info};
use commands::tags::lookup_dicom_tag;
use commands::validate::validate_value;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DicomStore::default())
        .invoke_handler(tauri::generate_handler![
            open_dicom_file,
            save_dicom_file,
            save_dicom_file_as,
            get_dicom_image_info,
            get_dicom_frame_image,
            get_dicom_frame_pixels,
            lookup_dicom_tag,
            validate_value
        ])
        .run(tauri::generate_context!())
        .expect("error while running Taglet");
}
