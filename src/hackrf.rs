//! HackRF One, receive only (it is half duplex, so it cannot listen to its own uplink),
//! through libhackrf loaded at run time (`hackrf.dll` on Windows, `libhackrf.so.0` on
//! Linux). Samples are signed 8-bit.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use libloading::Library;
use num_complex::Complex;

use crate::radio::{RxDevice, TxDevice};

const QUEUE: usize = 32;
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// Gain without automatic control: the LNA and VGA steps (dB).
const DEFAULT_LNA: u32 = 24;
const DEFAULT_VGA: u32 = 20;

type Dev = *mut c_void;

/// `hackrf_transfer` from hackrf.h (only the leading fields are read).
#[repr(C)]
struct Transfer {
    device: Dev,
    buffer: *mut u8,
    buffer_length: c_int,
    valid_length: c_int,
    rx_ctx: *mut c_void,
    tx_ctx: *mut c_void,
}

/// `hackrf_device_list_t`.
#[repr(C)]
struct DeviceList {
    serial_numbers: *mut *mut c_char,
    usb_board_ids: *mut c_int,
    usb_device_index: *mut c_int,
    devicecount: c_int,
    usb_devices: *mut *mut c_void,
    usb_devicecount: c_int,
}

type RxCb = unsafe extern "C" fn(*mut Transfer) -> c_int;

struct Api {
    init: unsafe extern "C" fn() -> c_int,
    device_list: unsafe extern "C" fn() -> *mut DeviceList,
    device_list_free: unsafe extern "C" fn(*mut DeviceList),
    open_by_serial: unsafe extern "C" fn(*const c_char, *mut Dev) -> c_int,
    close: unsafe extern "C" fn(Dev) -> c_int,
    set_freq: unsafe extern "C" fn(Dev, u64) -> c_int,
    set_sample_rate: unsafe extern "C" fn(Dev, f64) -> c_int,
    compute_bw: unsafe extern "C" fn(u32) -> u32,
    set_bw: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_lna_gain: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_vga_gain: unsafe extern "C" fn(Dev, u32) -> c_int,
    set_amp_enable: unsafe extern "C" fn(Dev, u8) -> c_int,
    start_rx: unsafe extern "C" fn(Dev, RxCb, *mut c_void) -> c_int,
    stop_rx: unsafe extern "C" fn(Dev) -> c_int,
    error_name: unsafe extern "C" fn(c_int) -> *const c_char,
    _lib: Library,
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let api = Api::load()?;
        // SAFETY: initialises libusb inside libhackrf; done once per run.
        let r = unsafe { (api.init)() };
        if r != 0 { Err(format!("libhackrf: init failed ({r})")) } else { Ok(api) }
    })
    .as_ref()
    .map_err(|e| e.clone())
}

impl Api {
    fn load() -> Result<Self, String> {
        let names: &[&str] = if cfg!(windows) {
            &["hackrf.dll", "libhackrf.dll"]
        } else if cfg!(target_os = "macos") {
            &["libhackrf.dylib", "/opt/homebrew/lib/libhackrf.dylib", "/usr/local/lib/libhackrf.dylib"]
        } else {
            &["libhackrf.so.0", "libhackrf.so"]
        };
        // SAFETY: loading the library runs only its own initialisers.
        let lib = names.iter().find_map(|&n| unsafe { Library::new(n) }.ok()).ok_or("libhackrf not found")?;
        // SAFETY: the symbols have these C signatures in hackrf.h.
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("libhackrf: {e}"))?
                };
            }
            Ok(Self {
                init: sym!("hackrf_init"),
                device_list: sym!("hackrf_device_list"),
                device_list_free: sym!("hackrf_device_list_free"),
                open_by_serial: sym!("hackrf_open_by_serial"),
                close: sym!("hackrf_close"),
                set_freq: sym!("hackrf_set_freq"),
                set_sample_rate: sym!("hackrf_set_sample_rate"),
                compute_bw: sym!("hackrf_compute_baseband_filter_bw"),
                set_bw: sym!("hackrf_set_baseband_filter_bandwidth"),
                set_lna_gain: sym!("hackrf_set_lna_gain"),
                set_vga_gain: sym!("hackrf_set_vga_gain"),
                set_amp_enable: sym!("hackrf_set_amp_enable"),
                start_rx: sym!("hackrf_start_rx"),
                stop_rx: sym!("hackrf_stop_rx"),
                error_name: sym!("hackrf_error_name"),
                _lib: lib,
            })
        }
    }

    fn check(&self, what: &str, r: c_int) -> Result<(), String> {
        if r == 0 {
            return Ok(());
        }
        // SAFETY: hackrf_error_name returns a static string.
        let name = unsafe { CStr::from_ptr((self.error_name)(r)) }.to_string_lossy().to_string();
        Err(format!("HackRF {what}: {name} ({r})"))
    }
}

pub fn library_status() -> Result<(), String> {
    api().map(|_| ())
}

/// Serial numbers of the HackRFs on USB.
pub fn devices() -> Vec<String> {
    let Ok(api) = api() else { return Vec::new() };
    // SAFETY: the list is read and then freed by the library.
    unsafe {
        let list = (api.device_list)();
        if list.is_null() {
            return Vec::new();
        }
        let l = &*list;
        let serials = (0..l.devicecount.max(0) as usize)
            .map(|i| {
                let p = *l.serial_numbers.add(i);
                if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().to_string() }
            })
            .collect();
        (api.device_list_free)(list);
        serials
    }
}

