mod commands;
mod dicom;

use commands::file::{
    open_dicom_file, save_dicom_file, save_dicom_file_as, take_launch_file_paths, DicomStore,
    LaunchFileStore,
};
use commands::image::{get_dicom_frame_image, get_dicom_frame_pixels, get_dicom_image_info};
use commands::plan::get_rt_plan_bev_info;
use commands::tags::lookup_dicom_tag;
use commands::validate::validate_value;

#[cfg(target_os = "macos")]
use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DicomStore::default())
        .manage(LaunchFileStore::default())
        .invoke_handler(tauri::generate_handler![
            open_dicom_file,
            save_dicom_file,
            save_dicom_file_as,
            take_launch_file_paths,
            get_dicom_image_info,
            get_dicom_frame_image,
            get_dicom_frame_pixels,
            get_rt_plan_bev_info,
            lookup_dicom_tag,
            validate_value
        ])
        .build(tauri::generate_context!())
        .expect("error while building Taglet");

    app.run(handle_run_event);
}

#[cfg(target_os = "macos")]
fn handle_run_event(app_handle: &tauri::AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Opened { urls } = event {
        let paths = urls
            .into_iter()
            .filter_map(|url| url.to_file_path().ok())
            .filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        if !paths.is_empty() {
            let _ = app_handle
                .state::<LaunchFileStore>()
                .push_paths(paths.clone());
            let _ = app_handle.emit("taglet://open-files", paths);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn handle_run_event(_app_handle: &tauri::AppHandle, _event: tauri::RunEvent) {}
