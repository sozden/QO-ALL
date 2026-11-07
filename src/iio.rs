//! AD9361/AD9363-based transceivers over libiio (ADALM-Pluto, LibreSDR and similar).
//!
//! One libiio context is shared by the receiving and the transmitting half, which run
//! on their own threads: their sample buffers are independent, attribute access is
//! serialised.
//!
//! libiio is loaded at run time: if it is not installed the program still starts and
//! only these devices are unavailable. The libiio 0.x C API is used (on Windows the
//! `libiio.dll` that comes with PothosSDR, on Linux `libiio.so.0`).

use std::ffi::{CStr, CString, c_char, c_uint, c_void};
use std::sync::{Arc, Mutex};

use libloading::Library;
use num_complex::Complex;

/// Samples per receive transfer: about 40 ms of signal. Over the network every transfer
/// is a round trip, so high rates need large buffers to keep up.
fn buffer_samples(rate: f64) -> usize {
    ((rate * 0.04) as usize).next_power_of_two().clamp(65_536, 1 << 20)
}
/// Samples per transmit transfer: about 10 ms. The device queues four of them, so short
/// transfers keep the delay of the voice low.
fn tx_buffer_samples(rate: f64) -> usize {
    ((rate * 0.01) as usize).next_power_of_two().clamp(16_384, 1 << 18)
}
/// The AD9361 ADC is 12-bit; samples arrive in the ±2048 range.
const FULL_SCALE: f32 = 2048.0;
/// The DAC takes 12-bit samples in the top bits of 16: full scale is ±32767.
const TX_FULL_SCALE: f32 = 32767.0;
/// Transmit attenuation range of the AD9361 (dB).
pub const TX_ATTENUATION_MAX: f64 = 89.75;
/// Network and USB operations give up after this long (ms).
const TIMEOUT_MS: c_uint = 3000;

type Ptr = *mut c_void;

/// libiio functions in use.
struct Api {
    create_context: unsafe extern "C" fn(*const c_char) -> Ptr,
    context_destroy: unsafe extern "C" fn(Ptr),
    context_set_timeout: unsafe extern "C" fn(Ptr, c_uint) -> i32,
    context_attr: unsafe extern "C" fn(Ptr, *const c_char) -> *const c_char,
    find_device: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    find_channel: unsafe extern "C" fn(Ptr, *const c_char, bool) -> Ptr,
    attr_write: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    attr_read: unsafe extern "C" fn(Ptr, *const c_char, *mut c_char, usize) -> isize,
    device_attr_write: unsafe extern "C" fn(Ptr, *const c_char, *const c_char) -> isize,
    device_attr_read: unsafe extern "C" fn(Ptr, *const c_char, *mut c_char, usize) -> isize,
    channel_enable: unsafe extern "C" fn(Ptr),
    create_buffer: unsafe extern "C" fn(Ptr, usize, bool) -> Ptr,
    buffer_refill: unsafe extern "C" fn(Ptr) -> isize,
    buffer_push: unsafe extern "C" fn(Ptr) -> isize,
    buffer_start: unsafe extern "C" fn(Ptr) -> *const c_void,
    buffer_end: unsafe extern "C" fn(Ptr) -> *const c_void,
    buffer_destroy: unsafe extern "C" fn(Ptr),
    scan_create: unsafe extern "C" fn(*const c_char, c_uint) -> Ptr,
    scan_destroy: unsafe extern "C" fn(Ptr),
    scan_list: unsafe extern "C" fn(Ptr, *mut *mut Ptr) -> isize,
    info_list_free: unsafe extern "C" fn(*mut Ptr),
    info_uri: unsafe extern "C" fn(Ptr) -> *const c_char,
    info_description: unsafe extern "C" fn(Ptr) -> *const c_char,
    // The library must stay open as long as the function pointers are valid.
    _lib: Library,
}

