use std::sync::LazyLock;

use regex::Regex;

use crate::nix::{HashAlgorithm, NIX_BASE32_ALPHABET, StorePathHash};

fn sha256_nix_base32_digest_len() -> usize {
    HashAlgorithm::Sha256
        .nix_base32_digest_len()
        .expect("sha256 nix-base32 digest length should exist")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarCompression {
    Uncompressed,
    Zstd,
    Xz,
    Bz2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheObjectPath {
    NixCacheInfo,
    IndexHtml,
    NarInfo {
        store_path_hash: StorePathHash,
    },
    Nar {
        nar_hash_digest: String,
        compression: NarCompression,
    },
    Listing {
        store_path_hash: StorePathHash,
    },
    Log {
        path: String,
    },
    Realisation {
        path: String,
    },
}

static NARINFO_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alphabet = std::str::from_utf8(NIX_BASE32_ALPHABET).unwrap();
    Regex::new(&format!(
        r"^[{alphabet}]{{{}}}\.narinfo$",
        StorePathHash::LENGTH
    ))
    .unwrap()
});

static NAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alphabet = std::str::from_utf8(NIX_BASE32_ALPHABET).unwrap();
    Regex::new(&format!(
        r"^nar/([{alphabet}]{{{}}})\.nar(\.zst|\.xz|\.bz2)?$",
        sha256_nix_base32_digest_len()
    ))
    .unwrap()
});

static LS_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alphabet = std::str::from_utf8(NIX_BASE32_ALPHABET).unwrap();
    Regex::new(&format!(r"^[{alphabet}]{{{}}}\.ls$", StorePathHash::LENGTH)).unwrap()
});

static LOG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^log/[a-zA-Z0-9._-]+\.drv$").unwrap());

static REALISATION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^realisations/[a-z0-9]+:[a-zA-Z0-9+/=]+![a-zA-Z0-9_-]+\.doi$").unwrap()
});

pub fn parse_cache_object_path(path: &str) -> Option<CacheObjectPath> {
    if path.is_empty() || path.starts_with('/') || path.contains("..") {
        return None;
    }

    match path {
        "nix-cache-info" => return Some(CacheObjectPath::NixCacheInfo),
        "index.html" => return Some(CacheObjectPath::IndexHtml),
        _ => {}
    }

    if NARINFO_RE.is_match(path) {
        let hash_text = path.strip_suffix(".narinfo")?;
        let store_path_hash = parse_store_path_hash_text(hash_text)?;
        return Some(CacheObjectPath::NarInfo { store_path_hash });
    }

    if let Some(captures) = NAR_RE.captures(path) {
        let nar_hash = captures.get(1)?.as_str().to_owned();
        let compression = match captures.get(2).map(|value| value.as_str()) {
            None => NarCompression::Uncompressed,
            Some(".zst") => NarCompression::Zstd,
            Some(".xz") => NarCompression::Xz,
            Some(".bz2") => NarCompression::Bz2,
            Some(_) => return None,
        };

        return Some(CacheObjectPath::Nar {
            nar_hash_digest: nar_hash,
            compression,
        });
    }

    if LS_RE.is_match(path) {
        let hash_text = path.strip_suffix(".ls")?;
        let store_path_hash = parse_store_path_hash_text(hash_text)?;
        return Some(CacheObjectPath::Listing { store_path_hash });
    }

    if LOG_RE.is_match(path) {
        return Some(CacheObjectPath::Log {
            path: path.to_owned(),
        });
    }

    if REALISATION_RE.is_match(path) {
        return Some(CacheObjectPath::Realisation {
            path: path.to_owned(),
        });
    }

    None
}

pub fn is_valid_cache_path(path: &str) -> bool {
    parse_cache_object_path(path).is_some()
}

fn parse_store_path_hash_text(hash_text: &str) -> Option<StorePathHash> {
    StorePathHash::parse_from_store_path(&format!("{hash_text}-x")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_path_hash(hash_text: &str) -> StorePathHash {
        StorePathHash::parse_from_store_path(&format!("{hash_text}-x")).unwrap()
    }

    #[test]
    fn valid_cache_paths_match_go_baseline() {
        let cases = [
            "26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo",
            "0123456789abcdfghijklmnpqrsvwxyz.narinfo",
            "nar/1ngi2dxw1f7khrrjamzkkdai393lwcm8s78gvs1ag8k3n82w7bvp.nar.zst",
            "nar/1ngi2dxw1f7khrrjamzkkdai393lwcm8s78gvs1ag8k3n82w7bvp.nar.xz",
            "nar/1ngi2dxw1f7khrrjamzkkdai393lwcm8s78gvs1ag8k3n82w7bvp.nar.bz2",
            "nar/1ngi2dxw1f7khrrjamzkkdai393lwcm8s78gvs1ag8k3n82w7bvp.nar",
            "26xbg1ndr7hbcncrlf9nhx5is2b25d13.ls",
            "log/k3b2gg5n0p2q8r9t1v4w6x7y-my-package-1.0.drv",
            "realisations/sha256:abc123def456!out.doi",
            "nix-cache-info",
            "index.html",
        ];

        for case in cases {
            assert!(is_valid_cache_path(case), "{case} should be valid");
        }
    }

    #[test]
    fn invalid_cache_paths_match_go_baseline() {
        let cases = [
            "../etc/passwd",
            "nar/../../../etc/passwd",
            "26xbg1ndr7hbcncrlf9nhx5is2b25e13.narinfo",
            "26xbg1ndr7hbcncrlf9nhx5is2b25u13.narinfo",
            "foo/bar/baz",
            "",
            "/26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo",
            "26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo.bak",
            "abc.narinfo",
        ];

        for case in cases {
            assert!(!is_valid_cache_path(case), "{case} should be invalid");
        }
    }

    #[test]
    fn parses_narinfo_object_path() {
        let parsed = parse_cache_object_path("26xbg1ndr7hbcncrlf9nhx5is2b25d13.narinfo");

        assert_eq!(
            parsed,
            Some(CacheObjectPath::NarInfo {
                store_path_hash: store_path_hash("26xbg1ndr7hbcncrlf9nhx5is2b25d13"),
            })
        );
    }

    #[test]
    fn parses_nar_object_path() {
        let parsed = parse_cache_object_path(
            "nar/1ngi2dxw1f7khrrjamzkkdai393lwcm8s78gvs1ag8k3n82w7bvp.nar.zst",
        );

        assert_eq!(
            parsed,
            Some(CacheObjectPath::Nar {
                nar_hash_digest: "1ngi2dxw1f7khrrjamzkkdai393lwcm8s78gvs1ag8k3n82w7bvp".to_owned(),
                compression: NarCompression::Zstd,
            })
        );
    }

    #[test]
    fn parses_listing_object_path() {
        let parsed = parse_cache_object_path("26xbg1ndr7hbcncrlf9nhx5is2b25d13.ls");

        assert_eq!(
            parsed,
            Some(CacheObjectPath::Listing {
                store_path_hash: store_path_hash("26xbg1ndr7hbcncrlf9nhx5is2b25d13"),
            })
        );
    }
}
