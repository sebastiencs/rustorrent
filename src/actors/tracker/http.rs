use async_std::sync::{Sender, Receiver, channel};
use async_std::net::{SocketAddr, ToSocketAddrs, TcpStream};
use async_trait::async_trait;

use std::sync::Arc;
//use std::io::prelude::*;
//use std::io::Cursor;

use crate::metadata::Torrent;
use crate::supervisors::torrent::Result as TResult;
//use crate::http_client::{self, AnnounceQuery};
//use crate::session::get_peers_addrs;
use crate::errors::TorrentError;

use super::{TrackerConnection, TrackerData};

//use std::net::TcpStream;
// use async_std::net::{SocketAddr, TcpStream};
use async_std::prelude::*;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use url::Url;

pub async fn peers_from_dict(peers: &[Peer], addrs: &mut Vec<SocketAddr>) {
    for peer in peers {
        if let Ok(s_addrs) = (peer.ip.as_str(), peer.port).to_socket_addrs().await {
            for addr in s_addrs {
                addrs.push(addr);
            }
        }
    }
}

pub async fn get_peers_addrs(response: &AnnounceResponse) -> Vec<SocketAddr> {
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

    println!("ADDRS: {:#?}", addrs);
    addrs
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AnnounceQuery<'a> {
    pub info_hash: &'a [u8],
    pub peer_id: String,
    pub port: i64,
    pub uploaded: i64,
    pub downloaded: i64,
    //left: i64,
    pub event: String,
    pub compact: i64
}

impl<'a> From<&'a Torrent> for AnnounceQuery<'a> {
    fn from(torrent: &'a Torrent) -> AnnounceQuery {
        AnnounceQuery {
            info_hash: torrent.info_hash.as_ref(),
            peer_id: "-RT1220sJ1Nna5rzWLd8".to_owned(),
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
    pub port: u16
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Peers {
    Dict(Vec<Peer>),
    #[serde(with = "serde_bytes")]
    Binary(Vec<u8>)
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Peers6 {
    Dict(Vec<Peer>),
    #[serde(with = "serde_bytes")]
    Binary(Vec<u8>)
}

#[derive(Deserialize, Debug)]
pub struct AnnounceResponse {
    #[serde(rename="warning message")]
    pub warning_message: Option<String>,
    pub interval: i64,
    #[serde(rename="min interval")]
    pub min_interval: Option<i64>,
    #[serde(rename="tracker id")]
    pub tracker_id: Option<String>,
    pub complete: i64,
    pub incomplete: i64,
    pub downloaded: Option<i64>,
    pub peers: Option<Peers>,
    pub peers6: Option<Peers6>,
}

use crate::de::{DeserializeError, from_bytes};

#[derive(Debug)]
pub enum HttpError {
    ResponseCode(String),
    Malformed,
    MissingContentLength,
    Deserialize(DeserializeError),
    HostResolution,
    IO(std::io::Error),
    IOAsync(async_std::io::Error)
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

type Result<T> = std::result::Result<T, HttpError>;

pub trait Escaped {
    fn escape(&self) -> String;
}

impl<T> Escaped for T
where
    T: AsRef<[u8]>
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
        format!("info_hash={}&peer_id={}&port={}&uploaded={}&downloaded={}&event={}&compact={}",
                self.info_hash.escape(),
                self.peer_id.escape(),
                self.port,
                self.uploaded,
                self.downloaded,
                self.event,
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

const DEFAULT_HEADERS: &str = "User-Agent: rustorrent/0.1\r\nAccept-Encoding: gzip\r\nConnection: close";

fn format_host(url: &Url) -> String {
    if let Some(port) = url.port() {
        format!("Host: {}:{}", url.host_str().unwrap(), port)
    } else {
        format!("Host: {}", url.host_str().unwrap())
    }
}

fn format_request<T: ToQuery>(url: &Url, query: &T) -> String {
    format!(
        "GET {}?{} HTTP/1.1\r\n{}\r\n{}\r\n\r\n",
        url.path(),
        query.to_query(),
        format_host(url),
        DEFAULT_HEADERS
    )
}

use std::time::Duration;
//use std::convert::TryInto;
use crate::utils::ConnectTimeout;

async fn send<T: DeserializeOwned, Q: ToQuery>(url: &Url, query: &Q, addr: &SocketAddr) -> Result<T> {

    let mut stream = TcpStream::connect_timeout(addr, Duration::from_secs(5)).await?;

    let req = format_request(url, query);

    println!("REQ {}", req);

    stream.write(req.as_bytes()).await?;
    stream.flush().await?;

    let response = read_response(stream).await?;

    println!("DATA {:x?}", String::from_utf8_lossy(&response));

    let value = from_bytes(&response)?;

    Ok(value)
}

use memchr::memchr;
use async_std::io::BufReader;

async fn read_response(stream: TcpStream) -> Result<Vec<u8>> {
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
            _ => return Err(HttpError::Malformed)
        };

        let name = &string[..index].trim().to_lowercase();
        let value = &string[index + 1..].trim();

        if name == "content-length" {
            content_length = Some(value.parse().map_err(|_| HttpError::Malformed)?);
        }
    }

    let content_length = match content_length {
        Some(content_length) => content_length,
        _ => return Err(HttpError::MissingContentLength)
    };

    let mut buffer = Vec::with_capacity(content_length as usize);

    reader.take(content_length).read_to_end(&mut buffer).await?;

    Ok(buffer)
}

pub async fn http_get<R, Q>(url: &Url, query: &Q, addr: &SocketAddr) -> Result<R>
where
    Q: ToQuery,
    R: DeserializeOwned
{
    println!("URL: {:?} {:?} {:?} {:?}", url, url.host(), url.port(), url.scheme());

    send(url, query, addr).await
}

pub struct HttpConnection {
    data: Arc<TrackerData>,
    addr: Vec<Arc<SocketAddr>>,
}

#[async_trait]
impl TrackerConnection for HttpConnection {
    async fn announce(&mut self, connected_addr: &mut usize) -> TResult<Vec<SocketAddr>> {
        let query = AnnounceQuery::from(self.data.metadata.as_ref());
        let mut last_err = None;
        for (index, addr) in self.addr.iter().enumerate() {
            let response = match http_get(&self.data.url, &query, addr).await {
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
            Some(e) => Err(e.into()),
            _ => Err(TorrentError::Unresponsive)
        }
    }

    async fn scrape(&mut self) -> TResult<()> {
        Ok(())
    }
}

impl HttpConnection {
    pub fn new(
        data: Arc<TrackerData>,
        addr: Vec<Arc<SocketAddr>>,
    ) -> Box<dyn TrackerConnection + Send + Sync>
    {
        Box::new(Self { data, addr })
    }
}
