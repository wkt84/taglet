use crate::dicom::model::ValidationResult;
use crate::dicom::vr;

#[tauri::command]
pub fn validate_value(vr: String, value: String) -> ValidationResult {
    vr::validate(&vr, &value)
}
