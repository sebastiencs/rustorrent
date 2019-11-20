use std::io::prelude::*;
use std::net::TcpStream;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use url::Url;

// // http://www.ietf.org/rfc/rfc2396.txt
// // section 2.3
// static char const unreserved_chars[] =
// // when determining if a url needs encoding
// // % should be ok
// 	"%+"
// // reserved
// 	";?:@=&,$/"
// // unreserved (special characters) ' excluded,
// // since some buggy trackers fail with those
// 	"-_!.~*()"
// // unreserved (alphanumerics)
// 	"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
// 	"0123456789";

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

use crate::metadata::Torrent;

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

    println!("RESULT: {:?}", String::from_utf8(result.clone()));

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

fn format_request<T: ToQuery>(url: &Url, query: T) -> String {
    format!(
        "GET {}?{} HTTP/1.1\r\n{}\r\n{}\r\n\r\n",
        url.path(),
        query.to_query(),
        format_host(url),
        DEFAULT_HEADERS
    )
}

use std::time::Duration;
use std::net::ToSocketAddrs;
use std::convert::TryInto;

fn try_addr(sockaddr: std::vec::IntoIter<std::net::SocketAddr>) -> Result<TcpStream> {
    let mut last_err = None;
    for addr in sockaddr {
        match TcpStream::connect_timeout(
            &addr,
            Duration::from_secs(5)
        ) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = Some(Err(e))
        }
    }
    match last_err {
        Some(e) => e?,
        _ => Err(HttpError::HostResolution)?
    }
}

fn send<T: DeserializeOwned>(url: Url, query: impl ToQuery) -> Result<T> {
    let sockaddr = (
        url.host_str().ok_or(HttpError::HostResolution)?,
        url.port().unwrap_or(80)
    ).to_socket_addrs()?;

    let mut stream = try_addr(sockaddr)?;

    let req = format_request(&url, query);

    println!("REQ {}", req);

    stream.write_all(req.as_bytes())?;
    stream.flush()?;

    let (buffer, state) = read_response(stream)?;

    let response = &buffer[state.header_length.unwrap()..];

    println!("DATA {:x?}", String::from_utf8_lossy(&response));

    let value = from_bytes(&response)?;

    Ok(value)
}

use memchr::{memchr, memrchr2};

fn get_content_length(buf: &[u8], state: &mut ReadingState) {
    // We skip the 1st line
    let mut buf = match memchr(b'\n', buf) {
        Some(index) => &buf[std::cmp::min(index + 1, buf.len())..],
        _ => return
    };

    loop {
        let line = match memchr(b'\n', buf) {
            Some(index) => &buf[..index],
            _ => return
        };

        // We convert the line to a string to later compare it lowercased
        let line = match std::str::from_utf8(line) {
            Ok(line) => line.to_lowercase(),
            _ => {
                state.valid_headers = false;
                return;
            }
        };

        let len = line.len();
        let line = line.trim();

        if line.starts_with("content-length:") {
            let value = match memrchr2(b':', b' ', line.as_bytes()) {
                Some(index) => &line[index..],
                _ => return
            };

            match value.trim().parse() {
                Ok(value) => {
                    state.content_length = Some(value);
                    return;
                },
                _ => {
                    state.valid_headers = false;
                    return;
                }
            };
        }

        if line.is_empty() { return }

        buf = &buf[std::cmp::min(len + 1, buf.len())..];
    }
}

fn get_header_length(buf: &[u8], state: &mut ReadingState) -> usize {
    let start = buf.as_ptr();

    // We skip the 1st line
    let mut buf = match memchr(b'\n', buf) {
        Some(index) => &buf[std::cmp::min(index + 1, buf.len())..],
        _ => return 0
    };

    loop {
        let line = match memchr(b'\n', buf) {
            Some(index) => &buf[..index],
            _ => return 0
        };

        if !line.is_empty() && line[0] == b'\r' {
            let length = line.as_ptr() as usize - start as usize + 2;
            state.header_length = Some(length);
            return length;
        }

        buf = match memchr(b'\n', buf) {
            Some(index) => &buf[std::cmp::min(index + 1, buf.len())..],
            _ => return 0
        };
    }
}

fn check_http_code(buf: &[u8]) -> Result<()> {
    let line = match memchr(b'\n', buf) {
        Some(index) => &buf[..index],
        _ => return Err(HttpError::Malformed)
    };

    let line = match std::str::from_utf8(line) {
        Ok(line) => line.trim(),
        _ => return Err(HttpError::Malformed)
    };

    if line.contains("200") {
        Ok(())
    } else {
        println!("BUFFER: {:?}\n", String::from_utf8_lossy(buf));
        Err(HttpError::ResponseCode(line.to_owned()))
    }
}

fn stopper(buf: &[u8], state: &mut ReadingState) -> bool {
    if !state.valid_headers {
        return false;
    }

    if state.content_length.is_none() {
        get_content_length(buf, state);
    }

    if state.header_length.is_none() {
        get_header_length(buf, state);
    }

    match (state.header_length, state.content_length) {
        (Some(header), Some(content)) => {
            if header + content == state.offset {
                return true;
            }
        }
        _ => {}
    }

    false
}

#[derive(Debug)]
struct ReadingState {
    offset: usize,
    valid_headers: bool,
    header_length: Option<usize>,
    content_length: Option<usize>,
}

impl Default for ReadingState {
    fn default() -> ReadingState {
        ReadingState {
            offset: 0,
            valid_headers: true,
            header_length: None,
            content_length: None,
        }
    }
}

const BUFFER_READ_SIZE: usize = 64;

fn read_response(mut stream: TcpStream) -> Result<(Vec<u8>, ReadingState)> {
    let mut buffer = Vec::with_capacity(BUFFER_READ_SIZE);

    unsafe { buffer.set_len(BUFFER_READ_SIZE); }

    let mut state = ReadingState::default();

    loop {
        if state.offset == buffer.len() {
            buffer.reserve(BUFFER_READ_SIZE);
            unsafe {
                buffer.set_len(buffer.capacity());
            }
        }

        match stream.read(&mut buffer[state.offset..]) {
            Ok(0) => break,
            Ok(n) => {
                state.offset += n;
                if stopper(&buffer, &mut state) {
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(HttpError::IO(e))
        }
    }

    unsafe {
        buffer.set_len(state.offset);
    }

    //println!("DATA {:x?}", String::from_utf8_lossy(&buffer));

    if state.header_length.is_none() && get_header_length(&buffer, &mut state) == 0 {
        return Err(HttpError::Malformed);
    }

    check_http_code(&buffer)?;

    Ok((buffer, state))
}

pub fn get<R, T, Q>(url: T, query: Q) -> Result<R>
where
    T: AsRef<str>,
    Q: ToQuery,
    R: DeserializeOwned
{
    let url: Url = url.as_ref().parse().unwrap();

    println!("URL: {:?} {:?} {:?} {:?}", url, url.host(), url.port(), url.scheme());

    send(url, query)
}
