use async_trait::async_trait;
use futures::Future;
use kv_log_macro::{debug, info};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use url::Url;

use std::sync::Arc;

use crate::{errors::TorrentError, torrent::Result};

async fn peers_from_dict(peers: &[Peer], addrs: &mut Vec<SocketAddr>) {
    for peer in peers {
        if let Ok(s_addrs) = tokio::net::lookup_host((peer.ip.as_str(), peer.port)).await {
            for addr in s_addrs {
                addrs.push(addr);
            }
        }
    }
}

pub(super) async fn get_peers_addrs(response: &AnnounceResponse) -> Vec<SocketAddr> {
    let mut addrs = Vec::new();

    match response.peers6 {
        Some(Peers6::Binary(ref bin)) => {
            addrs.reserve(bin.len() / 18);
            crate::utils::ipv6_from_slice(bin, &mut addrs);
        }
        Some(Peers6::Dict(ref peers)) => {
            addrs.reserve(peers.len());
            peers_from_dict(peers, &mut addrs).await;
        }
        None => {}
    }

    match response.peers {
        Some(Peers::Binary(ref bin)) => {
            addrs.reserve(bin.len() / 6);
            crate::utils::ipv4_from_slice(bin, &mut addrs);
        }
        Some(Peers::Dict(ref peers)) => {
            addrs.reserve(peers.len());
            peers_from_dict(peers, &mut addrs).await;
        }
        None => {}
    }

    debug!("[http tracker] peers found: {:#?}", addrs);
    addrs
}

#[derive(Serialize, Debug)]
pub struct AnnounceQuery<'a> {
    pub info_hash: &'a [u8],
    pub peer_id: &'a str,
    pub port: i64,
    pub uploaded: i64,
    pub downloaded: i64,
    //left: i64,
    pub event: String,
    pub compact: i64,
}

