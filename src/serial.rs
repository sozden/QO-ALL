//! Serial ports for their control lines only: DTR or RTS out (PTT for an amplifier,
//! sequencer or another radio) and CTS, DSR and DCD in (a CW key or paddle). No data is
//! sent. Both lines out are held low when the port opens, so nothing keys by itself.
//! (After SeDiR's serial port.)

use std::fs::File;
use std::io;

/// The modem lines read from a port.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub cts: bool,
    pub dsr: bool,
    pub dcd: bool,
}

/// An open port.
pub struct Port {
    file: File,
    pub name: String,
}

pub use imp::ports;

impl Port {
    pub fn open(name: &str) -> io::Result<Port> {
        Ok(Port { file: imp::open(name)?, name: name.to_string() })
    }

    pub fn set_dtr(&self, on: bool) -> io::Result<()> {
        imp::set_line(&self.file, imp::Line::Dtr, on)
    }

    pub fn set_rts(&self, on: bool) -> io::Result<()> {
        imp::set_line(&self.file, imp::Line::Rts, on)
    }

    pub fn inputs(&self) -> io::Result<Inputs> {
        imp::inputs(&self.file)
    }
}

impl Drop for Port {
    fn drop(&mut self) {
        // Leave nothing keyed.
        let _ = self.set_dtr(false);
        let _ = self.set_rts(false);
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    use super::Inputs;

    pub enum Line {
        Dtr,
        Rts,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Dcb {
        dcb_length: u32,
        baud_rate: u32,
        /// fBinary, fParity, flow control, fDtrControl, fRtsControl… as bits.
        flags: u32,
        w_reserved: u16,
        xon_lim: u16,
        xoff_lim: u16,
        byte_size: u8,
        parity: u8,
        stop_bits: u8,
        xon_char: i8,
        xoff_char: i8,
        error_char: i8,
        eof_char: i8,
        evt_char: i8,
        w_reserved1: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCommState(h: *mut c_void, dcb: *mut Dcb) -> i32;
        fn SetCommState(h: *mut c_void, dcb: *const Dcb) -> i32;
        fn EscapeCommFunction(h: *mut c_void, func: u32) -> i32;
        fn GetCommModemStatus(h: *mut c_void, status: *mut u32) -> i32;
        fn QueryDosDeviceW(name: *const u16, target: *mut u16, max: u32) -> u32;
    }

    const SETRTS: u32 = 3;
    const CLRRTS: u32 = 4;
    const SETDTR: u32 = 5;
    const CLRDTR: u32 = 6;
    const MS_CTS_ON: u32 = 0x10;
    const MS_DSR_ON: u32 = 0x20;
    const MS_RLSD_ON: u32 = 0x80;
    /// Only fBinary: no parity, no flow control, DTR and RTS disabled (held low).
    const FLAGS_BINARY: u32 = 1;

    fn check(ok: i32) -> io::Result<()> {
        if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    fn handle(f: &File) -> *mut c_void {
        f.as_raw_handle() as *mut c_void
    }

    /// `name` is e.g. "COM5".
    pub fn open(name: &str) -> io::Result<File> {
        let file = OpenOptions::new().read(true).write(true).share_mode(0).open(format!(r"\\.\{name}"))?;
        let h = handle(&file);
        // SAFETY: `h` is the open port's handle; the structure is laid out as in winbase.h.
        unsafe {
            let mut dcb = Dcb { dcb_length: std::mem::size_of::<Dcb>() as u32, ..Default::default() };
            check(GetCommState(h, &mut dcb))?;
            dcb.baud_rate = 9600;
            dcb.flags = FLAGS_BINARY;
            dcb.byte_size = 8;
            dcb.parity = 0;
            dcb.stop_bits = 0;
            check(SetCommState(h, &dcb))?;
            check(EscapeCommFunction(h, CLRRTS))?;
            check(EscapeCommFunction(h, CLRDTR))?;
        }
        Ok(file)
    }

    pub fn set_line(f: &File, line: Line, on: bool) -> io::Result<()> {
        let func = match (line, on) {
            (Line::Dtr, true) => SETDTR,
            (Line::Dtr, false) => CLRDTR,
            (Line::Rts, true) => SETRTS,
            (Line::Rts, false) => CLRRTS,
        };
        // SAFETY: the handle is open.
        check(unsafe { EscapeCommFunction(handle(f), func) })
    }

    pub fn inputs(f: &File) -> io::Result<Inputs> {
        let mut s = 0u32;
        // SAFETY: as above.
        check(unsafe { GetCommModemStatus(handle(f), &mut s) })?;
        Ok(Inputs { cts: s & MS_CTS_ON != 0, dsr: s & MS_DSR_ON != 0, dcd: s & MS_RLSD_ON != 0 })
    }

    /// The serial ports, as (name, the driver behind it), e.g. ("COM5", "Silabser0").
    pub fn ports() -> Vec<(String, String)> {
        let mut buf = vec![0u16; 1 << 18];
        // SAFETY: the buffer holds `len` u16s; a null name lists every DOS device name.
        let n = unsafe { QueryDosDeviceW(std::ptr::null(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let names: Vec<String> = buf[..n].split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
        let mut ports: Vec<(String, String)> = names
            .into_iter()
            .filter(|n| n.starts_with("COM") && n.len() > 3 && n[3..].chars().all(|c| c.is_ascii_digit()))
            .map(|name| {
                let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
                let mut target = vec![0u16; 1024];
                // SAFETY: as above, for one name.
                let n = unsafe { QueryDosDeviceW(wide.as_ptr(), target.as_mut_ptr(), target.len() as u32) } as usize;
                let target = String::from_utf16_lossy(&target[..n]).trim_end_matches('\0').to_string();
                let driver = target.rsplit('\\').next().unwrap_or_default().to_string();
                (name, driver)
            })
            .collect();
        ports.sort_by_key(|(n, _)| n[3..].parse::<u32>().unwrap_or(0));
        ports
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    use super::Inputs;

    pub enum Line {
        Dtr,
        Rts,
    }

    unsafe extern "C" {
        fn ioctl(fd: i32, request: u64, ...) -> i32;
    }

    const O_NOCTTY: i32 = 0o400;
    const O_NONBLOCK: i32 = 0o4000;
    const TIOCMGET: u64 = 0x5415;
    const TIOCMBIS: u64 = 0x5416;
    const TIOCMBIC: u64 = 0x5417;
    const TIOCM_DTR: i32 = 0x002;
    const TIOCM_RTS: i32 = 0x004;
    const TIOCM_CTS: i32 = 0x020;
    const TIOCM_CAR: i32 = 0x040;
    const TIOCM_DSR: i32 = 0x100;

    fn check(r: i32) -> io::Result<()> {
        if r >= 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    /// `name` is a device path, e.g. "/dev/ttyUSB0".
    pub fn open(name: &str) -> io::Result<File> {
        let file = OpenOptions::new().read(true).write(true).custom_flags(O_NOCTTY | O_NONBLOCK).open(name)?;
        // SAFETY: the descriptor is open; the argument points to an int.
        check(unsafe { ioctl(file.as_raw_fd(), TIOCMBIC, &(TIOCM_DTR | TIOCM_RTS) as *const i32) })?;
        Ok(file)
    }

    pub fn set_line(f: &File, line: Line, on: bool) -> io::Result<()> {
        let bit = match line {
            Line::Dtr => TIOCM_DTR,
            Line::Rts => TIOCM_RTS,
        };
        // SAFETY: as above.
        check(unsafe { ioctl(f.as_raw_fd(), if on { TIOCMBIS } else { TIOCMBIC }, &bit as *const i32) })
    }

    pub fn inputs(f: &File) -> io::Result<Inputs> {
        let mut s: i32 = 0;
        // SAFETY: as above; the result goes into `s`.
        check(unsafe { ioctl(f.as_raw_fd(), TIOCMGET, &mut s as *mut i32) })?;
        Ok(Inputs { cts: s & TIOCM_CTS != 0, dsr: s & TIOCM_DSR != 0, dcd: s & TIOCM_CAR != 0 })
    }

    /// USB serial ports (adapters, radios' USB links), as (device path, name).
    pub fn ports() -> Vec<(String, String)> {
        let mut ports: Vec<(String, String)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/dev/serial/by-id") {
            for e in entries.flatten() {
                if let Ok(path) = std::fs::canonicalize(e.path()) {
                    ports.push((path.to_string_lossy().to_string(), e.file_name().to_string_lossy().to_string()));
                }
            }
        }
        // Built-in ports have no by-id name.
        for i in 0..4 {
            let p = format!("/dev/ttyS{i}");
            if std::path::Path::new(&p).exists() && !ports.iter().any(|(n, _)| *n == p) {
                ports.push((p, String::new()));
            }
        }
        ports.sort();
        ports
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use std::fs::File;
    use std::io;

    use super::Inputs;

    pub enum Line {
        Dtr,
        Rts,
    }

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "serial ports are not supported here")
    }

    pub fn open(_name: &str) -> io::Result<File> {
        Err(unsupported())
    }

    pub fn set_line(_f: &File, _line: Line, _on: bool) -> io::Result<()> {
        Err(unsupported())
    }

    pub fn inputs(_f: &File) -> io::Result<Inputs> {
        Err(unsupported())
    }

    pub fn ports() -> Vec<(String, String)> {
        Vec::new()
    }
}
