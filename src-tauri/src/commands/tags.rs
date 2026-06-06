use crate::dicom::model::DicomTagInfo;
use crate::dicom::parser::tag_info;

#[tauri::command]
pub fn lookup_dicom_tag(tag: String) -> Result<DicomTagInfo, String> {
    tag_info(&tag)
}
