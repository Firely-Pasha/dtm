//! Looking into other processes (working directory, name, command line, process
//! groups): `/proc` on Linux, libproc and sysctl on macOS.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod imp {
    use super::PathBuf;

    pub fn cwd(pid: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }

    pub fn name(pid: i32) -> Option<String> {
        Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string())
    }

    pub fn cmdline(pid: i32) -> Option<Vec<String>> {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        Some(raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
    }

    /// Names of the live (not stopped, dead or zombie) members of a process group.
    pub fn group_commands(pgrp: i32) -> Vec<String> {
        crate::procinfo::group_commands(pgrp)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::os::unix::ffi::OsStrExt;

    use super::PathBuf;

    pub fn cwd(pid: i32) -> Option<PathBuf> {
        // SAFETY: proc_pidinfo fills at most `size` bytes of a zeroed plain struct.
        let (info, n, size) = unsafe {
            let mut info: libc::proc_vnodepathinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as c_int;
            let n = libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut info).cast::<c_void>(), size);
            (info, n, size)
        };
        if n < size {
            return None;
        }
        // SAFETY: the kernel NUL-terminates the path inside the struct.
        let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast::<c_char>()) };
        (!path.to_bytes().is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
    }

    pub fn name(pid: i32) -> Option<String> {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is writable for its whole length.
        let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast::<c_void>(), buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    pub fn cmdline(pid: i32) -> Option<Vec<String>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: usize = 0;
        // SAFETY: the first call only asks how large the answer is.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        // SAFETY: `buf` is `size` bytes long.
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast::<c_void>(), &mut size, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        buf.truncate(size);
        parse_procargs(&buf)
    }

    /// `KERN_PROCARGS2`: argc, the executable path, NUL padding, then argv.
    pub fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
        let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
        let rest = &buf[4..];
        let after_path = rest.iter().position(|&b| b == 0)?;
        let rest = &rest[after_path..];
        let start = rest.iter().position(|&b| b != 0)?;
        let args: Vec<String> = rest[start..]
            .split(|&b| b == 0)
            .take(argc.max(0) as usize)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        (!args.is_empty()).then_some(args)
    }

    pub fn group_commands(pgrp: i32) -> Vec<String> {
        const MAX: usize = 256;
        let mut pids = [0 as libc::pid_t; MAX];
        // SAFETY: the buffer is writable for `MAX` pids; unused entries stay 0.
        unsafe {
            libc::proc_listpgrppids(pgrp, pids.as_mut_ptr().cast::<c_void>(), std::mem::size_of_val(&pids) as c_int);
        }
        pids.iter()
            .take_while(|&&p| p > 0)
            .filter(|&&pid| !stopped_or_dead(pid))
            .filter_map(|&pid| name(pid))
            .collect()
    }

    fn stopped_or_dead(pid: i32) -> bool {
        // SAFETY: proc_pidinfo fills at most `size` bytes of a zeroed plain struct.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
            let n = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast::<c_void>(), size);
            n < size || matches!(info.pbi_status, libc::SSTOP | libc::SZOMB)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_procargs() {
            let mut buf = 3i32.to_ne_bytes().to_vec();
            buf.extend_from_slice(b"/usr/bin/sleep\0\0\0sleep\0 10\0x\0HOME=/root\0");
            assert_eq!(parse_procargs(&buf), Some(vec!["sleep".into(), " 10".into(), "x".into()]));
            assert_eq!(parse_procargs(&[1, 0]), None);
        }

        #[test]
        fn looks_at_itself() {
            let me = std::process::id() as i32;
            assert_eq!(cwd(me), std::env::current_dir().ok().and_then(|d| d.canonicalize().ok()));
            assert!(name(me).is_some());
            assert!(cmdline(me).is_some_and(|a| !a.is_empty()));
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use super::PathBuf;
    pub fn cwd(_: i32) -> Option<PathBuf> {
        None
    }
    pub fn name(_: i32) -> Option<String> {
        None
    }
    pub fn cmdline(_: i32) -> Option<Vec<String>> {
        None
    }
    pub fn group_commands(_: i32) -> Vec<String> {
        Vec::new()
    }
}

pub use imp::{cmdline, cwd, group_commands, name};
