use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageId(String);

impl StorageId {
    pub fn new(value: impl Into<String>) -> Result<Self, StorageIdError> {
        let value = value.into();
        let trimmed = value.trim();

        if trimmed.is_empty() {
            return Err(StorageIdError::Empty);
        }

        if trimmed != value {
            return Err(StorageIdError::Invalid(value));
        }

        if !trimmed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(StorageIdError::Invalid(value));
        }

        Ok(Self(value))
    }

    pub fn main() -> Self {
        Self("main".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StorageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StorageIdError {
    #[error("storage id must not be empty")]
    Empty,
    #[error("invalid storage id {0:?}; expected only ASCII letters, numbers, '.', '_', or '-'")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathObjectKind {
    Nar,
    Listing,
    Log,
    Realisation,
}

impl PathObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nar => "nar",
            Self::Listing => "listing",
            Self::Log => "log",
            Self::Realisation => "realisation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StorageRef {
    DepotObject {
        storage_id: String,
        object_path: String,
    },
    Upstream {
        upstream_id: Uuid,
        object_path: String,
    },
    CasManifest {
        storage_id: String,
        manifest_id: String,
    },
}
