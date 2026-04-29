use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DepotClientError {
    #[error("invalid server url {url}: {message}")]
    InvalidServerUrl { url: String, message: String },

    #[error("failed to build endpoint url: {message}")]
    InvalidEndpointUrl { message: String },

    #[error("server returned {status}: {body}")]
    UnexpectedStatus { status: StatusCode, body: String },

    #[error("http request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("client upload failed: {message}")]
    ClientUpload { message: String },
}
