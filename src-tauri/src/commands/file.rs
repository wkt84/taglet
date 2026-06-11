use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use dicom_object::DefaultDicomObject;
use tauri::State;

use crate::dicom::model::{DicomNode, RtStructData};
use crate::dicom::parser::{apply_nodes_to_object, open_full_object, open_nodes, rt_struct_data};

struct StoredDicomObject {
    full: Option<DefaultDicomObject>,
    rt_struct: Option<RtStructData>,
}

#[derive(Default)]
pub struct DicomStore {
    objects: Mutex<HashMap<String, StoredDicomObject>>,
    current_path: Mutex<Option<String>>,
}

pub struct LaunchFileStore {
    paths: Mutex<Vec<String>>,
}

impl Default for LaunchFileStore {
    fn default() -> Self {
        Self::from_env()
    }
}

impl LaunchFileStore {
    pub fn from_env() -> Self {
        Self {
            paths: Mutex::new(
                std::env::args_os()
                    .skip(1)
                    .map(PathBuf::from)
                    .filter(|path| path.is_file())
                    .map(|path| path.to_string_lossy().to_string())
                    .collect(),
            ),
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn push_paths(&self, paths: Vec<String>) -> Result<(), String> {
        self.paths
            .lock()
            .map_err(|_| "Launch file store lock poisoned".to_string())?
            .extend(paths);
        Ok(())
    }

    pub fn take_paths(&self) -> Result<Vec<String>, String> {
        let mut paths = self
            .paths
            .lock()
            .map_err(|_| "Launch file store lock poisoned".to_string())?;
        Ok(std::mem::take(&mut *paths))
    }
}

impl DicomStore {
    pub fn with_current_full_object<R>(
        &self,
        f: impl FnOnce(&DefaultDicomObject) -> Result<R, String>,
    ) -> Result<R, String> {
        let current_path = self
            .current_path
            .lock()
            .map_err(|_| "DICOM store lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "No DICOM file is currently open".to_string())?;
        let mut objects = self
            .objects
            .lock()
            .map_err(|_| "DICOM store lock poisoned".to_string())?;
        let stored = objects
            .get(&current_path)
            .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;
        if stored.full.is_none() {
            drop(objects);
            let full = open_full_object(&current_path)?;
            objects = self
                .objects
                .lock()
                .map_err(|_| "DICOM store lock poisoned".to_string())?;
            let stored = objects
                .get_mut(&current_path)
                .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;
            stored.full = Some(full);
        }

        let object = objects
            .get(&current_path)
            .and_then(|stored| stored.full.as_ref())
            .ok_or_else(|| "Full DICOM object is not loaded in this session".to_string())?;
        f(object)
    }

    pub fn with_current_rt_struct_data<R>(
        &self,
        f: impl FnOnce(&RtStructData) -> Result<R, String>,
    ) -> Result<R, String> {
        let current_path = self
            .current_path
            .lock()
            .map_err(|_| "DICOM store lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "No DICOM file is currently open".to_string())?;
        let mut objects = self
            .objects
            .lock()
            .map_err(|_| "DICOM store lock poisoned".to_string())?;
        let stored = objects
            .get(&current_path)
            .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;
        if stored.full.is_none() {
            drop(objects);
            let full = open_full_object(&current_path)?;
            objects = self
                .objects
                .lock()
                .map_err(|_| "DICOM store lock poisoned".to_string())?;
            let stored = objects
                .get_mut(&current_path)
                .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;
            stored.full = Some(full);
        }

        let stored = objects
            .get_mut(&current_path)
            .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;
        if stored.rt_struct.is_none() {
            let object = stored
                .full
                .as_ref()
                .ok_or_else(|| "Full DICOM object is not loaded in this session".to_string())?;
            stored.rt_struct = Some(rt_struct_data(object)?);
        }

        let data = stored
            .rt_struct
            .as_ref()
            .ok_or_else(|| "RT Structure data is not loaded in this session".to_string())?;
        f(data)
    }
}

#[tauri::command]
pub async fn set_current_dicom_file(
    path: Option<String>,
    store: State<'_, DicomStore>,
) -> Result<(), String> {
    if let Some(path) = path.as_ref() {
        let objects = store
            .objects
            .lock()
            .map_err(|_| "DICOM store lock poisoned".to_string())?;
        if !objects.contains_key(path) {
            return Err("DICOM object is not loaded in this session".to_string());
        }
    }

    *store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())? = path;
    Ok(())
}

#[tauri::command]
pub async fn take_launch_file_paths(
    store: State<'_, LaunchFileStore>,
) -> Result<Vec<String>, String> {
    store.take_paths()
}

#[tauri::command]
pub async fn open_dicom_file(
    path: String,
    store: State<'_, DicomStore>,
) -> Result<Vec<DicomNode>, String> {
    let (_preview, nodes) = open_nodes(&path)?;
    store
        .objects
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())?
        .insert(
            path.clone(),
            StoredDicomObject {
                full: None,
                rt_struct: None,
            },
        );
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
    let stored = objects
        .get_mut(&source_path)
        .ok_or_else(|| "Original DICOM object is not loaded in this session".to_string())?;

    if stored.full.is_none() {
        stored.full = Some(open_full_object(&source_path)?);
    }
    let object = stored
        .full
        .as_mut()
        .ok_or_else(|| "Full DICOM object is not loaded in this session".to_string())?;

    apply_nodes_to_object(object, &nodes)?;
    stored.rt_struct = None;
    object
        .write_to_file(&destination_path)
        .map_err(|error| error.to_string())?;

    if source_path != destination_path {
        objects.insert(
            destination_path.clone(),
            StoredDicomObject {
                full: None,
                rt_struct: None,
            },
        );
    }
    *store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())? = Some(destination_path);

    Ok(())
}
