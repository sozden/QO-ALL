//! RTL-SDR dongles, receive only: on USB through librtlsdr (loaded at run time, like
//! libiio: `rtlsdr.dll` on Windows, `librtlsdr.so.0` on Linux), or over the network from
//! an `rtl_tcp` server.
//!
//! The LNB's IF (about 739.5 MHz) is well inside an RTL-SDR's range. The samples are
//! 8-bit, offset by 127.5.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::OnceLock;
use std::thread::JoinHandle;
use std::time::Duration;

use libloading::Library;
use num_complex::Complex;

use crate::radio::{RxDevice, TxDevice};

/// The default rtl_tcp port.
pub const TCP_PORT: u16 = 1234;
/// Blocks of samples queued between the USB thread and the reader before new ones are
/// dropped (about 0.4 s at 2.4 MS/s).
const QUEUE: usize = 32;
/// Bytes per USB transfer: 16384 samples, about 7 ms at 2.4 MS/s.
const TRANSFER_BYTES: u32 = 32_768;
const TRANSFERS: u32 = 12;
/// Nothing arriving for this long means the radio is gone.
const READ_TIMEOUT: Duration = Duration::from_secs(3);

type Dev = *mut c_void;
type AsyncCb = unsafe extern "C" fn(*mut u8, u32, *mut c_void);

struct Api {
    get_device_count: unsafe extern "C" fn() -> u32,
    get_device_name: unsafe extern "C" fn(u32) -> *const c_char,
    get_device_usb_strings: unsafe extern "C" fn(u32, *mut c_char, *mut c_char, *mut c_char) -> c_int,
    open: unsafe extern "C" fn(*mut Dev, u32) -> c_int,
    close: unsafe extern "C" fn(Dev) -> c_int,
    set_center_freq: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_sample_rate: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_tuner_gain_mode: unsafe extern "C" fn(Dev, c_int) -> c_int,
    get_tuner_gains: unsafe extern "C" fn(Dev, *mut c_int) -> c_int,
    set_tuner_gain: unsafe extern "C" fn(Dev, c_int) -> c_int,
    reset_buffer: unsafe extern "C" fn(Dev) -> c_int,
    read_async: unsafe extern "C" fn(Dev, AsyncCb, *mut c_void, u32, u32) -> c_int,
    cancel_async: unsafe extern "C" fn(Dev) -> c_int,
    _lib: Library,
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref().map_err(|e| e.clone())
}

impl Api {
    fn load() -> Result<Self, String> {
        let names: &[&str] = if cfg!(windows) {
            &["rtlsdr.dll", "librtlsdr.dll"]
        } else if cfg!(target_os = "macos") {
            &["librtlsdr.dylib", "/opt/homebrew/lib/librtlsdr.dylib", "/usr/local/lib/librtlsdr.dylib"]
        } else {
            &["librtlsdr.so.0", "librtlsdr.so.2", "librtlsdr.so"]
        };
        // SAFETY: loading a library runs its initialisers; librtlsdr's are harmless.
        let lib = names.iter().find_map(|&n| unsafe { Library::new(n) }.ok()).ok_or("librtlsdr not found")?;
        // SAFETY: the symbols have these C signatures in rtl-sdr.h.
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("librtlsdr: {e}"))?
                };
            }
            Ok(Self {
                get_device_count: sym!("rtlsdr_get_device_count"),
                get_device_name: sym!("rtlsdr_get_device_name"),
                get_device_usb_strings: sym!("rtlsdr_get_device_usb_strings"),
                open: sym!("rtlsdr_open"),
                close: sym!("rtlsdr_close"),
                set_center_freq: sym!("rtlsdr_set_center_freq"),
                set_sample_rate: sym!("rtlsdr_set_sample_rate"),
                set_tuner_gain_mode: sym!("rtlsdr_set_tuner_gain_mode"),
                get_tuner_gains: sym!("rtlsdr_get_tuner_gains"),
                set_tuner_gain: sym!("rtlsdr_set_tuner_gain"),
                reset_buffer: sym!("rtlsdr_reset_buffer"),
                read_async: sym!("rtlsdr_read_async"),
                cancel_async: sym!("rtlsdr_cancel_async"),
                _lib: lib,
            })
        }
    }
}