impl Api {
    fn load() -> Result<Self, String> {
        let names: &[&str] = if cfg!(windows) {
            &["libiio.dll", "iio.dll"]
        } else if cfg!(target_os = "macos") {
            // Homebrew's lib folders are not on the default dlopen search path.
            &["libiio.dylib", "/opt/homebrew/lib/libiio.dylib", "/usr/local/lib/libiio.dylib", "/Library/Frameworks/iio.framework/iio"]
        } else {
            &["libiio.so.0", "libiio.so"]
        };
        let lib = names.iter().find_map(|&n| unsafe { Library::new(n) }.ok()).ok_or("libiio not found")?;
        unsafe {
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("libiio: {e}"))?
                };
            }
            Ok(Self {
                create_context: sym!("iio_create_context_from_uri"),
                context_destroy: sym!("iio_context_destroy"),
                context_set_timeout: sym!("iio_context_set_timeout"),
                context_attr: sym!("iio_context_get_attr_value"),
                find_device: sym!("iio_context_find_device"),
                find_channel: sym!("iio_device_find_channel"),
                attr_write: sym!("iio_channel_attr_write"),
                attr_read: sym!("iio_channel_attr_read"),
                device_attr_write: sym!("iio_device_attr_write"),
                device_attr_read: sym!("iio_device_attr_read"),
                channel_enable: sym!("iio_channel_enable"),
                create_buffer: sym!("iio_device_create_buffer"),
                buffer_refill: sym!("iio_buffer_refill"),
                buffer_push: sym!("iio_buffer_push"),
                buffer_start: sym!("iio_buffer_start"),
                buffer_end: sym!("iio_buffer_end"),
                buffer_destroy: sym!("iio_buffer_destroy"),
                scan_create: sym!("iio_create_scan_context"),
                scan_destroy: sym!("iio_scan_context_destroy"),
                scan_list: sym!("iio_scan_context_get_info_list"),
                info_list_free: sym!("iio_context_info_list_free"),
                info_uri: sym!("iio_context_info_get_uri"),
                info_description: sym!("iio_context_info_get_description"),
                _lib: lib,
            })
        }
    }
}

/// Whether libiio can be loaded (for diagnostics).
pub fn library_status() -> Result<(), String> {
    Api::load().map(|_| ())
}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("text without NUL")
}

fn text(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// A libiio URI from what the operator typed: a bare address or host name
/// ("192.168.2.1", "pluto.local") gets the network prefix "ip:".
pub fn normalize_uri(text: &str) -> String {
    let t = text.trim();
    let known = ["ip:", "usb:", "local:", "serial:", "xml:"];
    if t.is_empty() || known.iter().any(|p| t.to_ascii_lowercase().starts_with(p)) { t.to_string() } else { format!("ip:{t}") }
}

/// libiio contexts found on USB: (URI, description).
pub fn scan_usb() -> Vec<(String, String)> {
    let Ok(api) = Api::load() else { return Vec::new() };
    unsafe {
        let scan = (api.scan_create)(cstr("usb").as_ptr(), 0);
        if scan.is_null() {
            return Vec::new();
        }
        let mut list: *mut Ptr = std::ptr::null_mut();
        let n = (api.scan_list)(scan, &mut list);
        let mut found = Vec::new();
        for i in 0..n.max(0) as usize {
            let info = *list.add(i);
            if let Some(uri) = text((api.info_uri)(info)) {
                found.push((uri, text((api.info_description)(info)).unwrap_or_default()));
            }
        }
        if !list.is_null() {
            (api.info_list_free)(list);
        }
        (api.scan_destroy)(scan);
        found
    }
}

/// What an AD936x context reports about itself.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// The `hw_model` context attribute, e.g. "Analog Devices PlutoSDR Rev.B (Z7010-AD9363A)".
    pub model: String,
    /// Tunable receive range (Hz), from the LO's `frequency_available`.
    pub frequency_range: Option<(f64, f64)>,
    /// Sample rate range (S/s), from `sampling_frequency_available`.
    pub rate_range: Option<(f64, f64)>,
}

/// "[70000000 1 6000000000]" → (70e6, 6e9).
fn parse_range(text: &str) -> Option<(f64, f64)> {
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    let parts: Vec<f64> = inner.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [lo, _, hi] => Some((*lo, *hi)),
        [lo, hi] => Some((*lo, *hi)),
        _ => None,
    }
}

/// The context and what both halves need from it.
struct Shared {
    api: Api,
    context: Ptr,
    /// Serialises attribute reads and writes between the threads.
    attrs: Mutex<()>,
}

