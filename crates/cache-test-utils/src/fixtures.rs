use uuid::Uuid;

use cache_api::NarInfoPayload;
use cache_core::narinfo::{NarInfo, NarInfoRenderer};
use cache_core::nix::{NixHash, StoreDir, StorePathHash};
use cache_core::project::ProjectSlug;
use cache_core::signing::NamedSigningKey;
use cache_store::upstream::UpstreamCache;

pub const EXAMPLE_PROJECT_SLUG: &str = "example_repo";
pub const EXAMPLE_PROJECT_NAME: &str = "Example Repo";

#[derive(Debug, Clone, Copy)]
pub struct SamplePath {
    store_path: &'static str,
    url: &'static str,
    nar_hash: &'static str,
    nar_size: u64,
}

impl SamplePath {
    pub fn store_path(self) -> &'static str {
        self.store_path
    }

    pub fn url(self) -> &'static str {
        self.url
    }

    pub fn hash(self) -> StorePathHash {
        StorePathHash::parse_from_store_path(self.store_path).unwrap()
    }

    pub fn hash_str(self) -> String {
        self.hash().as_str().to_owned()
    }

    pub fn narinfo(self) -> NarInfo {
        NarInfo {
            store_path: self.store_path.to_owned(),
            url: self.url.to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(self.nar_hash.to_owned()),
            nar_size: self.nar_size,
            references: Vec::new(),
            deriver: None,
            signatures: Vec::new(),
            ca: None,
        }
    }

    pub fn payload(self) -> NarInfoPayload {
        NarInfoPayload::from(&self.narinfo())
    }

    pub fn narinfo_text(self, extra_signatures: &[&str]) -> String {
        let mut text = NarInfoRenderer::new(StoreDir::default())
            .render(&self.narinfo())
            .unwrap();

        for signature in extra_signatures {
            text.push_str("Sig: ");
            text.push_str(signature);
            text.push('\n');
        }

        text
    }
}

pub fn example_project() -> ProjectSlug {
    ProjectSlug::parse(EXAMPLE_PROJECT_SLUG).unwrap()
}

pub fn hello_path() -> SamplePath {
    SamplePath {
        store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1",
        url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst",
        nar_hash: "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz",
        nar_size: 226560,
    }
}

pub fn goodbye_path() -> SamplePath {
    SamplePath {
        store_path: "/nix/store/11111111111111111111111111111111-goodbye-1.0",
        url: "nar/1111111111111111111111111111111111111111111111111111.nar.zst",
        nar_hash: "sha256:1111111111111111111111111111111111111111111111111111",
        nar_size: 123,
    }
}

pub fn test_signing_keys() -> Vec<NamedSigningKey> {
    vec![
        NamedSigningKey::parse("cache.example.com-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            .unwrap(),
    ]
}

pub fn sample_upstream(base_url: impl Into<String>) -> UpstreamCache {
    UpstreamCache::new(Uuid::now_v7(), "cache.nixos.org", base_url, 10)
}