/// Whether librtlsdr can be loaded (for diagnostics and the settings).
pub fn library_status() -> Result<(), String> {
    api().map(|_| ())
}

/// RTL-SDR dongles on USB, as (index, description).
pub fn devices() -> Vec<(u32, String)> {
    let Ok(api) = api() else { return Vec::new() };
    // SAFETY: plain queries by index; the strings are copied at once.
    unsafe {
        (0..(api.get_device_count)())
            .map(|i| {
                let name = text((api.get_device_name)(i));
                let (mut m, mut p, mut s) = ([0 as c_char; 256], [0 as c_char; 256], [0 as c_char; 256]);
                let usb = if (api.get_device_usb_strings)(i, m.as_mut_ptr(), p.as_mut_ptr(), s.as_mut_ptr()) == 0 {
                    format!(" · {} {} SN {}", text(m.as_ptr()), text(p.as_ptr()), text(s.as_ptr()))
                } else {
                    String::new()
                };
                (i, format!("{name}{usb}"))
            })
            .collect()
    }
}

/// # Safety
/// `p` is null or a NUL-terminated string.
unsafe fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().trim().to_string() }
}

/// 8-bit offset samples to complex ones; returns how many were written.
fn convert(bytes: &[u8], out: &mut [Complex<f32>]) -> usize {
    let n = (bytes.len() / 2).min(out.len());
    for (o, p) in out[..n].iter_mut().zip(bytes.chunks_exact(2)) {
        *o = Complex::new((p[0] as f32 - 127.5) / 128.0, (p[1] as f32 - 127.5) / 128.0);
    }
    n
}

/// Samples waiting to be handed out: a block and how far it has been read.
#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    pos: usize,
}

impl Pending {
    fn take(&mut self, out: &mut [Complex<f32>]) -> usize {
        let n = convert(&self.bytes[self.pos..], out);
        self.pos += 2 * n;
        n
    }

    fn is_empty(&self) -> bool {
        self.bytes.len() - self.pos < 2
    }
}

/// The device handle, moved to the USB thread.
struct Handle(Dev);
// SAFETY: librtlsdr lets one thread run read_async while another changes the tuning and
// cancels it; the handle is closed only after that thread has ended.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

/// An RTL-SDR on USB.
pub struct RtlUsb {
    api: &'static Api,
    dev: std::sync::Arc<Handle>,
    index: u32,
    name: String,
    gains: Vec<c_int>,
    blocks: Receiver<Vec<u8>>,
    pending: Pending,
    thread: Option<JoinHandle<()>>,
}

unsafe extern "C" fn on_samples(buf: *mut u8, len: u32, ctx: *mut c_void) {
    if buf.is_null() || ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the sender boxed in `open`, alive until the thread has ended;
    // `buf` holds `len` bytes for the duration of the call.
    let (sink, bytes) = unsafe { (&*(ctx as *const SyncSender<Vec<u8>>), std::slice::from_raw_parts(buf, len as usize)) };
    let _ = sink.try_send(bytes.to_vec());
}

impl RtlUsb {
    pub fn open(index: u32) -> Result<Self, String> {
        let api = api()?;
        let mut dev: Dev = std::ptr::null_mut();
        // SAFETY: `dev` receives the handle.
        let r = unsafe { (api.open)(&mut dev, index) };
        if r < 0 || dev.is_null() {
            return Err(format!("RTL-SDR #{index}: could not open it ({r}); on Windows it needs the WinUSB driver (Zadig)"));
        }
        // SAFETY: `dev` is open; the gain list is read in two calls as the API asks.
        let gains = unsafe {
            let n = (api.get_tuner_gains)(dev, std::ptr::null_mut()).max(0) as usize;
            let mut g = vec![0; n];
            if n > 0 {
                (api.get_tuner_gains)(dev, g.as_mut_ptr());
            }
            g
        };
        let name = devices().into_iter().find(|(i, _)| *i == index).map_or_else(|| "RTL-SDR".to_string(), |(_, d)| d);
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        Ok(Self { api, dev: std::sync::Arc::new(Handle(dev)), index, name, gains, blocks: rx, pending: Pending::default(), thread: Some(Self::spawn(api, dev, tx)) })
    }

