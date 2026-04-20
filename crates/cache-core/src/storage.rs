use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalBackendName(String);

impl LocalBackendName {
    pub fn new(value: impl Into<String>) -> Result<Self, LocalBackendNameError> {
        let value = value.into();

        if value.trim().is_empty() {
            return Err(LocalBackendNameError::Empty);
        }

        if value.contains('/') || value.contains('\\') {
            return Err(LocalBackendNameError::Invalid(value));
        }

        Ok(Self(value))
    }

    pub fn fs() -> Self {
        Self("fs".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LocalBackendName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocalBackendNameError {
    #[error("local backend name must not be empty")]
    Empty,
    #[error("invalid local backend name {0}")]
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
    LocalBlob {
        backend: String,
        key: String,
    },
    Upstream {
        upstream_id: Uuid,
        object_path: String,
    },
    CasManifest {
        backend: String,
        manifest_id: String,
    },
}
