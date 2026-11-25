use tauri::State;

use crate::commands::file::DicomStore;
use crate::dicom::model::RtPlanBevInfo;
use crate::dicom::parser::rt_plan_bev_info;
use crate::dicom::text_values::TextValues;

#[tauri::command]
pub async fn get_rt_plan_bev_info(store: State<'_, DicomStore>) -> Result<RtPlanBevInfo, String> {
    // Read the text values and the object from the same file, even if the current file
    // changes meanwhile.
    let path = store.current_path()?;
    let text = TextValues::read(&path);
    store.with_full_object(&path, |object| rt_plan_bev_info(object, &text))
}
