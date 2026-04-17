use std::fmt::Write as _;

use crate::nix::{NixContentAddress, NixHash, NixHashError, NormalizedNarHash, StoreDir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarInfo {
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub nar_hash: NixHash,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub signatures: Vec<String>,
    pub ca: Option<NixContentAddress>,
}

impl NarInfo {
    pub fn normalized_nar_hash(&self) -> Result<NormalizedNarHash, NixHashError> {
        self.nar_hash.normalize()
    }

    pub fn ca_narinfo_string(&self) -> Option<String> {
        self.ca.as_ref().map(NixContentAddress::format_for_narinfo)
    }

    pub fn sorted_references(&self) -> Vec<&str> {
        let mut refs = self
            .references
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        refs.sort_unstable();
        refs
    }

    pub fn sorted_signatures(&self) -> Vec<&str> {
        let mut sigs = self
            .signatures
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        sigs.sort_unstable();
        sigs
    }
}

#[derive(Debug, Clone)]
pub struct NarInfoRenderer {
    store_dir: StoreDir,
}

impl NarInfoRenderer {
    pub fn new(store_dir: StoreDir) -> Self {
        Self { store_dir }
    }

    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    pub fn render(&self, narinfo: &NarInfo) -> Result<String, NixHashError> {
        self.render_with_signatures(narinfo, &narinfo.signatures)
    }

    pub fn render_with_signatures(
        &self,
        narinfo: &NarInfo,
        signatures: &[String],
    ) -> Result<String, NixHashError> {
        let nar_hash = narinfo.normalized_nar_hash()?;
        let ca = narinfo.ca_narinfo_string();

        let mut out = String::new();

        writeln!(&mut out, "StorePath: {}", narinfo.store_path).unwrap();
        writeln!(&mut out, "URL: {}", narinfo.url).unwrap();
        writeln!(&mut out, "Compression: {}", narinfo.compression).unwrap();
        writeln!(&mut out, "NarHash: {}", nar_hash).unwrap();
        writeln!(&mut out, "NarSize: {}", narinfo.nar_size).unwrap();

        out.push_str("References:");
        let refs = narinfo.sorted_references();
        if refs.is_empty() {
            out.push(' ')
        } else {
            for reference in refs {
                out.push(' ');
                out.push_str(self.store_dir.display_path(reference));
            }
        }
        out.push('\n');

        if let Some(deriver) = &narinfo.deriver {
            writeln!(
                &mut out,
                "Deriver: {}",
                self.store_dir.display_path(deriver)
            )
            .unwrap();
        }

        if !signatures.is_empty() {
            let mut sorted_sigs = signatures.iter().map(String::as_str).collect::<Vec<_>>();
            sorted_sigs.sort_unstable();

            for signature in sorted_sigs {
                writeln!(&mut out, "Sig: {}", signature).unwrap();
            }
        }

        if let Some(ca) = ca {
            writeln!(&mut out, "CA: {}", ca).unwrap();
        }

        Ok(out)
    }

    pub fn compress(&self, narinfo: &NarInfo) -> Result<Vec<u8>, NarInfoCompressionError> {
        let rendered = self.render(narinfo)?;
        compress_narinfo(&rendered)
    }

    pub fn compress_with_signatures(
        &self,
        narinfo: &NarInfo,
        signatures: &[String],
    ) -> Result<Vec<u8>, NarInfoCompressionError> {
        let rendered = self.render_with_signatures(narinfo, signatures)?;
        compress_narinfo(&rendered)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NarInfoCompressionError {
    #[error("failed to render narinfo: {0}")]
    Render(#[from] NixHashError),
    #[error("failed to compress narinfo: {0}")]
    Compress(#[from] std::io::Error),
}

fn compress_narinfo(content: &str) -> Result<Vec<u8>, NarInfoCompressionError> {
    Ok(zstd::stream::encode_all(content.as_bytes(), 0)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nix::{
        ContentAddressMethod, HashAlgorithm, HashTextEncoding, NixContentAddress, StoreDir,
    };

    fn sample_narinfo() -> NarInfo {
        NarInfo {
            store_path: "/nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1".to_owned(),
            url: "nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst".to_owned(),
            compression: "zstd".to_owned(),
            nar_hash: NixHash::Raw(
                "sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned(),
            ),
            nar_size: 226560,
            references: vec![],
            deriver: None,
            signatures: vec![],
            ca: None,
        }
    }

    fn sample_renderer() -> NarInfoRenderer {
        NarInfoRenderer::new(StoreDir::default())
    }

    #[test]
    fn render_formats_basic_fields() {
        let narinfo = sample_narinfo();
        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(
            rendered
                .contains("StorePath: /nix/store/26xbg1ndr7hbcncrlf9nhx5is2b25d13-hello-2.12.1\n")
        );
        assert!(
            rendered.contains(
                "URL: nar/020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz.nar.zst\n"
            )
        );
        assert!(rendered.contains("Compression: zstd\n"));
        assert!(
            rendered
                .contains("NarHash: sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz\n")
        );
        assert!(rendered.contains("NarSize: 226560\n"));
    }

    #[test]
    fn render_sorts_references_and_strips_store_prefix() {
        let mut narinfo = sample_narinfo();
        narinfo.references = vec![
            "/nix/store/zzz-package".to_owned(),
            "/nix/store/aaa-package".to_owned(),
            "/nix/store/mmm-package".to_owned(),
        ];

        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(rendered.contains("References: aaa-package mmm-package zzz-package\n"));
    }

    #[test]
    fn render_keeps_trailing_space_for_empty_references() {
        let narinfo = sample_narinfo();
        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(rendered.contains("References: \n"));
    }

    #[test]
    fn render_strips_store_prefix_from_deriver() {
        let mut narinfo = sample_narinfo();
        narinfo.deriver = Some("/nix/store/abcdefghijklmnopqrstuvwx-source.drv".to_owned());

        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(rendered.contains("Deriver: abcdefghijklmnopqrstuvwx-source.drv\n"));
    }

    #[test]
    fn render_sorts_embedded_signatures() {
        let mut narinfo = sample_narinfo();
        narinfo.signatures = vec!["cache-b:bbbb".to_owned(), "cache-a:aaaa".to_owned()];

        let rendered = sample_renderer().render(&narinfo).unwrap();

        let a_pos = rendered.find("Sig: cache-a:aaaa\n").unwrap();
        let b_pos = rendered.find("Sig: cache-b:bbbb\n").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn render_with_signatures_uses_passed_signatures() {
        let narinfo = sample_narinfo();
        let rendered = sample_renderer()
            .render_with_signatures(
                &narinfo,
                &["cache-b:bbbb".to_owned(), "cache-a:aaaa".to_owned()],
            )
            .unwrap();

        let a_pos = rendered.find("Sig: cache-a:aaaa\n").unwrap();
        let b_pos = rendered.find("Sig: cache-b:bbbb\n").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn render_includes_ca_field() {
        let mut narinfo = sample_narinfo();
        narinfo.ca = Some(NixContentAddress::Structured {
            method: ContentAddressMethod::Nar,
            hash: NixHash::Raw("sha256-n4bQgYhMfWWaL+qgxVrQFaO/TxsrC4Is0V1sFbDwCgg=".to_owned()),
        });

        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(
            rendered.contains(
                "CA: fixed:r:sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz\n"
            )
        );
    }

    #[test]
    fn render_omits_sig_lines_when_no_signatures_are_present() {
        let narinfo = sample_narinfo();
        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(!rendered.contains("\nSig: "));
    }

    #[test]
    fn render_structured_nix32_hash_uses_direct_nix32_rendering() {
        let mut narinfo = sample_narinfo();
        narinfo.nar_hash = NixHash::Structured {
            algorithm: HashAlgorithm::Sha256,
            encoding: Some(HashTextEncoding::NixBase32),
            digest: "020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz".to_owned(),
        };

        let rendered = sample_renderer().render(&narinfo).unwrap();

        assert!(
            rendered
                .contains("NarHash: sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz\n")
        );
    }

    #[test]
    fn render_with_signatures_sorts_supplied_signatures() {
        let narinfo = sample_narinfo();
        let rendered = sample_renderer()
            .render_with_signatures(
                &narinfo,
                &["cache-b:bbbb".to_owned(), "cache-a:aaaa".to_owned()],
            )
            .unwrap();

        let a_pos = rendered.find("Sig: cache-a:aaaa\n").unwrap();
        let b_pos = rendered.find("Sig: cache-b:bbbb\n").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn compress_narinfo_produces_non_empty_output() {
        let narinfo = sample_narinfo();
        let compressed = sample_renderer().compress(&narinfo).unwrap();

        assert!(!compressed.is_empty());
    }

    #[test]
    fn renderer_uses_custom_store_dir_when_trimming_paths() {
        let mut narinfo = sample_narinfo();
        narinfo.references = vec!["/custom/store/aaa-package".to_owned()];
        narinfo.deriver = Some("/custom/store/example.drv".to_owned());

        let renderer = NarInfoRenderer::new(StoreDir::new("/custom/store").unwrap());
        let rendered = renderer.render(&narinfo).unwrap();

        assert!(rendered.contains("References: aaa-package\n"));
        assert!(rendered.contains("Deriver: example.drv\n"));
    }

    #[test]
    fn normalized_nar_hash_returns_typed_normalized_value() {
        let narinfo = sample_narinfo();

        let nar_hash = narinfo.normalized_nar_hash().unwrap();

        assert_eq!(nar_hash.algorithm(), &HashAlgorithm::Sha256);
        assert_eq!(
            nar_hash.to_string(),
            "sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz"
        );
    }
}
