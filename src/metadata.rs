use itertools::Itertools;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use url::Url;

use std::iter::Iterator;
use std::ops::Deref;
use std::path::MAIN_SEPARATOR;
use std::sync::Arc;
use std::{
    convert::TryInto,
    hash::{Hash, Hasher},
    path::Path,
    path::PathBuf,
};

type StackVec<T> = SmallVec<[T; 16]>;

#[derive(Debug, Serialize, Deserialize)]
pub struct MetaFile {
    pub length: u64,
    pub md5sum: Option<String>,
    pub path: StackVec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InfoFile {
    Single {
        name: String,
        length: u64,
        md5sum: Option<String>,
    },
    Multiple {
        name: String,
        files: StackVec<MetaFile>,
    },
}

#[derive(Debug)]
pub struct TorrentFile {
    path: PathBuf,
    // TODO: Make it a shared pointer
    md5sum: Option<String>,
    length: u64,
}

#[derive(Serialize, Deserialize)]
pub struct MetaInfo {
    #[serde(with = "serde_bytes")]
    pub pieces: Vec<u8>,
    #[serde(rename = "piece length")]
    pub piece_length: u64,
    pub private: Option<i64>,
    #[serde(flatten)]
    pub files: InfoFile,
}

impl<'a> std::fmt::Debug for MetaInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Info")
            .field("piece_length", &self.piece_length)
            .field("pieces", &&self.pieces.get(0..self.pieces.len()))
            .field("files", &self.files)
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UrlList {
    Single(String),
    Multiple(Vec<String>),
    Ignore(Vec<Vec<String>>),
    Ignore2(Vec<i64>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MetaTorrent {
    pub announce: Option<String>,
    pub info: MetaInfo,
    #[serde(rename = "announce-list")]
    pub announce_list: Option<StackVec<StackVec<String>>>,
    #[serde(rename = "creation date")]
    pub creation_date: Option<u64>,
    pub comment: Option<String>,
    #[serde(rename = "created by")]
    pub created_by: Option<String>,
    pub encoding: Option<String>,
    #[serde(rename = "url-list")]
    pub url_list: Option<UrlList>,
}

#[derive(Debug)]
pub struct Torrent {
    pub meta: MetaTorrent,
    pub info_hash: Arc<Vec<u8>>,
}

pub struct UrlIterator<'a> {
    list: Vec<&'a str>,
    index: usize,
}

impl<'a> Iterator for UrlIterator<'a> {
    type Item = Url;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let index = self.index;
            self.index += 1;
            match self.list.get(index).map(|u| u.parse()) {
                Some(Ok(url)) => return Some(url),
                None => return None,
                _ => {}
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.list.len(), None)
    }
}

#[derive(Debug, Eq)]
pub struct TrackerUrl {
    url: Url,
    hash: UrlHash,
    tier: usize,
}

impl PartialEq for TrackerUrl {
    fn eq(&self, other: &TrackerUrl) -> bool {
        self.hash() == other.hash()
    }
}

impl Deref for TrackerUrl {
    type Target = Url;

    fn deref(&self) -> &Url {
        &self.url
    }
}

#[derive(Debug, Copy, PartialEq, Eq, Hash, Clone)]
pub struct UrlHash(u64);

impl TrackerUrl {
    fn new(url: Url, tier: usize) -> TrackerUrl {
        let mut hasher = ahash::AHasher::new_with_keys(12345, 4242);
        url.hash(&mut hasher);
        let hash = UrlHash(hasher.finish());

        TrackerUrl { url, hash, tier }
    }

