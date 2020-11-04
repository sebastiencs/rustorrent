


use crate::bencode::de::DeserializeError;
use crate::actors::tracker::http::HttpError;

#[derive(Debug)]
pub enum TorrentError {
    Deserialization(DeserializeError),
    InvalidInput,
    Http(HttpError),
    Unresponsive,
    IO(std::io::Error),
    IOAsync(tokio::io::Error)
}

impl From<HttpError> for TorrentError {
    fn from(e: HttpError) -> TorrentError {
        match e {
            HttpError::IO(e) => TorrentError::IO(e),
            HttpError::IOAsync(e) => TorrentError::IOAsync(e),
            e => TorrentError::Http(e)
        }
    }
}

impl From<tokio::io::Error> for TorrentError {
    fn from(e: tokio::io::Error) -> TorrentError {
        TorrentError::IOAsync(e)
    }
}

impl From<DeserializeError> for TorrentError {
    fn from(e: DeserializeError) -> TorrentError {
        TorrentError::Deserialization(e)
    }
}