// Used from the receive and the transmit thread: buffers are separate and attribute
// access goes through `attrs`.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn write(&self, channel: Ptr, attr: &str, value: &str) -> Result<(), String> {
        let _guard = self.attrs.lock().unwrap();
        let n = unsafe { (self.api.attr_write)(channel, cstr(attr).as_ptr(), cstr(value).as_ptr()) };
        if n < 0 { Err(format!("{attr}={value}: error {n}")) } else { Ok(()) }
    }

    fn read(&self, channel: Ptr, attr: &str) -> Option<String> {
        let _guard = self.attrs.lock().unwrap();
        let mut buf = vec![0u8; 256];
        let n = unsafe { (self.api.attr_read)(channel, cstr(attr).as_ptr(), buf.as_mut_ptr() as *mut c_char, buf.len()) };
        if n <= 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(n as usize);
        Some(String::from_utf8_lossy(&buf[..end]).trim().to_string())
    }

    fn write_device(&self, device: Ptr, attr: &str, value: &str) -> Result<(), String> {
        let _guard = self.attrs.lock().unwrap();
        let n = unsafe { (self.api.device_attr_write)(device, cstr(attr).as_ptr(), cstr(value).as_ptr()) };
        if n < 0 { Err(format!("{attr}={value}: error {n}")) } else { Ok(()) }
    }

    fn read_device(&self, device: Ptr, attr: &str) -> Option<String> {
        let _guard = self.attrs.lock().unwrap();
        let mut buf = vec![0u8; 512];
        let n = unsafe { (self.api.device_attr_read)(device, cstr(attr).as_ptr(), buf.as_mut_ptr() as *mut c_char, buf.len()) };
        if n <= 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(n as usize);
        Some(String::from_utf8_lossy(&buf[..end]).trim().to_string())
    }

    fn device(&self, name: &str) -> Ptr {
        unsafe { (self.api.find_device)(self.context, cstr(name).as_ptr()) }
    }

    fn channel(&self, device: Ptr, name: &str, output: bool) -> Ptr {
        unsafe { (self.api.find_channel)(device, cstr(name).as_ptr(), output) }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe { (self.api.context_destroy)(self.context) };
    }
}

/// The receiving half of an open AD936x (and the owner of the connection).
pub struct IioRadio {
    shared: Arc<Shared>,
    uri: String,
    phy: Ptr,
    /// Why the programmable FIR filters could not be switched off, if so.
    pub fir_warning: Option<String>,
    /// The receive settings channel of ad9361-phy (sample rate, bandwidth, gain).
    rx_config: Ptr,
    /// The RX_LO channel of ad9361-phy.
    rx_lo: Ptr,
    rx: Ptr,
    i: Ptr,
    q: Ptr,
    /// Created on the first read, so that settings made before it apply to the first block;
    /// made again when the sample rate changes (its size follows the rate).
    buffer: Ptr,
    rate: f64,
    pending: Vec<Complex<f32>>,
    position: usize,
}

// The libiio objects of this half are used on a single thread.
unsafe impl Send for IioRadio {}