    pub fn hash(&self) -> UrlHash {
        self.hash
    }
}

impl Torrent {
    pub fn get_urls_tiers(&self) -> Vec<Arc<TrackerUrl>> {
        let mut vec = self
            .meta
            .announce_list
            .as_ref()
            .map(|list| {
                list.iter()
                    .enumerate()
                    .flat_map(|(index, tier)| {
                        tier.iter()
                            .filter_map(|url_str| url_str.parse::<Url>().ok())
                            // .filter(TrackerSupervisor::is_scheme_supported)
                            .map(|u| Arc::new(TrackerUrl::new(u, index)))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(Vec::new);

        if let Some(url) = self
            .meta
            .announce
            .as_ref()
            .and_then(|a| a.parse().map(|u| TrackerUrl::new(u, 10)).ok())
        {
            if !vec.iter().any(|u| **u == url) {
                vec.push(Arc::new(url));
            }
        }

        vec
    }

    pub fn iter_urls(&self) -> UrlIterator {
        let mut urls = self
            .meta
            .announce_list
            .as_ref()
            .map(|list| {
                list.iter()
                    .flat_map(|u| u.iter().map(|s| s.as_str()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(Vec::new);

        if let Some(announce) = self.meta.announce.as_ref() {
            if !urls.contains(&announce.as_str()) {
                urls.push(announce);
            }
        };

        UrlIterator {
            index: 0,
            list: urls,
        }
    }

    pub fn files_total_size(&self) -> usize {
        match &self.meta.info.files {
            InfoFile::Single { length, .. } => *length as usize,
            InfoFile::Multiple { files, .. } => files.iter().map(|f| f.length as usize).sum(),
        }
    }

    pub fn sha_pieces(&self) -> Vec<Arc<[u8; 20]>> {
        let pieces = self.meta.info.pieces.as_slice();

        pieces
            .chunks_exact(20)
            .map(|p| Arc::new(p.try_into().unwrap()))
            .collect()
    }

    pub fn web_seeds(&self) -> Vec<String> {
        match &self.meta.url_list {
            Some(UrlList::Single(url)) => vec![url.clone()],
            Some(UrlList::Multiple(urls)) => urls.iter().unique().cloned().collect(),
            _ => vec![],
        }
    }

    pub fn nfiles(&self) -> usize {
        match &self.meta.info.files {
            InfoFile::Single { .. } => 1,
            InfoFile::Multiple { files, .. } => files.len(),
        }
    }

    pub fn files(&self) -> Vec<TorrentFile> {
        match &self.meta.info.files {
            InfoFile::Single {
                name,
                length,
                md5sum,
            } => {
                let name = name
                    .chars()
                    .filter(|c| !std::path::is_separator(*c))
                    .collect::<String>();
                vec![TorrentFile {
                    path: PathBuf::from(name),
                    length: *length,
                    md5sum: md5sum.clone(),
                }]
            }
            InfoFile::Multiple { files, name } => {
                let sep = MAIN_SEPARATOR.to_string();

                files
                    .iter()
                    .map(|ref file| {
                        let name =
                            std::iter::Iterator::chain(std::iter::once(name), file.path.iter())
                                .map(|s| {
                                    s.chars()
                                        .filter(|c| !std::path::is_separator(*c))
                                        .collect::<String>()
                                })
                                .intersperse(sep.clone())
                                .collect::<String>();

                        TorrentFile {
                            path: Path::new(name.as_str()).iter().collect(),
                            md5sum: file.md5sum.clone(),
                            length: file.length,
                        }
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bencode::de;
    use itertools::assert_equal;
    use std::ffi::OsStr;

    #[test]
    fn read_torrent_file() {
        let file = env!("CARGO_MANIFEST_DIR").to_owned()
            + "/scripts/Fedora-Workstation-Live-x86_64-33_Beta.torrent";
        let buffer = std::fs::read(file).unwrap();

        let torrent = de::read_meta(&buffer).unwrap();

        let tiers = torrent.get_urls_tiers();
        println!("TIERS {:#?}", tiers);

        torrent.sha_pieces();
        torrent.files_total_size();

        let iter = torrent.iter_urls();

        assert_eq!(iter.size_hint(), (1, None));

        for url in iter {
            println!("url {:?}", url);
        }

        println!("{:?}", torrent);
    }

    #[test]
    fn parse_torrent_fail() {
        use de::DeserializeError::*;

        #[derive(Debug)]
        struct TorrentFail {
            filename: &'static str,
            error: de::DeserializeError,
        }

        macro_rules! declare_torrent_errors (
            (
                $( { $file:tt, $error:expr } ),*
            ) => (
                &[$(TorrentFail { filename: $file, error: $error },)*];
            )
        );

        for torrent_error in declare_torrent_errors![
            { "missing_piece_len.torrent", Message("missing field `piece length`".into()) },
            { "invalid_piece_len.torrent", Message("invalid type: byte array, expected u64".into()) },
            { "negative_piece_len.torrent", Message("invalid value: integer `-16384`, expected u64".into()) },
            { "no_name.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "bad_name.torrent", Message("invalid value: byte array, expected a string".into()) },
            { "invalid_name.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "invalid_info.torrent", Message("invalid type: byte array, expected struct MetaInfo".into()) },
            { "string.torrent", Message("invalid type: byte array, expected struct MetaTorrent".into()) },
            { "negative_size.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "negative_file_size.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "invalid_path_list.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "missing_path_list.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "invalid_pieces.torrent", Message("invalid type: integer `-23`, expected byte array".into()) },
            { "unaligned_pieces.torrent", UnalignedPieces },
            { "invalid_file_size.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "invalid_symlink.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            // { "many_pieces.torrent", Message("missing field `announce`".into()) },
            { "no_files.torrent", NoFile },
            { "zero.torrent", EmptyFile },
            // { "zero2.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            // { "v2_mismatching_metadata.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "v2_no_power2_piece.torrent", Message("missing field `pieces`".into()) },
            // { "v2_invalid_file.torrent", Message("data did not match any variant of untagged enum InfoFile".into()) },
            { "v2_deep_recursion.torrent", TooDeep },
            { "v2_non_multiple_piece_layer.torrent", Message("missing field `pieces`".into()) },
            { "v2_piece_layer_invalid_file_hash.torrent", Message("missing field `pieces`".into()) },
            { "v2_invalid_piece_layer.torrent", Message("missing field `pieces`".into()) },
            { "v2_invalid_piece_layer_size.torrent", Message("missing field `pieces`".into()) },
            // { "v2_bad_file_alignment.torrent", Message("missing field `announce`".into()) },
            { "v2_unordered_files.torrent", Message("missing field `pieces`".into()) },
            { "v2_overlong_integer.torrent", Message("missing field `pieces`".into()) },
            { "v2_missing_file_root_invalid_symlink.torrent", Message("missing field `pieces`".into()) },
            { "v2_large_file.torrent", Message("missing field `pieces`".into()) },
            { "v2_no_piece_layers.torrent", Message("missing field `pieces`".into()) },
            { "v2_large_offset.torrent", Message("missing field `pieces`".into()) },
            { "v2_piece_size.torrent", Message("missing field `pieces`".into()) },
            { "v2_invalid_pad_file.torrent", Message("missing field `pieces`".into()) },
            { "v2_zero_root.torrent", Message("missing field `pieces`".into()) },
            { "v2_zero_root_small.torrent", Message("missing field `pieces`".into()) }
        ] {
            println!("Processing {:?}", torrent_error);
            let filename = env!("CARGO_MANIFEST_DIR").to_owned()
                + "/scripts/test_torrents/"
                + torrent_error.filename;
            let content = std::fs::read(filename).unwrap();
            let result = de::read_meta(&content);

            assert!(
                result.is_err() && result.as_ref().err() == Some(&torrent_error.error),
                "Fail on {:?}: Result: '{:?}', should be: '{:?}'",
                torrent_error.filename,
                result,
                torrent_error.error // grcov_ignore
            );
        }
    }

    #[test]
    fn parse_torrent_success() {
        use super::Torrent;

        struct TorrentSuccess {
            filename: &'static str,
            assert: Box<dyn Fn(&Torrent)>,
        }

        macro_rules! declare_torrent_success (
            (
                $( { $file:tt, $assert:expr } ),*
            ) => (
                &[$(TorrentSuccess { filename: $file, assert: Box::new($assert) },)*];
            )
        );

        // TODO:
        // https://blog.libtorrent.org/2020/09/bittorrent-v2/

        for torrent_success in declare_torrent_success![
            { "base.torrent", |_| {} },
            { "empty_path.torrent" , |_| {} },
            { "parent_path.torrent" , |_| {} },
            { "hidden_parent_path.torrent", |_| {} },
            { "single_multi_file.torrent", |_| {} },
            { "slash_path.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 1);
                assert_equal(torrent.files()[0].path.iter(), ["temp", "bar"].iter().map(OsStr::new));
            }
            },
            { "slash_path2.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 1);
                assert_equal(torrent.files()[0].path.iter(), ["temp", "abc....def", "bar"].iter().map(OsStr::new));
            }
            },
            { "slash_path3.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 1);
                assert_equal(torrent.files()[0].path.iter(), ["temp....abc"].iter().map(OsStr::new));
            }
            },
            { "backslash_path.torrent", |_| {} },
            { "url_list.torrent", |_| {} },
            { "url_list2.torrent", |_| {} },
            { "url_list3.torrent", |_| {} },
            { "httpseed.torrent", |_| {} },
            { "empty_httpseed.torrent", |_| {} },
            { "long_name.torrent", |_| {} },
            { "whitespace_url.torrent", |torrent| {
                let urls = torrent.get_urls_tiers();
                assert!(!urls.is_empty());
                assert_eq!(urls[0].url.as_str(), "udp://test.com/announce");
            }
            },
            { "duplicate_files.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 2);
                assert_equal(torrent.files()[0].path.iter(), ["temp", "foo", "bar.txt"].iter().map(OsStr::new));
                // TODO
            }
            },
            { "pad_file.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 2);
                let files = torrent.files();
                assert_equal(files[0].path.iter(), ["temp", "foo", "bar.txt"].iter().map(OsStr::new));
                assert_equal(files[1].path.iter(), ["temp", "_____padding_file_"].iter().map(OsStr::new));
                // TODO
            }
            },
            { "creation_date.torrent", |torrent| {
                assert_eq!(torrent.meta.creation_date, Some(1234567));
            }
            },
            { "no_creation_date.torrent", |torrent| {
                assert_eq!(torrent.meta.creation_date, None);
            }
            },
            { "url_seed.torrent", |torrent| {
                assert!(torrent.meta.url_list.is_some());
                // TODO
            }
            },
            { "url_seed_multi.torrent", |torrent| {
                assert!(torrent.meta.url_list.is_some());
                // TODO
            }
            },
            { "url_seed_multi_single_file.torrent", |torrent| {
                assert!(torrent.meta.url_list.is_some());
                // TODO
            }
            },
            { "url_seed_multi_space.torrent", |torrent| {
                assert!(torrent.meta.url_list.is_some());
                // TODO
            }
            },
            { "url_seed_multi_space_nolist.torrent", |torrent| {
                assert!(torrent.meta.url_list.is_some());
                // TODO
            }
            },
            { "empty_path_multi.torrent", |_| {} },
            { "duplicate_web_seeds.torrent", |torrent| {
                assert_eq!(torrent.web_seeds().len(), 3);
            }
            },
            { "invalid_name2.torrent", |_torrent| {
                // TODO
            }
            },
            { "invalid_name3.torrent", |_torrent| {
                // TODO
            }
            },
            { "symlink1.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 2);
                assert_equal(torrent.files()[0].path.iter(), ["temp", "a", "b", "bar"].iter().map(OsStr::new));
            }
            },
            { "symlink2.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 5);
                assert_equal(torrent.files()[3].path.iter(), ["Some.framework", "Versions", "A", "SDL2"].iter().map(OsStr::new));
            }
            },
            { "unordered.torrent", |_| {} },
            // { "symlink_zero_size.torrent", |torrent| {
            // }
            // },
            // { "pad_file_no_path.torrent", |torrent| {
            // }
            // },
            { "large.torrent", |_| {} },
            { "absolute_filename.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 2);
                let files = torrent.files();
                assert_equal(files[0].path.iter(), ["temp", "abcde"].iter().map(OsStr::new));
                assert_equal(files[1].path.iter(), ["temp", "foobar"].iter().map(OsStr::new));
            }
            },
            { "invalid_filename.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 2);
            }
            },
            { "invalid_filename2.torrent", |torrent| {
                assert_eq!(torrent.nfiles(), 3);
            }
            },
            { "overlapping_symlinks.torrent", |_torrent| {
                // TODO
            }
            }
            // { "v2.torrent", |torrent| {
            // }
            // },
            // { "v2_multipiece_file.torrent", |torrent| {
            // }
            // },
            // { "v2_only.torrent", |torrent| {
            // }
            // },
            // { "v2_invalid_filename.torrent", |torrent| {
            // }
            // },
            // { "v2_multiple_files.torrent", |torrent| {
            // }
            // },
            // { "v2_symlinks.torrent", |torrent| {
            // }
            // },
            // { "v2_hybrid.torrent", |torrent| {
            // }
            // }
        ] {
            let filename = env!("CARGO_MANIFEST_DIR").to_owned()
                + "/scripts/test_torrents/"
                + torrent_success.filename;
            let content = std::fs::read(filename).unwrap();
            let result = de::read_meta(&content);

            assert!(
                result.is_ok(),
                "Fail on {:?} {:?}",
                torrent_success.filename,
                result
            );

            let torrent = result.unwrap();
            (torrent_success.assert)(&torrent);
            torrent.files_total_size();
            torrent.get_urls_tiers();
            torrent.sha_pieces();
            torrent.iter_urls();
        }
    }

    #[test]
    fn parse_torrent_file() {
        let dir = env!("CARGO_MANIFEST_DIR").to_owned() + "/scripts/test_torrents/";

        for item in std::fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let buffer = std::fs::read(path.as_path()).unwrap();
            let torrent = de::read_meta(&buffer);
            println!("{} {:?}", torrent.is_ok(), path.file_name());
            if let Ok(torrent) = torrent {
                torrent.files_total_size();
                torrent.get_urls_tiers();
                torrent.sha_pieces();
                torrent.iter_urls();
                torrent.web_seeds();
                println!("{:?}", torrent);
                println!("{:?}", torrent.files());
                println!("{:?}", torrent.get_urls_tiers());
                println!("{:?}", torrent.web_seeds());
            }
        }
    }

    #[test]
    fn url_list_debug() {
        // For coverage
        let list = super::UrlList::Single("a".to_string());
        println!("{:?}", list);
    }

    #[test]
    fn tracker_url_debug() {
        // For coverage
        let url = super::TrackerUrl::new("http://test.com".parse().unwrap(), 0);
        println!("{:?}", *url);
    }
}
