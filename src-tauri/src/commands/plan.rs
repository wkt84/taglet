use tauri::State;

use crate::commands::file::DicomStore;
use crate::dicom::model::RtPlanBevInfo;
use crate::dicom::parser::rt_plan_bev_info;

#[tauri::command]
pub async fn get_rt_plan_bev_info(
    store: State<'_, DicomStore>,
) -> Result<RtPlanBevInfo, String> {
    store.with_current_full_object(rt_plan_bev_info)
}
