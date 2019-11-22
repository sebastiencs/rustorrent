
use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::de::DeserializeError;
use crate::http_client::HttpError;

#[derive(Debug)]
pub enum TorrentError {
    Deserialization(DeserializeError),
    InvalidInput,
    Http(HttpError),
    IO(std::io::Error),
    IOAsync(async_std::io::Error)
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

impl From<async_std::io::Error> for TorrentError {
    fn from(e: async_std::io::Error) -> TorrentError {
        TorrentError::IOAsync(e)
    }
}

impl From<DeserializeError> for TorrentError {
    fn from(e: DeserializeError) -> TorrentError {
        TorrentError::Deserialization(e)
    }
}
