use futures::ready;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::{
    io::unix::{AsyncFd, AsyncFdReadyGuard},
    net::{lookup_host, ToSocketAddrs},
};

use std::{
    io::ErrorKind,
    net::SocketAddr,
    os::unix::prelude::AsRawFd,
    task::{Context, Poll},
};

use super::UtpError;

pub type FdGuard<'a, T> = AsyncFdReadyGuard<'a, T>;

#[derive(Debug)]
pub struct MyUdpSocket {
    inner: AsyncFd<Socket>,
}

unsafe impl Send for MyUdpSocket {}
unsafe impl Sync for MyUdpSocket {}

const N_VECTORED_BUFFER: usize = 32;
const SOCKADDR_STORAGE_LENGTH: libc::socklen_t =
    std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;

pub struct MmsgBuffer {
    addr_storage: [libc::sockaddr_storage; N_VECTORED_BUFFER],
    iov: [libc::iovec; N_VECTORED_BUFFER],
    mmsghdr: [libc::mmsghdr; N_VECTORED_BUFFER],
    buffers: [[u8; 1500]; N_VECTORED_BUFFER],
    nrecv: u32,
}

impl std::fmt::Debug for MmsgBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("MmsgBuffer")
            .field("addr_storage", &self.addr_storage)
            .field("iov", &self.iov)
            .field("mmsghdr", &self.mmsghdr)
            // .field("buffers", &self.buffers)
            .finish()
    }
}

unsafe impl Send for MmsgBuffer {}

impl MmsgBuffer {
    pub fn new() -> Box<MmsgBuffer> {
        // TODO: Use Box::new_uninit when stable
        let mut ptr: Box<MmsgBuffer> = Box::new(unsafe { std::mem::zeroed() });

        let buffers = ptr.buffers.as_mut_ptr();

        ptr.iov.iter_mut().enumerate().for_each(|(index, iov)| {
            let buffer = unsafe { &mut *buffers.add(index) };
            *iov = libc::iovec {
                iov_base: buffer.as_mut_ptr() as *mut libc::c_void,
                iov_len: buffer.len(),
            }
        });

        let addrs = ptr.addr_storage.as_mut_ptr();
        let iov = ptr.iov.as_mut_ptr();

        ptr.mmsghdr.iter_mut().enumerate().for_each(|(index, h)| {
            *h = libc::mmsghdr {
                msg_hdr: libc::msghdr {
                    msg_name: unsafe { addrs.add(index) as *mut libc::c_void },
                    msg_namelen: SOCKADDR_STORAGE_LENGTH,
                    msg_iov: unsafe { iov.add(index) },
                    msg_iovlen: 1,
                    msg_control: std::ptr::null_mut(),
                    msg_controllen: 0,
                    msg_flags: 0,
                },
                msg_len: 0,
            }
        });

        ptr.nrecv = 0;

        ptr
    }

    fn init(&mut self) {
        self.mmsghdr.iter_mut().for_each(|h| {
            h.msg_hdr.msg_namelen = SOCKADDR_STORAGE_LENGTH;
        });
    }

    fn as_raw_mut(&mut self) -> &mut [libc::mmsghdr] {
        self.init();

        &mut self.mmsghdr
    }

    pub fn iter_mmsgs(&self) -> IterRecvMmsg {
        IterRecvMmsg {
            mmsg: self,
            nrecv: self.nrecv,
            current: 0,
        }
    }
}

pub struct IterRecvMmsg<'a> {
    mmsg: &'a MmsgBuffer,
    nrecv: u32,
    current: u32,
}

unsafe impl<'a> Send for IterRecvMmsg<'a> {}

impl<'a> Iterator for IterRecvMmsg<'a> {
    type Item = (SocketAddr, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.current == self.nrecv {
            return None;
        }

        let current = self.current as usize;
        self.current += 1;

        let msg = &self.mmsg.mmsghdr[current];

        let storage = &self.mmsg.addr_storage[current];
        let addr = unsafe {
            SockAddr::from_raw_parts(
                storage as *const libc::sockaddr_storage as *const _,
                msg.msg_hdr.msg_namelen,
            )
        };

        // println!("addr={:?} len={:?} namelen={:?}", addr, msg.msg_len, msg.msg_hdr.msg_namelen);

        let data = &self.mmsg.buffers[current][..msg.msg_len as usize];

        Some((addr.as_std().unwrap(), data))
    }
}

impl MyUdpSocket {
    pub async fn bind<A: ToSocketAddrs>(addr: A) -> tokio::io::Result<MyUdpSocket> {
        let addrs = lookup_host(addr).await?;
        let mut last_err = None;

        for addr in addrs {
            match MyUdpSocket::bind_addr(addr) {
                Ok(socket) => return Ok(socket),
                Err(e) => last_err = Some(e),
            }
        }

        Err(last_err.unwrap_or_else(|| {
            tokio::io::Error::new(
                tokio::io::ErrorKind::InvalidInput,
                "could not resolve to any address",
            )
        }))
    }

