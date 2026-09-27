use serde::{Deserialize, Serialize};

/// One configured library or output root, as advertised to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootInfo {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    /// Path relative to the root, suitable for passing back into the browse
    /// or probe API.
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirListing {
    pub root_id: String,
    pub path: String,
    pub entries: Vec<DirEntry>,
}
