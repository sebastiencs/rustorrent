
use serde::de::DeserializeOwned;
use serde::{self, Deserialize, Serialize};
use serde_bytes::ByteBuf;

use hashbrown::HashMap;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ExtendedHandshake {
    /// Dictionary of supported extension messages which maps names of
    /// extensions to an extended message ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub m: Option<HashMap<String, i64>>,
    /// Client name and version (as a utf-8 string). This is a much
    /// more reliable way of identifying the client than relying on
    /// the peer id encoding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v: Option<String>,
    /// The number of outstanding request messages this client supports
    /// without dropping any. The default in in libtorrent is 250.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reqq: Option<i64>,
    /// Local TCP listen port. Allows each side to learn about the TCP
    /// port number of the other side. Note that there is no need for
    /// the receiving side of the connection to send this extension
    /// message, since its port number is already known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p: Option<i64>,
    /// A string containing the compact representation of the ip address
    /// this peer sees you as
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yourip: Option<ByteBuf>,
    /// If this peer has an IPv4 interface, this is the compact
    /// representation of that address (4 bytes). The client may prefer
    /// to connect back via the IPv6 address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<ByteBuf>,
    /// If this peer has an IPv6 interface, this is the compact
    /// representation of that address (16 bytes). The client may prefer
    /// to connect back via the IPv6 address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<ByteBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_only: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ut_holepunch: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lt_donthave: Option<i64>,
    /// the time when this peer last saw a complete copy
	/// of this torrent
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete_ago: Option<i64>
}

use crate::bencode::PtrBuf;

use std::net::SocketAddr;

// We only deserialize the struct with pointers so we avoid
// too many allocations
// See the Into<Vec<SocketAddr>>> implementation below
#[derive(Deserialize, Debug)]
pub struct PEXMessage<'a> {
    /// <one or more contacts in IPv4 compact format (string)>
    #[serde(borrow)]
    added: Option<PtrBuf<'a>>,
    /// <optional, bit-flags, 1 byte per added IPv4 peer (string)>
    #[serde(rename = "added.f", borrow)]
    added_flags: Option<PtrBuf<'a>>,
    /// <one or more contacts IPv6 compact format (string)>,
    #[serde(borrow)]
    added6: Option<PtrBuf<'a>>,
    /// added6.f: <optional, bit-flags, 1 byte per added IPv6 peer (string)>,
    #[serde(rename = "added6.f", borrow)]
    added6_flags: Option<PtrBuf<'a>>,
    /// <one or more contacts in IPv6 compact format (string)>,
    #[serde(borrow)]
    dropped: Option<PtrBuf<'a>>,
    /// <one or more contacts in IPv6 compact format (string)>
    #[serde(borrow)]
    dropped6: Option<PtrBuf<'a>>,
}

use crate::utils;

impl<'a> Into<Vec<SocketAddr>> for PEXMessage<'a> {
    fn into(self) -> Vec<SocketAddr> {
        let mut length = self.added.as_ref().map(|a| a.slice.len() / 6).unwrap_or(0);
        length += self.added6.as_ref().map(|a| a.slice.len() / 18).unwrap_or(0);

        // 1 alloc
        let mut addrs = Vec::with_capacity(length);

        if let Some(PtrBuf { slice }) = self.added {
            utils::ipv4_from_slice(slice, &mut addrs);
        };

        if let Some(PtrBuf { slice }) = self.added6 {
            utils::ipv6_from_slice(slice, &mut addrs);
        };

        addrs
    }
}

#[derive(Debug)]
pub enum ExtendedMessage<'a> {
    Handshake(ExtendedHandshake),
    Message {
        id: u8,
        buffer: &'a [u8]
    }
}

// // peer plugins are associated with a specific peer. A peer could be
// // both a regular bittorrent peer (``bt_peer_connection``) or one of the
// // web seed connections (``web_peer_connection`` or ``http_seed_connection``).
// // In order to only attach to certain peers, make your
// // torrent_plugin::new_connection only return a plugin for certain peer
// // connection types
// struct TORRENT_EXPORT peer_plugin
// {
// 	// hidden
// 	virtual ~peer_plugin() {}

// 	// This function is expected to return the name of
// 	// the plugin.
// 	virtual string_view type() const { return {}; }

// 	// can add entries to the extension handshake
// 	// this is not called for web seeds
// 	virtual void add_handshake(entry&) {}

// 	// called when the peer is being disconnected.
// 	virtual void on_disconnect(error_code const&) {}

