use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind")]
pub enum DicomNode {
    Element {
        tag: String,
        vr: String,
        description: String,
        value: String,
        length: u32,
        path: Vec<String>,
        editable: bool,
    },
    Sequence {
        tag: String,
        description: String,
        length: u32,
        path: Vec<String>,
        items: Vec<Vec<DicomNode>>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DicomTagInfo {
    pub tag: String,
    pub vr: String,
    pub description: String,
    pub editable: bool,
}

impl ValidationResult {
    pub fn valid() -> Self {
        Self {
            valid: true,
            message: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            valid: false,
            message: Some(message.into()),
        }
    }
}
