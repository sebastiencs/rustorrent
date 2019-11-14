use std::io::prelude::*;
use std::net::TcpStream;
use std::io::{self, BufReader};

use serde::Serialize;
use url::Url;
use memchr::memchr;

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

static UNRESERVED_CHAR: &[u8] =
	//"%+;?:@=&,$/"
    b"-_!.~*()ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

static HEXCHARS: &[u8] = b"0123456789abcdef";

pub fn escape_str<T: AsRef<[u8]>>(s: T) -> String {
    let bytes = s.as_ref();

    let mut result = Vec::with_capacity(bytes.len() * 3);

    // Algo borrowed from libtorrent
    for b in bytes.iter() {
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

fn send(url: Url, query: String) {
    let sockaddr = (url.host_str().unwrap(), url.port().unwrap());

    println!("ADDR: {:?}", sockaddr);
    
    let mut stream = TcpStream::connect(sockaddr).unwrap();

    let headers = "User-Agent: rustorrent/0.1\r\nAccept-Encoding: gzip\r\nConnection: close";

    let s = format!("GET {}?{}&left=1929380020 HTTP/1.1\r\nHost: {}:{}\r\n{}\r\n\r\n", url.path(), query, url.host().unwrap(), url.port().unwrap(), headers);

    println!("REQ {}", s);

    //escape_str(query)
    
    stream.write_all(s.as_bytes()).unwrap();
    stream.flush();
    
    let tcp = BufReader::new(stream);

    for line in tcp.lines() {
        println!("LINE: {:?}", line);
    }
}

pub fn get<T, Q>(url: T, query: Q)
where
    T: AsRef<str>,
    Q: ToQuery
{
    let url: Url = url.as_ref().parse().unwrap();
    let query = query.to_query();
    
    println!("URL: {:?} {:?} {:?} {:?}", url, url.host(), url.port(), url.scheme());
    println!("QUERY: {:?}", query);

    //send(url, query);
}
