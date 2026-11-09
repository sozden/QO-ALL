//! PTT out and a CW key in on serial ports.
//!
//! PTT: DTR or RTS goes high while the station transmits (SSB, CW, FT8/FT4, tune), for an
//! amplifier, a sequencer or another radio. Key: a straight key or a paddle between a
//! line the program holds high (DTR and RTS) and CTS or DSR; the usual paddle wiring is
//! dit on DSR, dah on CTS. The key only works in CW with TX enabled.
//!
//! It lives apart from the engine: the settings can change it without restarting the
//! radio, and a new engine is attached to it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::engine::{TxSettings, TxStatus};
use crate::keyer::Keyer;
use crate::serial::{Inputs, Port};
use crate::vox::TxState;

/// Poll period of the lines.
const POLL: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PttLine {
    #[default]
    Off,
    Dtr,
    Rts,
}

impl PttLine {
    pub const ALL: [PttLine; 3] = [PttLine::Off, PttLine::Dtr, PttLine::Rts];

    pub fn name(self) -> &'static str {
        match self {
            PttLine::Off => "off",
            PttLine::Dtr => "DTR",
            PttLine::Rts => "RTS",
        }
    }

    pub fn from_name(s: &str) -> Self {
        Self::ALL.into_iter().find(|l| l.name() == s).unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum KeyInput {
    #[default]
    Off,
    StraightCts,
    StraightDsr,
    /// Dit on DSR, dah on CTS (the usual wiring).
    PaddleDsrCts,
    PaddleCtsDsr,
}

impl KeyInput {
    pub const ALL: [KeyInput; 5] = [KeyInput::Off, KeyInput::StraightCts, KeyInput::StraightDsr, KeyInput::PaddleDsrCts, KeyInput::PaddleCtsDsr];

    pub fn name(self) -> &'static str {
        match self {
            KeyInput::Off => "off",
            KeyInput::StraightCts => "straight-cts",
            KeyInput::StraightDsr => "straight-dsr",
            KeyInput::PaddleDsrCts => "paddle-dsr-cts",
            KeyInput::PaddleCtsDsr => "paddle-cts-dsr",
        }
    }

    pub fn from_name(s: &str) -> Self {
        Self::ALL.into_iter().find(|k| k.name() == s).unwrap_or_default()
    }

    /// (straight, dit, dah) from the lines.
    pub fn read(self, i: Inputs) -> (bool, bool, bool) {
        match self {
            KeyInput::Off => (false, false, false),
            KeyInput::StraightCts => (i.cts, false, false),
            KeyInput::StraightDsr => (i.dsr, false, false),
            KeyInput::PaddleDsrCts => (false, i.dsr, i.cts),
            KeyInput::PaddleCtsDsr => (false, i.cts, i.dsr),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerialConfig {
    pub ptt_port: String,
    pub ptt_line: PttLine,
    pub key_port: String,
    pub key_input: KeyInput,
}

impl SerialConfig {
    fn ptt(&self) -> Option<&str> {
        (self.ptt_line != PttLine::Off && !self.ptt_port.trim().is_empty()).then(|| self.ptt_port.trim())
    }

    fn key(&self) -> Option<&str> {
        (self.key_input != KeyInput::Off && !self.key_port.trim().is_empty()).then(|| self.key_port.trim())
    }
}

/// The running station's parts the lines work with.
#[derive(Clone)]
pub struct Station {
    pub tx: Arc<Mutex<TxSettings>>,
    pub status: Arc<Mutex<TxStatus>>,
    pub keyer: Arc<Mutex<Keyer>>,
}

/// What the window shows: whether PTT is out and what the key lines read.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineState {
    pub ptt: bool,
    pub key: (bool, bool, bool),
}

pub struct SerialIo {
    pub config: SerialConfig,
    /// Ports that could not be opened.
    pub errors: Vec<String>,
    station: Arc<Mutex<Option<Station>>>,
    state: Arc<Mutex<LineState>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SerialIo {
    pub fn start(config: SerialConfig) -> Self {
        let mut errors = Vec::new();
        let mut open = |name: &str| match Port::open(name) {
            Ok(p) => Some(p),
            Err(e) => {
                errors.push(format!("{name}: {e}"));
                None
            }
        };
        // One handle when PTT and the key share a port.
        let ptt_port = config.ptt().and_then(&mut open);
        let key_port = match config.key() {
            Some(k) if Some(k) == config.ptt() => None,
            Some(k) => open(k),
            None => None,
        };
        let shared = config.key().is_some() && config.key() == config.ptt();
        let station = Arc::new(Mutex::new(None));
        let state = Arc::new(Mutex::new(LineState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = (ptt_port.is_some() || key_port.is_some() || shared).then(|| {
            let lines = Lines { ptt_line: config.ptt_line, key: config.key_input, ptt_port, key_port, shared };
            let (station, state, stop) = (station.clone(), state.clone(), stop.clone());
            thread::Builder::new().name("serial".into()).spawn(move || lines.run(&station, &state, &stop)).expect("serial thread")
        });
        Self { config, errors, station, state, stop, thread }
    }

    /// Works with this station from now on (None: none running).
    pub fn attach(&self, station: Option<Station>) {
        *self.station.lock().unwrap() = station;
    }

    pub fn state(&self) -> LineState {
        *self.state.lock().unwrap()
    }

    pub fn active(&self) -> bool {
        self.thread.is_some()
    }
}

impl Drop for SerialIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Lines {
    ptt_line: PttLine,
    key: KeyInput,
    ptt_port: Option<Port>,
    key_port: Option<Port>,
    /// The key is on the PTT port.
    shared: bool,
}

impl Lines {
    fn set_ptt(&self, on: bool) {
        if let Some(p) = &self.ptt_port {
            let _ = match self.ptt_line {
                PttLine::Dtr => p.set_dtr(on),
                PttLine::Rts => p.set_rts(on),
                PttLine::Off => Ok(()),
            };
        }
    }

    fn run(self, station: &Mutex<Option<Station>>, state: &Mutex<LineState>, stop: &AtomicBool) {
        let key_port = if self.shared { self.ptt_port.as_ref() } else { self.key_port.as_ref() };
        // The key's supply: both lines out high, except the one keying PTT.
        if let Some(k) = key_port {
            let ptt_here = self.shared;
            if !(ptt_here && self.ptt_line == PttLine::Dtr) {
                let _ = k.set_dtr(true);
            }
            if !(ptt_here && self.ptt_line == PttLine::Rts) {
                let _ = k.set_rts(true);
            }
        }
        let mut ptt = false;
        while !stop.load(Ordering::Relaxed) {
            let st = station.lock().unwrap().clone();
            let key = key_port.and_then(|k| k.inputs().ok()).map_or((false, false, false), |i| self.key.read(i));
            let transmitting = st.as_ref().is_some_and(|s| s.status.lock().unwrap().state == TxState::Transmit);
            if transmitting != ptt {
                ptt = transmitting;
                self.set_ptt(ptt);
            }
            if let Some(s) = &st
                && key_port.is_some()
            {
                let tx = *s.tx.lock().unwrap();
                let on = tx.armed && tx.cw && !tx.digital;
                let mut k = s.keyer.lock().unwrap();
                (k.ext_straight, k.ext_dit, k.ext_dah) = (on && key.0, on && key.1, on && key.2);
            }
            *state.lock().unwrap() = LineState { ptt, key };
            thread::sleep(POLL);
        }
        self.set_ptt(false);
        if let Some(s) = station.lock().unwrap().as_ref() {
            let mut k = s.keyer.lock().unwrap();
            (k.ext_straight, k.ext_dit, k.ext_dah) = (false, false, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_wiring() {
        let i = Inputs { cts: true, dsr: false, dcd: false };
        assert_eq!(KeyInput::StraightCts.read(i), (true, false, false));
        assert_eq!(KeyInput::PaddleDsrCts.read(i), (false, false, true));
        assert_eq!(KeyInput::PaddleCtsDsr.read(i), (false, true, false));
        for k in KeyInput::ALL {
            assert_eq!(KeyInput::from_name(k.name()), k);
        }
        for l in PttLine::ALL {
            assert_eq!(PttLine::from_name(l.name()), l);
        }
    }

    #[test]
    fn nothing_to_open_runs_nothing() {
        let io = SerialIo::start(SerialConfig { ptt_port: "COM5".into(), ..Default::default() });
        assert!(!io.active() && io.errors.is_empty());
    }
}