impl IioRadio {
    /// `uri` e.g. "ip:192.168.2.1" or "usb:1.5.5" (a bare address is taken as "ip:").
    pub fn open(uri: &str) -> Result<Self, String> {
        let uri = &normalize_uri(uri);
        let api = Api::load()?;
        let context = unsafe { (api.create_context)(cstr(uri).as_ptr()) };
        if context.is_null() {
            return Err(format!("could not connect to {uri}"));
        }
        unsafe { (api.context_set_timeout)(context, TIMEOUT_MS) };
        let shared = Arc::new(Shared { api, context, attrs: Mutex::new(()) });
        let (phy, rx) = (shared.device("ad9361-phy"), shared.device("cf-ad9361-lpc"));
        if phy.is_null() || rx.is_null() {
            return Err(format!("{uri} is not an AD936x radio"));
        }
        let rx_config = shared.channel(phy, "voltage0", false);
        let rx_lo = shared.channel(phy, "altvoltage0", true);
        let (i, q) = (shared.channel(rx, "voltage0", false), shared.channel(rx, "voltage1", false));
        if [rx_config, rx_lo, i, q].iter().any(|c| c.is_null()) {
            return Err(format!("{uri}: receiver channels not found"));
        }
        // A FIR filter left loaded by another program (SDR-Console, for its own rate)
        // tilted QODA's transmitted signal by 5 dB; without it the AD936x still reaches
        // 2.083 MS/s and more. The Pluto refuses to switch off the transmit filter alone,
        // so both go off together; failing that, each on its own.
        let fir_warning = shared.write_device(phy, "in_out_voltage_filter_fir_en", "0").err().and_then(|both| {
            let tx_config = shared.channel(phy, "voltage0", true);
            let rx = shared.write(rx_config, "filter_fir_en", "0");
            let tx = if tx_config.is_null() { Ok(()) } else { shared.write(tx_config, "filter_fir_en", "0") };
            (rx.is_err() || tx.is_err()).then(|| format!("FIR filter left on ({both})"))
        });
        Ok(Self {
            shared,
            uri: uri.to_string(),
            phy,
            fir_warning,
            rx_config,
            rx_lo,
            rx,
            i,
            q,
            buffer: std::ptr::null_mut(),
            rate: 2.4e6,
            pending: Vec::new(),
            position: 0,
        })
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn identity(&self) -> Identity {
        let model = {
            let _guard = self.shared.attrs.lock().unwrap();
            text(unsafe { (self.shared.api.context_attr)(self.shared.context, cstr("hw_model").as_ptr()) }).unwrap_or_default()
        };
        Identity {
            model,
            frequency_range: self.shared.read(self.rx_lo, "frequency_available").as_deref().and_then(parse_range),
            rate_range: self.shared.read(self.rx_config, "sampling_frequency_available").as_deref().and_then(parse_range),
        }
    }

    pub fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        self.shared.write(self.rx_lo, "frequency", &format!("{}", hz.round() as i64))
    }

    /// Sets the analog filter together with the sample rate. The filter is opened to the
    /// sample rate: when narrower, signals near the band edge are attenuated. The AD936x
    /// has one sample rate: it is also the transmitter's.
    pub fn set_sample_rate(&mut self, rate: f64) -> Result<(), String> {
        self.shared.write(self.rx_config, "sampling_frequency", &format!("{}", rate.round() as i64))?;
        let bandwidth = rate.clamp(200_000.0, 56_000_000.0);
        self.shared.write(self.rx_config, "rf_bandwidth", &format!("{}", bandwidth.round() as i64))?;
        if buffer_samples(rate) != buffer_samples(self.rate) && !self.buffer.is_null() {
            unsafe { (self.shared.api.buffer_destroy)(self.buffer) };
            self.buffer = std::ptr::null_mut();
        }
        self.rate = rate;
        Ok(())
    }

    /// `None` for automatic gain (slow AGC), otherwise a fixed gain in dB.
    pub fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String> {
        match gain {
            Some(db) => {
                self.shared.write(self.rx_config, "gain_control_mode", "manual")?;
                self.shared.write(self.rx_config, "hardwaregain", &format!("{db:.0}"))
            }
            None => self.shared.write(self.rx_config, "gain_control_mode", "slow_attack"),
        }
    }

