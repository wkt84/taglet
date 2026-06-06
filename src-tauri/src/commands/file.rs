use std::collections::HashMap;
use std::sync::Mutex;

use dicom_object::DefaultDicomObject;
use tauri::State;

use crate::dicom::model::DicomNode;
use crate::dicom::parser::{apply_nodes_to_object, open_nodes};

#[derive(Default)]
pub struct DicomStore {
    objects: Mutex<HashMap<String, DefaultDicomObject>>,
    current_path: Mutex<Option<String>>,
}

#[tauri::command]
pub async fn open_dicom_file(
    path: String,
    store: State<'_, DicomStore>,
) -> Result<Vec<DicomNode>, String> {
    let (object, nodes) = open_nodes(&path)?;
    store
        .objects
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())?
        .insert(path.clone(), object);
    *store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())? = Some(path);
    Ok(nodes)
}

#[tauri::command]
pub async fn save_dicom_file(
    path: String,
    nodes: Vec<DicomNode>,
    store: State<'_, DicomStore>,
) -> Result<(), String> {
    save_to_path(path.clone(), path, nodes, store)
}

#[tauri::command]
pub async fn save_dicom_file_as(
    path: String,
    nodes: Vec<DicomNode>,
    store: State<'_, DicomStore>,
) -> Result<(), String> {
    let source_path = store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())?
        .clone()
        .ok_or_else(|| "No DICOM file is currently open".to_string())?;
    save_to_path(source_path, path, nodes, store)
}

fn save_to_path(
    source_path: String,
    destination_path: String,
    nodes: Vec<DicomNode>,
    store: State<'_, DicomStore>,
) -> Result<(), String> {
    let mut objects = store
        .objects
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())?;
    let object = objects
        .get_mut(&source_path)
        .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;

    apply_nodes_to_object(object, &nodes)?;
    object
        .write_to_file(&destination_path)
        .map_err(|error| error.to_string())?;

    if source_path != destination_path {
        let cloned = object.clone();
        objects.insert(destination_path.clone(), cloned);
    }
    *store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())? = Some(destination_path);

    Ok(())
}