    fn bind_addr(addr: SocketAddr) -> tokio::io::Result<MyUdpSocket> {
        let domain = if addr.is_ipv4() {
            Domain::ipv4()
        } else {
            Domain::ipv6()
        };

        let socket = Socket::new(domain, Type::dgram(), Some(Protocol::udp())).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket.bind(&addr.into()).unwrap();

        MyUdpSocket::new(socket)
    }

    fn new(socket: Socket) -> tokio::io::Result<MyUdpSocket> {
        Ok(MyUdpSocket {
            inner: AsyncFd::new(socket)?,
            // sent: Cell::new(0)
        })
    }

    pub async fn writable(&self) -> tokio::io::Result<FdGuard<'_, Socket>> {
        self.inner.writable().await
    }

    pub fn poll_recv_from(
        &self,
        cx: &mut Context,
        buf: &mut MmsgBuffer,
    ) -> Poll<tokio::io::Result<()>> {
        loop {
            let mut ev = ready!(self.inner.poll_read_ready(cx))?;

            match self.recv_from(buf) {
                Err(ref e) if e.kind() == tokio::io::ErrorKind::WouldBlock => {
                    ev.clear_ready();
                }
                Err(e) => return Poll::Ready(Err(e)),
                Ok(_) => {
                    return Poll::Ready(Ok(()));
                }
            }
        }
    }

    pub fn poll_recvmmsg(
        &self,
        cx: &mut Context,
        mmsgs: &mut MmsgBuffer,
    ) -> Poll<tokio::io::Result<()>> {
        loop {
            let mut ev = ready!(self.inner.poll_read_ready(cx))?;

            match self.recvmmsg(mmsgs) {
                Err(ref e) if e.kind() == tokio::io::ErrorKind::WouldBlock => {
                    ev.clear_ready();
                }
                Err(e) => return Poll::Ready(Err(e)),
                ok => {
                    return Poll::Ready(ok);
                }
            }
        }
    }

    pub fn recv_from(&self, buffers: &mut MmsgBuffer) -> tokio::io::Result<()> {
        let fd = self.inner.get_ref().as_raw_fd();

        let storage = &mut buffers.addr_storage[0];
        let addrlen = &mut buffers.mmsghdr[0].msg_hdr.msg_namelen;
        let buffer = buffers.buffers[0].as_mut_ptr();
        let buffer_len = buffers.buffers[0].len();

        let result = unsafe {
            libc::recvfrom(
                fd,
                buffer as *mut libc::c_void,
                buffer_len,
                0,
                storage as *mut _ as *mut _,
                addrlen,
            )
        };

        if result == -1 {
            return Err(std::io::Error::last_os_error());
        }

        buffers.mmsghdr[0].msg_len = result as u32;
        buffers.nrecv = 1;

        Ok(())
    }

    fn recvmmsg(&self, buffers: &mut MmsgBuffer) -> tokio::io::Result<()> {
        let fd = self.inner.get_ref().as_raw_fd();

        let mmsghdr = buffers.as_raw_mut();

        let result = unsafe {
            libc::recvmmsg(
                fd,
                mmsghdr.as_mut_ptr(),
                mmsghdr.len() as u32,
                0,
                std::ptr::null_mut(),
            )
        };

        if result == -1 {
            // println!("Error recvmmsg {:?}", std::io::Error::last_os_error());
            return Err(std::io::Error::last_os_error());
        }

        buffers.nrecv = result as u32;

        // println!("recvmmsg {:?}", result);

        // if result != 1 {
        //     //println!("RESULT recvmmsg {:?}", result);
        // }

        Ok(())
    }

    // pub async fn send_to(
    //     &self,
    //     buf: &[u8], target: SocketAddr
    // ) -> tokio::io::Result<usize> {
    //     loop {
    //         let mut ev = self.inner.writable().await?;

    //         match self.inner.get_ref().send_to(buf, &SockAddr::from(target)) {
    //             Err(ref e) if e.kind() == tokio::io::ErrorKind::WouldBlock => {
    //                 ev.clear_ready();
    //             }
    //             x => return x,
    //         }
    //     }
    // }

    pub fn try_send_to(&self, buf: &[u8], target: SocketAddr) -> super::Result<()> {
        // println!("sending {} bytes", buf.len());
        self.inner
            .get_ref()
            .send_to(buf, &SockAddr::from(target))
            .map(|_| ())
            .map_err(|e| match e {
                e if e.kind() == ErrorKind::WouldBlock => UtpError::SendWouldBlock,
                e => UtpError::IO(e),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::MmsgBuffer;

    #[test]
    fn new_mmsg_buffers() {
        println!("LAAAAAAAMMM {:?}", std::mem::size_of::<MmsgBuffer>());
        MmsgBuffer::new();
    }
}
