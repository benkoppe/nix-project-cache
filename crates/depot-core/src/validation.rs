use thiserror::Error;

use crate::cache_path::{CacheObjectPath, NarCompression, parse_cache_object_path};
use crate::narinfo::NarInfo;
use crate::nix::{NixHashError, StoreDir, StorePathHash, StorePathHashError};

#[derive(Debug, Error)]
pub enum PublishNarInfoValidationError {
    #[error("store path {path} is not inside store dir {store_dir}")]
    StorePathOutsideStoreDir { store_dir: String, path: String },

    #[error("invalid store path {path}: {source}")]
    InvalidStorePathHash {
        path: String,
        source: StorePathHashError,
    },

    #[error("URL {url} is not a valid cache object path")]
    InvalidCacheObjectPath { url: String },

    #[error("URL {url} must point to a NAR object")]
    UrlIsNotNarObject { url: String },

    #[error("unsupported narinfo compression {compression}")]
    UnsupportedCompression { compression: String },

    #[error(
        "narinfo compression {narinfo_compression} does not match URL compression {url_compression}"
    )]
    CompressionMismatch {
        narinfo_compression: String,
        url_compression: &'static str,
    },

    #[error("invalid NAR hash for {store_path}: {source}")]
    InvalidNarHash {
        store_path: String,
        source: NixHashError,
    },

    #[error("URL {actual_url} does not match expected URL {expected_url}")]
    UrlDoesNotMatchNarHash {
        actual_url: String,
        expected_url: String,
    },

    #[error("reference {path} is not inside store dir {store_dir}")]
    ReferenceOutsideStoreDir { store_dir: String, path: String },

    #[error("invalid reference store path {path}: {source}")]
    InvalidReferenceStorePath {
        path: String,
        source: StorePathHashError,
    },

    #[error("deriver {path} is not inside store dir {store_dir}")]
    DeriverOutsideStoreDir { store_dir: String, path: String },

    #[error("invalid deriver store path {path}: {source}")]
    InvalidDeriverStorePath {
        path: String,
        source: StorePathHashError,
    },
}

pub fn validate_publish_narinfo(
    store_dir: &StoreDir,
    narinfo: &NarInfo,
) -> Result<StorePathHash, PublishNarInfoValidationError> {
    validate_store_path_inside_store_dir(store_dir, &narinfo.store_path)?;

    let store_path_hash =
        StorePathHash::parse_from_store_path(&narinfo.store_path).map_err(|source| {
            PublishNarInfoValidationError::InvalidStorePathHash {
                path: narinfo.store_path.clone(),
                source,
            }
        })?;

    let (url_nar_hash_digest, url_compression) = parse_nar_url(&narinfo.url)?;

    let narinfo_compression = NarCompression::from_narinfo_compression(&narinfo.compression)
        .ok_or_else(|| PublishNarInfoValidationError::UnsupportedCompression {
            compression: narinfo.compression.clone(),
        })?;

    if narinfo_compression != url_compression {
        return Err(PublishNarInfoValidationError::CompressionMismatch {
            narinfo_compression: narinfo.compression.clone(),
            url_compression: url_compression.narinfo_compression(),
        });
    }

    let normalized_nar_hash = narinfo.normalized_nar_hash().map_err(|source| {
        PublishNarInfoValidationError::InvalidNarHash {
            store_path: narinfo.store_path.clone(),
            source,
        }
    })?;

    let expected_url = format!(
        "nar/{}{}",
        normalized_nar_hash.digest(),
        narinfo_compression.file_suffix()
    );

    if narinfo.url != expected_url || url_nar_hash_digest != normalized_nar_hash.digest() {
        return Err(PublishNarInfoValidationError::UrlDoesNotMatchNarHash {
            actual_url: narinfo.url.clone(),
            expected_url,
        });
    }

    for reference in &narinfo.references {
        validate_reference(store_dir, reference)?;
    }

    if let Some(deriver) = &narinfo.deriver {
        validate_deriver(store_dir, deriver)?;
    }

    Ok(store_path_hash)
}

fn parse_nar_url(url: &str) -> Result<(String, NarCompression), PublishNarInfoValidationError> {
    match parse_cache_object_path(url) {
        Some(CacheObjectPath::Nar {
            nar_hash_digest,
            compression,
        }) => Ok((nar_hash_digest, compression)),
        Some(_) => Err(PublishNarInfoValidationError::UrlIsNotNarObject {
            url: url.to_owned(),
        }),
        None => Err(PublishNarInfoValidationError::InvalidCacheObjectPath {
            url: url.to_owned(),
        }),
    }
}

