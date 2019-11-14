use serde::{Serialize, Deserialize};
use smallvec::SmallVec;

type StackVec<T> = SmallVec<[T; 16]>;

#[derive(Debug, Serialize, Deserialize)]
pub struct MetaFile<'a> {
    pub length: i64,
    pub md5sum: Option<&'a str>,
    pub path: StackVec<&'a str>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InfoFile<'a> {
    Single {
        name: &'a str,
        length: i64,
        md5sum: Option<&'a str>,        
    },
    Multiple {
        name: &'a str,
        files: StackVec<MetaFile<'a>>        
    },
}

#[derive(Serialize, Deserialize)]
pub struct MetaInfo<'a> {
    #[serde(with = "serde_bytes")]
    pub pieces: &'a [u8],
    #[serde(rename="piece length")]
    pub piece_length: i64,
    pub private: Option<i64>,
    #[serde(flatten)]
    pub files: InfoFile<'a>,
}

impl<'a> std::fmt::Debug for MetaInfo<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Info")
         .field("piece_length", &self.piece_length)
         .field("pieces", &&self.pieces[..10])
         .field("files", &self.files)
         .finish()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MetaTorrent<'a> {
    
    pub announce: &'a str,
    
    pub info: MetaInfo<'a>,
    
    #[serde(rename="announce-list")]
    pub announce_list: Option<StackVec<StackVec<&'a str>>>,
    
    #[serde(rename="creation date")]
    pub creation_date: Option<u64>,
    
    pub comment: Option<&'a str>,
    
    #[serde(rename="created by")] 
    pub created_by: Option<&'a str>,

    pub encoding: Option<&'a str>
}
