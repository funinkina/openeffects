use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Info,
}

impl Status {
    pub fn tag(&self) -> &'static str {
        match self {
            Status::Ok => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
            Status::Info => "??  ",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub group: String,
    pub name: String,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Check {
    pub fn new(
        group: impl Into<String>,
        name: impl Into<String>,
        status: Status,
        detail: impl Into<String>,
        fix: Option<String>,
    ) -> Self {
        Self {
            group: group.into(),
            name: name.into(),
            status,
            detail: detail.into(),
            fix,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CameraClass {
    Rgb,
    Ir,
    Metadata,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraNode {
    pub path: String,
    pub name: String,
    pub class: CameraClass,
    pub formats: Vec<String>,
}
