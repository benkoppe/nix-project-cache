#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarInfoRecord {
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub nar_hash: String,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub signatures: Vec<String>,
    pub ca: Option<String>,
}
