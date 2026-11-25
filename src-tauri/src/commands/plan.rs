use tauri::State;

use crate::commands::file::DicomStore;
use crate::dicom::model::RtPlanBevInfo;
use crate::dicom::parser::rt_plan_bev_info;
use crate::dicom::text_values::TextValues;

#[tauri::command]
pub async fn get_rt_plan_bev_info(store: State<'_, DicomStore>) -> Result<RtPlanBevInfo, String> {
    let text = TextValues::read(&store.current_path()?);
    store.with_current_full_object(|object| rt_plan_bev_info(object, &text))
}