    fn spawn(api: &'static Api, dev: Dev, sink: SyncSender<Vec<u8>>) -> JoinHandle<()> {
        let dev = Handle(dev);
        std::thread::Builder::new()
            .name("rtlsdr".into())
            .spawn(move || {
                let dev = dev;
                let ctx = Box::into_raw(Box::new(sink));
                // SAFETY: the handle is open; read_async returns once cancelled, after
                // which the callback is no longer called and the box can go.
                unsafe {
                    (api.reset_buffer)(dev.0);
                    (api.read_async)(dev.0, on_samples, ctx as *mut c_void, TRANSFERS, TRANSFER_BYTES);
                    drop(Box::from_raw(ctx));
                }
            })
            .expect("rtlsdr thread")
    }
}

impl RxDevice for RtlUsb {
    fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String> {
        if self.pending.is_empty() {
            match self.blocks.recv_timeout(READ_TIMEOUT) {
                Ok(b) => self.pending = Pending { bytes: b, pos: 0 },
                Err(RecvTimeoutError::Timeout) => return Err("RTL-SDR: no samples (unplugged?)".into()),
                Err(RecvTimeoutError::Disconnected) => return Err("RTL-SDR: the USB transfer stopped".into()),
            }
        }
        Ok(self.pending.take(out))
    }

    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        // SAFETY: the handle is open.
        let r = unsafe { (self.api.set_center_freq)(self.dev.0, hz.round() as u32) };
        if r < 0 { Err(format!("RTL-SDR: {:.3} MHz not accepted ({r})", hz / 1e6)) } else { Ok(()) }
    }

    fn set_sample_rate(&mut self, rate: f64) -> Result<(), String> {
        // SAFETY: as above.
        let r = unsafe { (self.api.set_sample_rate)(self.dev.0, rate.round() as u32) };
        if r < 0 { Err(format!("RTL-SDR: {:.2} MS/s not accepted ({r})", rate / 1e6)) } else { Ok(()) }
    }

    fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String> {
        // SAFETY: as above.
        unsafe {
            match gain {
                None => {
                    (self.api.set_tuner_gain_mode)(self.dev.0, 0);
                }
                Some(db) => {
                    (self.api.set_tuner_gain_mode)(self.dev.0, 1);
                    let want = (db * 10.0).round() as c_int;
                    let g = self.gains.iter().copied().min_by_key(|g| (g - want).abs()).unwrap_or(want);
                    (self.api.set_tuner_gain)(self.dev.0, g);
                }
            }
        }
        Ok(())
    }

    fn model(&self) -> String {
        format!("{} · USB #{}", self.name, self.index)
    }

    fn transmitter(&self) -> Result<Box<dyn TxDevice>, String> {
        Err("RTL-SDR: receive only".into())
    }
}

