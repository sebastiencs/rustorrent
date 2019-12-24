<h1 align="center">rustorrent</h1>
<div align="center">
  <strong>
    A BitTorrent library implemented in Rust
  </strong>
</div>


<br />

<div align="center">
  <a href="https://github.com/sebastiencs/rustorrent">
    <img src="https://img.shields.io/github/last-commit/sebastiencs/rustorrent?style=flat-square"
         alt="Last activity" />
  </a>
  <!-- Status -->
  <a href="https://github.com/sebastiencs/rustorrent">
    <img src="https://img.shields.io/badge/status-in%20development-orange?style=flat-square"
         alt="Status" />
  </a>
  <!-- Rust toolchain -->
  <a href="https://github.com/sebastiencs/rustorrent">
    <img src="https://img.shields.io/badge/rust-nightly-blue?style=flat-square"
         alt="rust toolchain" />
  </a>
</div>

<br />

Rustorrent is intented to be a full featured BitTorrent implementation.  
It is in active development and is not usable yet.

The library uses asynchronous Rust code with [async-std](https://github.com/async-rs/async-std)
and builds on the nightly toolchain to make use of SIMD instructions.

## Implemented [BEPs](https://www.bittorrent.org/beps/bep_0000.html)
- [The BitTorrent Protocol Specification](https://www.bittorrent.org/beps/bep_0003.html)
- [Extension Protocol](https://www.bittorrent.org/beps/bep_0010.html)
- [Peer Exchange PEX](https://www.bittorrent.org/beps/bep_0011.html)
- [Multitracker Metadata Extension](https://www.bittorrent.org/beps/bep_0012.html)
- [UDP Tracker Protocol](https://www.bittorrent.org/beps/bep_0015.html)
- [Tracker Returns Compact Peer Lists](https://www.bittorrent.org/beps/bep_0023.html)
- [uTorrent transport protocol](https://www.bittorrent.org/beps/bep_0029.html)
- [IPv6 Tracker Extension](https://www.bittorrent.org/beps/bep_0007.html)

## Architecture

Rustorrent is based on the [Actor model](https://en.wikipedia.org/wiki/Actor_model):
- TODO with diagrams