    /// Fills `out`; reads a new block from the device if needed.
    pub fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String> {
        if self.position >= self.pending.len() {
            let api = &self.shared.api;
            unsafe {
                if self.buffer.is_null() {
                    (api.channel_enable)(self.i);
                    (api.channel_enable)(self.q);
                    self.buffer = (api.create_buffer)(self.rx, buffer_samples(self.rate), false);
                    if self.buffer.is_null() {
                        return Err("could not create the sample buffer".into());
                    }
                }
                let n = (api.buffer_refill)(self.buffer);
                if n < 0 {
                    return Err(format!("sample read failed: error {n}"));
                }
                self.pending.clear();
                self.position = 0;
                let start = (api.buffer_start)(self.buffer) as *const i16;
                let end = (api.buffer_end)(self.buffer) as *const i16;
                let raw = std::slice::from_raw_parts(start, end.offset_from(start) as usize);
                // I and Q channels interleaved, signed 16-bit.
                self.pending.extend(
                    raw.chunks_exact(2).map(|iq| Complex::new(iq[0] as f32 / FULL_SCALE, iq[1] as f32 / FULL_SCALE)),
                );
            }
        }
        let n = out.len().min(self.pending.len() - self.position);
        out[..n].copy_from_slice(&self.pending[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }

    /// The transmit path as the device reports it, one "name: value" per line, for
    /// diagnosis: rates through the interpolation chain, FIR, bandwidth, attenuation, LO.
    pub fn describe_tx(&self) -> String {
        let s = &self.shared;
        let mut out = String::new();
        for attr in ["tx_path_rates", "rx_path_rates", "in_out_voltage_filter_fir_en", "ensm_mode"] {
            out += &format!("{attr}: {}\n", s.read_device(self.phy, attr).unwrap_or_else(|| "?".into()));
        }
        let tx_config = s.channel(self.phy, "voltage0", true);
        let tx_lo = s.channel(self.phy, "altvoltage1", true);
        if !tx_config.is_null() {
            for attr in ["sampling_frequency", "rf_bandwidth", "filter_fir_en", "hardwaregain", "rf_port_select"] {
                out += &format!("tx {attr}: {}\n", s.read(tx_config, attr).unwrap_or_else(|| "?".into()));
            }
        }
        if !tx_lo.is_null() {
            for attr in ["frequency", "powerdown"] {
                out += &format!("tx_lo {attr}: {}\n", s.read(tx_lo, attr).unwrap_or_else(|| "?".into()));
            }
        }
        out
    }

    /// The transmitting half, on the same connection. It starts with its LO powered down
    /// and full attenuation: nothing goes out until `set_powered(true)`.
    pub fn transmitter(&self) -> Result<IioTransmitter, String> {
        let shared = &self.shared;
        let (phy, tx) = (shared.device("ad9361-phy"), shared.device("cf-ad9361-dds-core-lpc"));
        if tx.is_null() {
            return Err(format!("{}: no transmitter", self.uri));
        }
        let tx_config = shared.channel(phy, "voltage0", true);
        let tx_lo = shared.channel(phy, "altvoltage1", true);
        let (i, q) = (shared.channel(tx, "voltage0", true), shared.channel(tx, "voltage1", true));
        if [tx_config, tx_lo, i, q].iter().any(|c| c.is_null()) {
            return Err(format!("{}: transmitter channels not found", self.uri));
        }
        let mut t = IioTransmitter {
            shared: Arc::clone(shared),
            tx_config,
            tx_lo,
            tx,
            i,
            q,
            buffer: std::ptr::null_mut(),
            buffer_len: tx_buffer_samples(self.rate),
            filled: 0,
            powered: true,
        };
        t.set_attenuation(TX_ATTENUATION_MAX)?;
        t.set_powered(false)?;
        Ok(t)
    }
}

impl Drop for IioRadio {
    fn drop(&mut self) {
        if !self.buffer.is_null() {
            unsafe { (self.shared.api.buffer_destroy)(self.buffer) };
        }
    }
}

/// The transmitting half of an open AD936x.
pub struct IioTransmitter {
    shared: Arc<Shared>,
    /// The transmit settings channel of ad9361-phy (bandwidth, attenuation).
    tx_config: Ptr,
    /// The TX_LO channel of ad9361-phy.
    tx_lo: Ptr,
    tx: Ptr,
    i: Ptr,
    q: Ptr,
    buffer: Ptr,
    buffer_len: usize,
    /// Samples already written into the current buffer.
    filled: usize,
    powered: bool,
}

// The libiio objects of this half are used on a single thread.
unsafe impl Send for IioTransmitter {}

impl IioTransmitter {
    pub fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        self.shared.write(self.tx_lo, "frequency", &format!("{}", hz.round() as i64))
    }

    /// Output attenuation (dB: 0 = full power … 89.75), in 0.25 dB steps.
    pub fn set_attenuation(&mut self, db: f64) -> Result<(), String> {
        let db = (db.clamp(0.0, TX_ATTENUATION_MAX) * 4.0).round() / 4.0;
        self.shared.write(self.tx_config, "hardwaregain", &format!("{:.2}", -db))
    }

    /// Analog transmit filter; the sample rate itself is set through the receiver.
    pub fn set_bandwidth(&mut self, hz: f64) -> Result<(), String> {
        self.shared.write(self.tx_config, "rf_bandwidth", &format!("{}", hz.clamp(200_000.0, 56_000_000.0).round() as i64))
    }

    /// Switches the transmit LO on or off. Off, nothing leaks out (not even the LO).
    pub fn set_powered(&mut self, on: bool) -> Result<(), String> {
        if on != self.powered {
            self.shared.write(self.tx_lo, "powerdown", if on { "0" } else { "1" })?;
            self.powered = on;
        }
        Ok(())
    }

    pub fn powered(&self) -> bool {
        self.powered
    }

    /// Samples per transfer.
    pub fn buffer_len(&self) -> usize {
        self.buffer_len
    }

    /// Drops the transfer buffer and whatever the device still has queued, so that the
    /// next transmission starts with fresh audio.
    pub fn stop(&mut self) {
        if !self.buffer.is_null() {
            unsafe { (self.shared.api.buffer_destroy)(self.buffer) };
            self.buffer = std::ptr::null_mut();
        }
        self.filled = 0;
    }

    /// Queues samples (full scale 1.0); every full buffer is sent. Blocks while the
    /// device's queue is full, which paces the caller at the sample rate.
    pub fn write(&mut self, mut samples: &[Complex<f32>]) -> Result<(), String> {
        let api = &self.shared.api;
        while !samples.is_empty() {
            unsafe {
                if self.buffer.is_null() {
                    (api.channel_enable)(self.i);
                    (api.channel_enable)(self.q);
                    self.buffer = (api.create_buffer)(self.tx, self.buffer_len, false);
                    if self.buffer.is_null() {
                        return Err("could not create the transmit buffer".into());
                    }
                    self.filled = 0;
                }
                let start = (api.buffer_start)(self.buffer) as *mut i16;
                let raw = std::slice::from_raw_parts_mut(start, self.buffer_len * 2);
                let n = samples.len().min(self.buffer_len - self.filled);
                for (k, z) in samples[..n].iter().enumerate() {
                    let at = (self.filled + k) * 2;
                    raw[at] = (z.re.clamp(-1.0, 1.0) * TX_FULL_SCALE) as i16;
                    raw[at + 1] = (z.im.clamp(-1.0, 1.0) * TX_FULL_SCALE) as i16;
                }
                self.filled += n;
                samples = &samples[n..];
                if self.filled == self.buffer_len {
                    self.filled = 0;
                    let sent = (api.buffer_push)(self.buffer);
                    if sent < 0 {
                        return Err(format!("sample write failed: error {sent}"));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Drop for IioTransmitter {
    fn drop(&mut self) {
        let _ = self.set_attenuation(TX_ATTENUATION_MAX);
        let _ = self.set_powered(false);
        if !self.buffer.is_null() {
            unsafe { (self.shared.api.buffer_destroy)(self.buffer) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_iio_ranges() {
        assert_eq!(parse_range("[70000000 1 6000000000]"), Some((70e6, 6e9)));
        assert_eq!(parse_range("[2083333 1 61440000]"), Some((2_083_333.0, 61.44e6)));
        assert_eq!(parse_range("garbage"), None);
    }

    #[test]
    fn bare_addresses_become_network_uris() {
        assert_eq!(normalize_uri(" 192.168.3.50 "), "ip:192.168.3.50");
        assert_eq!(normalize_uri("pluto.local"), "ip:pluto.local");
        assert_eq!(normalize_uri("ip:192.168.2.1"), "ip:192.168.2.1");
        assert_eq!(normalize_uri("usb:1.5.5"), "usb:1.5.5");
        assert_eq!(normalize_uri("IP:10.0.0.2"), "IP:10.0.0.2");
    }

    #[test]
    fn buffer_sizes() {
        assert_eq!(tx_buffer_samples(2.4e6), 32_768);
        assert_eq!(buffer_samples(2.4e6), 131_072);
    }

    /// With a real device: `QOALL_IIO_URI=ip:192.168.2.1 cargo test --lib iio -- --ignored --nocapture`
    /// (opens the transmitter too, but leaves it powered down and fully attenuated).
    #[test]
    #[ignore]
    fn identifies_device() {
        let Some(uri) = std::env::var_os("QOALL_IIO_URI") else { return };
        let radio = IioRadio::open(uri.to_str().unwrap()).unwrap();
        println!("{:?}", radio.identity());
        println!("transmitter: {:?}", radio.transmitter().map(|t| t.buffer_len()));
        println!("USB: {:?}", scan_usb());
    }
}
