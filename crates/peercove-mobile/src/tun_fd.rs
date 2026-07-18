//! VpnService から渡される TUN fd の入出力(Android / Unix)。
//!
//! Android では Kotlin 側が `Builder.establish()` → `ParcelFileDescriptor.detachFd()`
//! で fd の所有権を切り離してから Rust へ渡す。以後の所有者はこちら
//! (`OwnedFd` の drop で close)。

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use crate::engine::TunIo;

pub struct FdTun {
    fd: OwnedFd,
}

impl FdTun {
    /// 所有権を移譲された fd を受け取る(Android は `detachFd()` の戻り値)。
    /// 前提: `raw_fd` は open 済みで、他に所有者がいない(drop で close される)。
    pub fn from_raw(raw_fd: i32) -> FdTun {
        // unsafe: OS API 境界。fd の所有権は Kotlin 側の detachFd() で移譲済みで、
        // 二重 close の可能性はない
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        FdTun { fd }
    }
}

impl TunIo for FdTun {
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let mut pfd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // unsafe: OS API 境界(タイムアウト付き read は poll + read でしか書けない)
        let ready = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if ready == 0 {
            return Ok(0); // タイムアウト(TunIo の規約: データなしは Ok(0))
        }
        // unsafe: OS API 境界。buf は呼び出し側所有の有効なバッファ
        let n = unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn write(&self, buf: &[u8]) -> io::Result<()> {
        // unsafe: OS API 境界。buf は有効な読み取り専用バッファ
        let n = unsafe {
            libc::write(
                self.fd.as_raw_fd(),
                buf.as_ptr() as *const libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}
