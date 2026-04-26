use cache_store::local::UploadReader;
use tokio::io::AsyncWriteExt as _;

pub fn duplex_reader(bytes: impl Into<Vec<u8>>) -> UploadReader {
    let bytes = bytes.into();
    let (mut writer, reader) = tokio::io::duplex(bytes.len().max(1));

    tokio::spawn(async move {
        writer.write_all(&bytes).await.unwrap();
        writer.shutdown().await.unwrap();
    });

    Box::pin(reader)
}