fn validate_store_path_inside_store_dir(
    store_dir: &StoreDir,
    path: &str,
) -> Result<(), PublishNarInfoValidationError> {
    if store_dir.contains_path(path) {
        Ok(())
    } else {
        Err(PublishNarInfoValidationError::StorePathOutsideStoreDir {
            store_dir: store_dir.to_string(),
            path: path.to_owned(),
        })
    }
}

fn validate_reference(
    store_dir: &StoreDir,
    reference: &str,
) -> Result<(), PublishNarInfoValidationError> {
    if !store_dir.contains_path(reference) {
        return Err(PublishNarInfoValidationError::ReferenceOutsideStoreDir {
            store_dir: store_dir.to_string(),
            path: reference.to_owned(),
        });
    }

    StorePathHash::parse_from_store_path(reference).map_err(|source| {
        PublishNarInfoValidationError::InvalidReferenceStorePath {
            path: reference.to_owned(),
            source,
        }
    })?;

    Ok(())
}

fn validate_deriver(
    store_dir: &StoreDir,
    deriver: &str,
) -> Result<(), PublishNarInfoValidationError> {
    if !store_dir.contains_path(deriver) {
        return Err(PublishNarInfoValidationError::DeriverOutsideStoreDir {
            store_dir: store_dir.to_string(),
            path: deriver.to_owned(),
        });
    }

    StorePathHash::parse_from_store_path(deriver).map_err(|source| {
        PublishNarInfoValidationError::InvalidDeriverStorePath {
            path: deriver.to_owned(),
            source,
        }
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nix::NixHash;

    fn sample_narinfo() -> NarInfo {
        NarInfo {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            ),
            nar_size: 226560,
            references: Vec::new(),
            deriver: None,
            signatures: Vec::new(),
            ca: None,
        }
    }

    #[test]
    fn validates_publishable_narinfo() {
        let narinfo = sample_narinfo();

        let hash = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap();

        assert_eq!(hash.as_str(), "26xbg1ndr7hbcncrlf9nhx5is2b25d13");
    }

    #[test]
    fn rejects_store_path_outside_store_dir() {
        let mut narinfo = sample_narinfo();
        narinfo.store_path = "/tmp/not-in-store".to_owned();

        let error = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap_err();

        assert!(matches!(
            error,
            PublishNarInfoValidationError::StorePathOutsideStoreDir { .. }
        ));
    }

    #[test]
    fn rejects_non_nar_url() {
        let mut narinfo = sample_narinfo();
        narinfo.url = "26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo".to_owned();

        let error = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap_err();

        assert!(matches!(
            error,
            PublishNarInfoValidationError::UrlIsNotNarObject { .. }
        ));
    }

    #[test]
    fn rejects_compression_mismatch() {
        let mut narinfo = sample_narinfo();
        narinfo.compression = "xz".to_owned();

        let error = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap_err();

        assert!(matches!(
            error,
            PublishNarInfoValidationError::CompressionMismatch { .. }
        ));
    }

    #[test]
    fn rejects_url_that_does_not_match_nar_hash() {
        let mut narinfo = sample_narinfo();
        narinfo.url = "nar/1111111111111111111111111111111111111111111111111111.nar.zst".to_owned();

        let error = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap_err();

        assert!(matches!(
            error,
            PublishNarInfoValidationError::UrlDoesNotMatchNarHash { .. }
        ));
    }

    #[test]
    fn rejects_reference_outside_store_dir() {
        let mut narinfo = sample_narinfo();
        narinfo.references = vec!["/tmp/not-in-store".to_owned()];

        let error = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap_err();

        assert!(matches!(
            error,
            PublishNarInfoValidationError::ReferenceOutsideStoreDir { .. }
        ));
    }

    #[test]
    fn rejects_deriver_outside_store_dir() {
        let mut narinfo = sample_narinfo();
        narinfo.deriver = Some("/tmp/not-in-store.drv".to_owned());

        let error = validate_publish_narinfo(&StoreDir::default(), &narinfo).unwrap_err();

        assert!(matches!(
            error,
            PublishNarInfoValidationError::DeriverOutsideStoreDir { .. }
        ));
    }
}