unsafe extern "C" fn on_samples(t: *mut Transfer) -> c_int {
    // SAFETY: libhackrf passes a valid transfer whose rx_ctx is the sender boxed in
    // `open`, alive until the device is closed.
    unsafe {
        let t = &*t;
        if !t.rx_ctx.is_null() && !t.buffer.is_null() {
            let sink = &*(t.rx_ctx as *const SyncSender<Vec<u8>>);
            let bytes = std::slice::from_raw_parts(t.buffer, t.valid_length.max(0) as usize);
            let _ = sink.try_send(bytes.to_vec());
        }
    }
    0
}

pub struct HackRf {
    api: &'static Api,
    dev: Dev,
    serial: String,
    ctx: *mut SyncSender<Vec<u8>>,
    blocks: Receiver<Vec<u8>>,
    pending: Vec<u8>,
    pos: usize,
    started: bool,
}

// SAFETY: the device handle is used by one thread at a time (the receiver's); the
// library's own transfer thread only calls the callback.
unsafe impl Send for HackRf {}

impl HackRf {
    /// The HackRF with this serial number, or the first one.
    pub fn open(serial: Option<&str>) -> Result<Self, String> {
        let api = api()?;
        let c = serial.filter(|s| !s.is_empty()).map(|s| CString::new(s).map_err(|e| e.to_string())).transpose()?;
        let mut dev: Dev = std::ptr::null_mut();
        // SAFETY: a null serial opens the first device.
        let r = unsafe { (api.open_by_serial)(c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()), &mut dev) };
        api.check("open", r)?;
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let serial = serial.map(str::to_string).or_else(|| devices().into_iter().next()).unwrap_or_default();
        let mut me = Self { api, dev, serial, ctx: Box::into_raw(Box::new(tx)), blocks: rx, pending: Vec::new(), pos: 0, started: false };
        // The amplifier stays off: an LNB gives more than enough.
        // SAFETY: the device is open.
        unsafe { (api.set_amp_enable)(dev, 0) };
        me.set_gain(None)?;
        Ok(me)
    }

    fn start(&mut self) -> Result<(), String> {
        if !self.started {
            // SAFETY: the context outlives the transfers (stopped in drop).
            let r = unsafe { (self.api.start_rx)(self.dev, on_samples, self.ctx as *mut c_void) };
            self.api.check("start", r)?;
            self.started = true;
        }
        Ok(())
    }
}

impl RxDevice for HackRf {
    fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String> {
        self.start()?;
        if self.pending.len() - self.pos < 2 {
            match self.blocks.recv_timeout(READ_TIMEOUT) {
                Ok(b) => (self.pending, self.pos) = (b, 0),
                Err(RecvTimeoutError::Timeout) => return Err("HackRF: no samples (unplugged?)".into()),
                Err(RecvTimeoutError::Disconnected) => return Err("HackRF: the transfer stopped".into()),
            }
        }
        let n = ((self.pending.len() - self.pos) / 2).min(out.len());
        for (o, p) in out[..n].iter_mut().zip(self.pending[self.pos..].chunks_exact(2)) {
            *o = Complex::new(p[0] as i8 as f32 / 128.0, p[1] as i8 as f32 / 128.0);
        }
        self.pos += 2 * n;
        Ok(n)
    }

    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        // SAFETY: the device is open.
        self.api.check("frequency", unsafe { (self.api.set_freq)(self.dev, hz.round() as u64) })
    }

    fn set_sample_rate(&mut self, rate: f64) -> Result<(), String> {
        // SAFETY: as above; the baseband filter follows the rate.
        unsafe {
            self.api.check("sample rate", (self.api.set_sample_rate)(self.dev, rate))?;
            let bw = (self.api.compute_bw)((rate * 0.75) as u32);
            self.api.check("filter", (self.api.set_bw)(self.dev, bw))
        }
    }

    fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String> {
        // One figure split over the LNA (0–40 dB, 8 dB steps) and the VGA (0–62, 2 dB).
        let (lna, vga) = match gain {
            None => (DEFAULT_LNA, DEFAULT_VGA),
            Some(db) => {
                let db = db.clamp(0.0, 102.0) as u32;
                let lna = (db.min(40) / 8) * 8;
                (lna, ((db - lna).min(62) / 2) * 2)
            }
        };
        // SAFETY: the device is open.
        unsafe {
            self.api.check("LNA gain", (self.api.set_lna_gain)(self.dev, lna))?;
            self.api.check("VGA gain", (self.api.set_vga_gain)(self.dev, vga))
        }
    }

    fn model(&self) -> String {
        if self.serial.is_empty() { "HackRF One".into() } else { format!("HackRF One · SN …{}", &self.serial[self.serial.len().saturating_sub(8)..]) }
    }

    fn transmitter(&self) -> Result<Box<dyn TxDevice>, String> {
        Err("HackRF: receive only here (it is half duplex)".into())
    }
}

impl Drop for HackRf {
    fn drop(&mut self) {
        // SAFETY: stopping ends the callbacks before the context goes.
        unsafe {
            if self.started {
                (self.api.stop_rx)(self.dev);
            }
            (self.api.close)(self.dev);
            drop(Box::from_raw(self.ctx));
        }
    }
}