impl Drop for RtlUsb {
    fn drop(&mut self) {
        // SAFETY: cancelling ends read_async; the handle is closed after its thread.
        unsafe { (self.api.cancel_async)(self.dev.0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe { (self.api.close)(self.dev.0) };
    }
}

/// An RTL-SDR behind an rtl_tcp server ("host" or "host:port").
pub struct RtlTcp {
    stream: TcpStream,
    address: String,
    tuner: String,
    buf: Vec<u8>,
    /// An odd byte left over from the last read (half a sample).
    carry: Option<u8>,
}

impl RtlTcp {
    pub fn open(address: &str) -> Result<Self, String> {
        let address = address.trim();
        let with_port = if address.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { address.to_string() } else { format!("{address}:{TCP_PORT}") };
        let err = |e: std::io::Error| format!("rtl_tcp {with_port}: {e}");
        let addr = with_port.to_socket_addrs().map_err(err)?.next().ok_or_else(|| format!("rtl_tcp {with_port}: no address"))?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(err)?;
        stream.set_read_timeout(Some(READ_TIMEOUT)).map_err(err)?;
        stream.set_nodelay(true).map_err(err)?;
        // The server greets with "RTL0", the tuner type and its number of gains.
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).map_err(err)?;
        if &head[..4] != b"RTL0" {
            return Err(format!("rtl_tcp {with_port}: not an rtl_tcp server"));
        }
        let tuner = match u32::from_be_bytes([head[4], head[5], head[6], head[7]]) {
            1 => "E4000",
            2 => "FC0012",
            3 => "FC0013",
            4 => "FC2580",
            5 => "R820T",
            6 => "R828D",
            _ => "unknown tuner",
        };
        Ok(Self { stream, address: with_port, tuner: tuner.into(), buf: vec![0; 65_536], carry: None })
    }

    fn command(&mut self, cmd: u8, value: u32) -> Result<(), String> {
        let mut m = [cmd, 0, 0, 0, 0];
        m[1..].copy_from_slice(&value.to_be_bytes());
        self.stream.write_all(&m).map_err(|e| format!("rtl_tcp {}: {e}", self.address))
    }
}

impl RxDevice for RtlTcp {
    fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String> {
        let want = (out.len() * 2).min(self.buf.len());
        let start = usize::from(self.carry.is_some());
        if let Some(b) = self.carry.take() {
            self.buf[0] = b;
        }
        let n = match self.stream.read(&mut self.buf[start..want.max(start + 1)]) {
            Ok(0) => return Err(format!("rtl_tcp {}: the server closed the connection", self.address)),
            Ok(n) => n + start,
            Err(e) => return Err(format!("rtl_tcp {}: {e}", self.address)),
        };
        if n % 2 == 1 {
            self.carry = Some(self.buf[n - 1]);
        }
        Ok(convert(&self.buf[..n & !1], out))
    }

    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        self.command(0x01, hz.round() as u32)
    }

    fn set_sample_rate(&mut self, rate: f64) -> Result<(), String> {
        self.command(0x02, rate.round() as u32)
    }

    fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String> {
        match gain {
            None => self.command(0x03, 0),
            Some(db) => {
                self.command(0x03, 1)?;
                // The tuner takes the nearest gain it has (tenths of a dB).
                self.command(0x04, (db * 10.0).round().max(0.0) as u32)
            }
        }
    }

    fn model(&self) -> String {
        format!("RTL-SDR {} · rtl_tcp {}", self.tuner, self.address)
    }

    fn transmitter(&self) -> Result<Box<dyn TxDevice>, String> {
        Err("RTL-SDR: receive only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_become_samples() {
        let mut out = [Complex::default(); 3];
        assert_eq!(convert(&[255, 0, 128, 127, 1], &mut out), 2);
        assert!((out[0].re - 0.996).abs() < 0.01 && (out[0].im + 0.996).abs() < 0.01);
        assert!(out[1].re.abs() < 0.01 && out[1].im.abs() < 0.01);
    }

    /// An rtl_tcp server on the loopback: greeting, commands and samples (an odd count
    /// in one read must not shift I and Q).
    #[test]
    fn talks_to_an_rtl_tcp_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = b"RTL0".to_vec();
            head.extend(5u32.to_be_bytes());
            head.extend(29u32.to_be_bytes());
            s.write_all(&head).unwrap();
            let mut cmd = [0u8; 5];
            s.read_exact(&mut cmd).unwrap();
            assert_eq!(cmd, [0x01, 0x2C, 0x13, 0xE6, 0x48]);
            s.write_all(&[255, 0, 255]).unwrap();
            s.flush().unwrap();
            std::thread::sleep(Duration::from_millis(50));
            s.write_all(&[0]).unwrap();
        });
        let mut r = RtlTcp::open(&format!("127.0.0.1:{port}")).unwrap();
        assert!(r.model().contains("R820T"));
        r.set_frequency(739_501_640.0).unwrap();
        let mut out = [Complex::default(); 4];
        let mut got = Vec::new();
        while got.len() < 2 {
            let n = r.read(&mut out).unwrap();
            got.extend_from_slice(&out[..n]);
        }
        assert!(got.iter().all(|c| c.re > 0.9 && c.im < -0.9), "{got:?}");
        server.join().unwrap();
    }
}