impl<'a> From<&'a TrackerData> for AnnounceQuery<'a> {
    fn from(data: &'a TrackerData) -> AnnounceQuery {
        AnnounceQuery {
            info_hash: data.metadata.info_hash.as_ref(),
            peer_id: std::str::from_utf8(&**data.extern_id)
                .expect("Fail to convert extern id to str"),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            event: "started".to_owned(),
            compact: 1,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct Peer {
    pub ip: String,
    pub port: u16,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Peers {
    Dict(Vec<Peer>),
    #[serde(with = "serde_bytes")]
    Binary(Vec<u8>),
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Peers6 {
    Dict(Vec<Peer>),
    #[serde(with = "serde_bytes")]
    Binary(Vec<u8>),
}

#[derive(Deserialize, Debug)]
pub struct AnnounceResponse {
    #[serde(rename = "warning message")]
    pub warning_message: Option<String>,
    pub interval: i64,
    #[serde(rename = "min interval")]
    pub min_interval: Option<i64>,
    #[serde(rename = "tracker id")]
    pub tracker_id: Option<String>,
    pub complete: Option<i64>,
    pub incomplete: Option<i64>,
    pub downloaded: Option<i64>,
    pub peers: Option<Peers>,
    pub peers6: Option<Peers6>,
}

use crate::bencode::de::{from_bytes, DeserializeError};

#[derive(Debug)]
pub enum HttpError {
    ResponseCode(String),
    Malformed,
    MissingContentLength,
    Deserialize(DeserializeError),
    HostResolution,
    IO(std::io::Error),
    IOAsync(tokio::io::Error),
}

impl From<std::io::Error> for HttpError {
    fn from(e: std::io::Error) -> HttpError {
        HttpError::IO(e)
    }
}

impl From<DeserializeError> for HttpError {
    fn from(e: DeserializeError) -> HttpError {
        HttpError::Deserialize(e)
    }
}

pub trait Escaped {
    fn escape(&self) -> String;
}

impl<T> Escaped for T
where
    T: AsRef<[u8]>,
{
    fn escape(&self) -> String {
        escape_str(self)
    }
}

pub trait ToQuery {
    fn to_query(&self) -> String;
}

impl<'a> ToQuery for AnnounceQuery<'a> {
    fn to_query(&self) -> String {
        format!(
            "info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&event={}&left={}&compact={}",
            self.info_hash.escape(),
            self.peer_id.escape(),
            self.port,
            self.uploaded,
            self.downloaded,
            self.event,
            0, // left
            self.compact,
        )
    }
}

const UNRESERVED_CHAR: &[u8] =
	//"%+;?:@=&,$/"
    b"-_!.~*()ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

const HEXCHARS: &[u8] = b"0123456789abcdef";

pub fn escape_str<T: AsRef<[u8]>>(s: T) -> String {
    let bytes = s.as_ref();

    let mut result = Vec::with_capacity(bytes.len() * 3);

    for b in bytes {
        if memchr(*b, UNRESERVED_CHAR).is_some() {
            result.push(*b);
        } else {
            result.push(b'%');
            result.push(HEXCHARS[(b >> 4) as usize]);
            result.push(HEXCHARS[(b & 0xF) as usize]);
        }
    }

    String::from_utf8(result).unwrap()
}

const DEFAULT_HEADERS: &str = "User-Agent: rustorrent/0.1\r\nConnection: close";

pub(super) fn format_host(url: &Url) -> String {
    if let Some(port) = url.port() {
        format!("Host: {}:{}", url.host_str().unwrap(), port)
    } else {
        format!("Host: {}", url.host_str().unwrap())
    }
}

pub(super) fn format_request<T: ToQuery>(url: &Url, query: &T) -> String {
    format!(
        "GET {}?{} HTTP/1.1\r\n{}\r\n{}\r\n\r\n",
        url.path(),
        query.to_query(),
        format_host(url),
        DEFAULT_HEADERS
    )
}

use crate::utils::ConnectTimeout;
use std::time::Duration;

pub(super) async fn send_recv<T, S, Q>(mut stream: S, url: &Url, query: &Q) -> Result<T>
where
    T: DeserializeOwned,
    S: AsyncRead + AsyncWrite + Unpin,
    Q: ToQuery,
{
    let req = format_request(url, query);

    debug!("[http tracker] ", { request: req });

    stream.write(req.as_bytes()).await?;
    stream.flush().await?;

    let response = read_response(stream).await?;

    // println!("DATA {:x?}", String::from_utf8_lossy(&response));

    let value = from_bytes(&response)?;

    Ok(value)
}

use memchr::memchr;
use tokio::io::BufReader;

use super::{connection::TrackerConnection, supervisor::TrackerData};

pub(super) async fn read_response<T>(stream: T) -> Result<Vec<u8>>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut reader = BufReader::with_capacity(4 * 1024, stream);

    let mut content_length = None;

    // String containing headers
    let mut string = String::with_capacity(1024);

    // First line with HTTP code
    // TODO: Check code
    reader.read_line(&mut string).await?;

    loop {
        string.clear();
        reader.read_line(&mut string).await?;

        if string == "\r\n" {
            break; // End of headers
        }

        let index = match memchr(b':', string.as_bytes()) {
            Some(index) => index,
            _ => return Err(HttpError::Malformed.into()),
        };

        let name = &string[..index].trim().to_lowercase();
        let value = &string[index + 1..].trim();

        if name == "content-length" {
            content_length = Some(value.parse().map_err(|_| HttpError::Malformed)?);
        }
    }

    let content_length = match content_length {
        Some(content_length) => content_length,
        _ => return Err(HttpError::MissingContentLength.into()),
    };

    let mut buffer = Vec::with_capacity(content_length as usize);

    reader.take(content_length).read_to_end(&mut buffer).await?;

    Ok(buffer)
}

pub async fn http_get<R, Q>(url: &Url, query: &Q, addr: &SocketAddr) -> Result<R>
where
    Q: ToQuery,
    R: DeserializeOwned,
{
    info!(
        "[http tracker]", {
            url: url.to_string(),
            host: url.host().map(|h| h.to_string()),
            port: url.port(),
            scheme: url.scheme()
        }
    );

    let stream = TcpStream::connect_timeout(addr, Duration::from_secs(5)).await?;

    send_recv(stream, url, query).await
}

pub(super) async fn announce_http<'a, 'b, F, O>(
    addrs: &'a [Arc<SocketAddr>],
    connected_addr: &'b mut usize,
    fun_get: F,
) -> Result<Vec<SocketAddr>>
where
    F: Fn(&'a SocketAddr) -> O,
    O: Future<Output = Result<AnnounceResponse>> + Sized,
{
    let mut last_err = None;
    for (index, addr) in addrs.iter().enumerate() {
        let response = match fun_get(addr).await {
            Ok(resp) => resp,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        *connected_addr = index;
        let peers = get_peers_addrs(&response).await;
        return Ok(peers);
    }
    match last_err {
        Some(e) => Err(e),
        _ => Err(TorrentError::Unresponsive),
    }
}

pub struct HttpConnection {
    data: Arc<TrackerData>,
    addr: Vec<Arc<SocketAddr>>,
}

#[async_trait]
impl TrackerConnection for HttpConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> Result<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(self.data.as_ref());

        announce_http(&self.addr, connected_addr, |addr| {
            http_get(&self.data.url, &query, addr)
        })
        .await
    }

    async fn scrape(&mut self) -> Result<()> {
        Ok(())
    }
}

impl HttpConnection {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        data: Arc<TrackerData>,
        addr: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync> {
        Box::new(Self { data, addr })
    }
}
