use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use dicom_core::Tag;
use dicom_object::DefaultDicomObject;
use tauri::State;

use crate::dicom::model::{DicomFileContent, DicomNode, RtStructData};
use crate::dicom::parser::{
    open_full_object, open_nodes, rt_struct_data, save_file_with_nodes, trailing_tags,
    FILE_CHANGED_ON_DISK,
};
use crate::dicom::splice::file_stamp;
use crate::dicom::text_values::TextValues;

struct StoredDicomObject {
    full: Option<DefaultDicomObject>,
    rt_struct: Option<RtStructData>,
    /// The file's length and modification time when it was opened or last saved, to
    /// tell whether another program changed it since.
    stamp: Option<(u64, std::time::SystemTime)>,
    /// The top-level elements from Pixel Data's tag on that the shown nodes leave out
    /// and saving keeps (see `parser::HiddenTrailing`): none for raw data sets read
    /// without file meta information, which are shown whole. For Part 10 files, the
    /// ones in the file as it was opened, recorded when it is first saved (reading them
    /// when opening would read the whole file); until then `None`. Kept until the file
    /// is opened again, so that elements added after Pixel Data's tag and shown since
    /// can be removed.
    hidden_trailing: Option<HashSet<Tag>>,
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
    pub fn current_path(&self) -> Result<String, String> {
        self.current_path
            .lock()
            .map_err(|_| "DICOM store lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "No DICOM file is currently open".to_string())
    }

    pub fn with_current_full_object<R>(
        &self,
        f: impl FnOnce(&DefaultDicomObject) -> Result<R, String>,
    ) -> Result<R, String> {
        self.with_full_object(&self.current_path()?, f)
    }

    /// Run `f` with the full object of the file at `current_path`.
    pub fn with_full_object<R>(
        &self,
        current_path: &str,
        f: impl FnOnce(&DefaultDicomObject) -> Result<R, String>,
    ) -> Result<R, String> {
        let current_path = current_path.to_string();
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
            stored.rt_struct = Some(rt_struct_data(object, &TextValues::read(&current_path))?);
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
) -> Result<DicomFileContent, String> {
    // Taken before reading, so that a change made while reading is noticed on save.
    let stamp = file_stamp(&path);
    let content = open_nodes(&path)?;
    let hidden_trailing = content.file_meta.is_empty().then(HashSet::new);
    store
        .objects
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())?
        .insert(
            path.clone(),
            StoredDicomObject {
                full: None,
                rt_struct: None,
                stamp,
                hidden_trailing,
            },
        );
    *store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())? = Some(path);
    Ok(content)
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
    // The shown nodes come from the file as it was opened (or last saved). If another
    // program changed it since, saving them would silently undo those changes.
    let opened_stamp = store
        .objects
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())?
        .get(&source_path)
        .and_then(|stored| stored.stamp);
    ensure_unchanged_since_opened(opened_stamp, &source_path)?;

    // Text values read from the original bytes: their character sets are used to
    // encode edits, and the full dicom-rs writer (used when the edits cannot be spliced
    // into the original file) is checked against them.
    let text = TextValues::try_read(&source_path)
        .map_err(|error| format!("Could not check text values before saving: {error}"))?;

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
    // The object is the file as opened (or as last saved, in which case the hidden
    // elements were recorded at the first save).
    if stored.hidden_trailing.is_none() {
        stored.hidden_trailing = stored.full.as_ref().map(|full| trailing_tags(full));
    }
    let hidden_trailing = stored.hidden_trailing.clone();
    let object = stored
        .full
        .as_mut()
        .ok_or_else(|| "Full DICOM object is not loaded in this session".to_string())?;

    let object_matches_file = save_file_with_nodes(
        &source_path,
        object,
        &nodes,
        &text,
        &destination_path,
        opened_stamp,
        hidden_trailing.as_ref(),
    )?;
    if !object_matches_file || source_path != destination_path {
        // After a spliced save the object is read again from the file when needed.
        // After Save As, the source file is unchanged, so its object must not keep
        // the edits.
        stored.full = None;
    }
    if source_path == destination_path {
        stored.stamp = file_stamp(&destination_path);
    }
    stored.rt_struct = None;

    if source_path != destination_path {
        objects.insert(
            destination_path.clone(),
            StoredDicomObject {
                full: None,
                rt_struct: None,
                stamp: file_stamp(&destination_path),
                hidden_trailing,
            },
        );
    }
    *store
        .current_path
        .lock()
        .map_err(|_| "DICOM store lock poisoned".to_string())? = Some(destination_path);

    Ok(())
}

/// Fail when the file at `path` no longer has the length and modification time it had
/// when it was opened (or last saved).
fn ensure_unchanged_since_opened(
    opened: Option<(u64, std::time::SystemTime)>,
    path: &str,
) -> Result<(), String> {
    if opened.is_some() && opened != file_stamp(path) {
        return Err(FILE_CHANGED_ON_DISK.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_files_changed_after_opening() {
        let path = std::env::temp_dir().join(format!("taglet-{}-stamp.dcm", std::process::id()));
        std::fs::write(&path, b"original").unwrap();
        let path_text = path.to_str().unwrap();
        let opened = file_stamp(path_text);
        let unchanged = ensure_unchanged_since_opened(opened, path_text);
        std::fs::write(&path, b"changed by another program").unwrap();
        let changed = ensure_unchanged_since_opened(opened, path_text);
        std::fs::remove_file(&path).ok();

        assert!(opened.is_some());
        assert!(unchanged.is_ok());
        assert!(changed.unwrap_err().contains("changed by another program"));
        // Without a recorded stamp, saving is not blocked.
        assert!(ensure_unchanged_since_opened(None, path_text).is_ok());
    }
}
