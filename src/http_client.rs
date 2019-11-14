use std::io::prelude::*;
use std::net::TcpStream;
use std::io::{self, BufReader};

use serde::Serialize;
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

impl ToQuery for crate::Query {
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

    // Algo borrowed from libtorrent
    for b in bytes {
        if memchr(*b, UNRESERVED_CHAR).is_some() {
            result.push(*b);
        } else {
            result.push(b'%');
            result.push(HEXCHARS[(b >> 4) as usize]);
            result.push(HEXCHARS[(b & 15) as usize]);
        }
    }

    println!("RESULT: {:?}", String::from_utf8(result.clone()));

    String::from_utf8(result).unwrap()
}

const DEFAULT_HEADERS: &str = "User-Agent: rustorrent/0.1\r\nAccept-Encoding: gzip\r\nConnection: close";

const REQUEST_STR: &str = "GET {}?{} HTTP/1.1\r\n{}\r\n{}\r\n\r\n";

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

fn send(url: Url, query: impl ToQuery) {
    let sockaddr = (url.host_str().unwrap(), url.port().unwrap_or(80));

    let mut stream = TcpStream::connect(sockaddr).unwrap();

    let s = format_request(&url, query);

    println!("REQ {}", s);
    
    stream.write_all(s.as_bytes()).unwrap();
    stream.flush();

    // let mut data = Vec::new();

    // stream.read_to_end(&mut data);

    // println!("DATA {:x?}", String::from_utf8_lossy(&data));

    read_response(stream);
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

fn read_response(mut stream: TcpStream) {
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
            Err(_) => {}
        }
    }
    
    unsafe {
        buffer.set_len(state.offset);
    }

    println!("DATA {:x?}", String::from_utf8_lossy(&buffer));
}

pub fn get<T, Q>(url: T, query: Q)
where
    T: AsRef<str>,
    Q: ToQuery
{
    let url: Url = url.as_ref().parse().unwrap();
    //let query = query.to_query();
    
    println!("URL: {:?} {:?} {:?} {:?}", url, url.host(), url.port(), url.scheme());
    //println!("QUERY: {:?}", query);

    send(url, query);
}