// 	// called when the peer is successfully connected. Note that
// 	// incoming connections will have been connected by the time
// 	// the peer plugin is attached to it, and won't have this hook
// 	// called.
// 	virtual void on_connected() {}

// 	// throwing an exception from any of the handlers (except add_handshake)
// 	// closes the connection

// 	// this is called when the initial bittorrent handshake is received.
// 	// Returning false means that the other end doesn't support this extension
// 	// and will remove it from the list of plugins. this is not called for web
// 	// seeds
// 	virtual bool on_handshake(span<char const>) { return true; }

// 	// called when the extension handshake from the other end is received
// 	// if this returns false, it means that this extension isn't
// 	// supported by this peer. It will result in this peer_plugin
// 	// being removed from the peer_connection and destructed.
// 	// this is not called for web seeds
// 	virtual bool on_extension_handshake(bdecode_node const&) { return true; }

// 	// returning true from any of the message handlers
// 	// indicates that the plugin has handled the message.
// 	// it will break the plugin chain traversing and not let
// 	// anyone else handle the message, including the default
// 	// handler.
// 	virtual bool on_choke() { return false; }
// 	virtual bool on_unchoke() { return false; }
// 	virtual bool on_interested() { return false; }
// 	virtual bool on_not_interested() { return false; }
// 	virtual bool on_have(piece_index_t) { return false; }
// 	virtual bool on_dont_have(piece_index_t) { return false; }
// 	virtual bool on_bitfield(bitfield const& /*bitfield*/) { return false; }
// 	virtual bool on_have_all() { return false; }
// 	virtual bool on_have_none() { return false; }
// 	virtual bool on_allowed_fast(piece_index_t) { return false; }
// 	virtual bool on_request(peer_request const&) { return false; }

// 	// This function is called when the peer connection is receiving
// 	// a piece. ``buf`` points (non-owning pointer) to the data in an
// 	// internal immutable disk buffer. The length of the data is specified
// 	// in the ``length`` member of the ``piece`` parameter.
// 	// returns true to indicate that the piece is handled and the
// 	// rest of the logic should be ignored.
// 	virtual bool on_piece(peer_request const& /*piece*/
// 			              , span<char const> /*buf*/) { return false; }

// 	virtual bool on_cancel(peer_request const&) { return false; }
// 	virtual bool on_reject(peer_request const&) { return false; }
// 	virtual bool on_suggest(piece_index_t) { return false; }

// 	// called after a choke message has been sent to the peer
// 	virtual void sent_unchoke() {}

// 	// called after piece data has been sent to the peer
// 	// this can be used for stats book keeping
// 	virtual void sent_payload(int /* bytes */) {}

// 	// called when libtorrent think this peer should be disconnected.
// 	// if the plugin returns false, the peer will not be disconnected.
// 	virtual bool can_disconnect(error_code const& /*ec*/) { return true; }

// 	// called when an extended message is received. If returning true,
// 	// the message is not processed by any other plugin and if false
// 	// is returned the next plugin in the chain will receive it to
// 	// be able to handle it. This is not called for web seeds.
// 	// thus function may be called more than once per incoming message, but
// 	// only the last of the calls will the ``body`` size equal the ``length``.
// 	// i.e. Every time another fragment of the message is received, this
// 	// function will be called, until finally the whole message has been
// 	// received. The purpose of this is to allow early disconnects for invalid
// 	// messages and for reporting progress of receiving large messages.
// 	virtual bool on_extended(int /*length*/, int /*msg*/,
// 			                 span<char const> /*body*/)
// 	{ return false; }

// 	// this is not called for web seeds
// 	virtual bool on_unknown_message(int /*length*/, int /*msg*/,
// 			                        span<char const> /*body*/)
// 	{ return false; }

// 	// called when a piece that this peer participated in either
// 	// fails or passes the hash_check
// 	virtual void on_piece_pass(piece_index_t) {}
// 	virtual void on_piece_failed(piece_index_t) {}

// 	// called approximately once every second
// 	virtual void tick() {}

// 	// called each time a request message is to be sent. If true
// 	// is returned, the original request message won't be sent and
// 	// no other plugin will have this function called.
// 	virtual bool write_request(peer_request const&) { return false; }
// }

pub trait Extension {
    /// Name of this extension
    const NAME: &'static str;

    fn handshake() {}

    fn handle_hanshake() {}

    fn handle_extended_hanshake() {}
}
