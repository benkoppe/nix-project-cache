use std::fmt::Write as _;

use crate::nix::{NixContentAddress, NixHash, NixHashError};

const NIX_STORE_PREFIX: &str = "/nix/store/";

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
    pub fn nar_hash_nix32(&self) -> Result<String, NixHashError> {
        self.nar_hash.to_nix_base32()
    }

    pub fn ca_narinfo_string(&self) -> Option<String> {
        self.ca.as_ref().map(NixContentAddress::render_for_narinfo)
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

    pub fn render(&self) -> Result<String, NixHashError> {
        let nar_hash = self.nar_hash_nix32()?;
        let ca = self.ca_narinfo_string();

        let mut out = String::new();

        writeln!(&mut out, "StorePath: {}", self.store_path).unwrap();
        writeln!(&mut out, "URL: {}", self.url).unwrap();
        writeln!(&mut out, "Compression: {}", self.compression).unwrap();
        writeln!(&mut out, "NarHash: {}", nar_hash).unwrap();
        writeln!(&mut out, "NarSize: {}", self.nar_size).unwrap();

        out.push_str("References:");
        let refs = self.sorted_references();
        if refs.is_empty() {
            out.push(' ');
        } else {
            for reference in refs {
                out.push(' ');
                out.push_str(trim_store_prefix(reference));
            }
        }
        out.push('\n');

        if let Some(deriver) = &self.deriver {
            writeln!(&mut out, "Deriver: {}", trim_store_prefix(deriver)).unwrap();
        }

        for signature in self.sorted_signatures() {
            writeln!(&mut out, "Sig: {}", signature).unwrap();
        }

        if let Some(ca) = ca {
            writeln!(&mut out, "CA: {}", ca).unwrap();
        }

        Ok(out)
    }
}

fn trim_store_prefix(path: &str) -> &str {
    path.strip_prefix(NIX_STORE_PREFIX).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nix::{ContentAddressMethod, NixContentAddress};
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
    #[test]
    fn render_formats_basic_fields() {
        let narinfo = sample_narinfo();
        let rendered = narinfo.render().unwrap();
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
        let rendered = narinfo.render().unwrap();
        assert!(rendered.contains("References: aaa-package mmm-package zzz-package\n"));
    }
    #[test]
    fn render_keeps_trailing_space_for_empty_references() {
        let narinfo = sample_narinfo();
        let rendered = narinfo.render().unwrap();
        assert!(rendered.contains("References: \n"));
    }
    #[test]
    fn render_strips_store_prefix_from_deriver() {
        let mut narinfo = sample_narinfo();
        narinfo.deriver = Some("/nix/store/abcdefghijklmnopqrstuvwx-source.drv".to_owned());
        let rendered = narinfo.render().unwrap();
        assert!(rendered.contains("Deriver: abcdefghijklmnopqrstuvwx-source.drv\n"));
    }
    #[test]
    fn render_sorts_signatures() {
        let mut narinfo = sample_narinfo();
        narinfo.signatures = vec!["cache-b:bbbb".to_owned(), "cache-a:aaaa".to_owned()];
        let rendered = narinfo.render().unwrap();
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
        let rendered = narinfo.render().unwrap();
        assert!(
            rendered.contains(
                "CA: fixed:r:sha256:020ay2q1av2xs4n842rb3d7vz8qms1dcb87a5yd6azaci20x11lz\n"
            )
        );
    }
}
