//! CW sending: text in Morse, a straight key and an iambic paddle, made into a keyed
//! audio tone. The tone goes through the SSB modulator set so that the carrier lands on
//! the dial.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Audio pitch of the keyed tone (Hz); the modulator's carrier sits this much below the
/// dial so the signal goes out on the dial itself.
pub const TONE_HZ: f64 = crate::demod::CW_TONE;
/// Rise and fall of each element (s): no key clicks.
const RAMP_S: f64 = 0.005;
/// Level of the tone (the drive scales it).
const LEVEL: f32 = 0.9;

/// Morse of a character: '.' and '-'. Prosigns are sent from their bracketed form.
pub fn code(c: char) -> Option<&'static str> {
    Some(match c.to_ascii_uppercase() {
        'A' => ".-",
        'B' => "-...",
        'C' => "-.-.",
        'D' => "-..",
        'E' => ".",
        'F' => "..-.",
        'G' => "--.",
        'H' => "....",
        'I' | 'İ' | 'ı' => "..",
        'J' => ".---",
        'K' => "-.-",
        'L' => ".-..",
        'M' => "--",
        'N' => "-.",
        'O' => "---",
        'P' => ".--.",
        'Q' => "--.-",
        'R' => ".-.",
        'S' => "...",
        'T' => "-",
        'U' => "..-",
        'V' => "...-",
        'W' => ".--",
        'X' => "-..-",
        'Y' => "-.--",
        'Z' => "--..",
        '0' => "-----",
        '1' => ".----",
        '2' => "..---",
        '3' => "...--",
        '4' => "....-",
        '5' => ".....",
        '6' => "-....",
        '7' => "--...",
        '8' => "---..",
        '9' => "----.",
        '.' => ".-.-.-",
        ',' => "--..--",
        '?' => "..--..",
        '/' => "-..-.",
        '=' => "-...-",
        '+' => ".-.-.",
        '-' => "-....-",
        '\'' => ".----.",
        '(' => "-.--.",
        ')' => "-.--.-",
        ':' => "---...",
        '@' => ".--.-.",
        'Ç' | 'ç' => "-.-..",
        'Ğ' | 'ğ' => "--.-.",
        'Ö' | 'ö' => "---.",
        'Ş' | 'ş' => "----",
        'Ü' | 'ü' => "..--",
        _ => return None,
    })
}

/// Key-down/up per dot unit of a text: elements 1 unit apart, letters 3, words 7.
/// Prosigns written in angle brackets ("<SK>", "<AR>", "<BT>") are sent run together.
pub fn units(text: &str) -> Vec<bool> {
    let mut out = Vec::new();
    let mut word_gap = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            word_gap = !out.is_empty();
            continue;
        }
        // A prosign: the letters inside <> as one character.
        let letters: Vec<char> = if c == '<' {
            let mut v = Vec::new();
            for d in chars.by_ref() {
                if d == '>' {
                    break;
                }
                v.push(d);
            }
            v
        } else {
            vec![c]
        };
        let elements: String = letters.iter().filter_map(|&l| code(l)).collect();
        if elements.is_empty() {
            continue;
        }
        if !out.is_empty() {
            // The gap before this character: 3 units after a letter, 7 after a word.
            out.extend(std::iter::repeat_n(false, if word_gap { 7 } else { 3 } - 1));
        }
        word_gap = false;
        for e in elements.chars() {
            out.extend(std::iter::repeat_n(true, if e == '-' { 3 } else { 1 }));
            out.push(false);
        }
    }
    // A word's worth of silence at the end, so the last element is not cut.
    out.extend([false; 3]);
    out
}

/// The keyer: text, straight key and paddles to a tone.
pub struct Keyer {
    rate: f64,
    pub wpm: f64,
    /// Inputs from the window.
    pub straight: bool,
    pub dit: bool,
    pub dah: bool,
    /// Text being sent, as units, and how far it is (samples into the current unit).
    text: VecDeque<bool>,
    text_pos: usize,
    /// Paddle element in progress: samples left keyed and then of the gap after it.
    el_on: usize,
    el_gap: usize,
    last_dit: bool,
    /// A paddle pressed during the other element (iambic memory).
    dit_mem: bool,
    dah_mem: bool,
    /// Envelope (0..1) and tone phase.
    env: f64,
    phase: f64,
}

impl Keyer {
    pub fn new(rate: f64, wpm: f64) -> Self {
        Self {
            rate,
            wpm,
            straight: false,
            dit: false,
            dah: false,
            text: VecDeque::new(),
            text_pos: 0,
            el_on: 0,
            el_gap: 0,
            last_dit: false,
            dit_mem: false,
            dah_mem: false,
            env: 0.0,
            phase: 0.0,
        }
    }

    fn unit(&self) -> usize {
        (self.rate * 1.2 / self.wpm.clamp(5.0, 60.0)) as usize
    }

    /// Queues text to send.
    pub fn send(&mut self, text: &str) {
        self.text.extend(units(text));
    }

    /// Stops sending (text and paddle element).
    pub fn stop(&mut self) {
        self.text.clear();
        self.text_pos = 0;
        self.el_on = 0;
        self.el_gap = 0;
        self.dit_mem = false;
        self.dah_mem = false;
    }

    /// Text not yet sent, in units.
    pub fn text_left(&self) -> usize {
        self.text.len()
    }

    /// Whether anything is keyed or about to be.
    pub fn busy(&self) -> bool {
        self.straight || self.dit || self.dah || !self.text.is_empty() || self.el_on + self.el_gap > 0 || self.env > 0.0
    }

    /// Whether the key is down now (for the sidetone).
    pub fn key_down(&self) -> bool {
        self.env > 0.5
    }

    /// Key state for the next sample.
    fn next_key(&mut self) -> bool {
        let unit = self.unit().max(1);
        // Text first.
        if let Some(&on) = self.text.front() {
            self.text_pos += 1;
            if self.text_pos >= unit {
                self.text_pos = 0;
                self.text.pop_front();
            }
            return on;
        }
        // The paddle: an element and its gap run to the end.
        if self.el_on > 0 {
            self.el_on -= 1;
            // Iambic memory: the other paddle pressed while this element sounds.
            if self.last_dit && self.dah {
                self.dah_mem = true;
            }
            if !self.last_dit && self.dit {
                self.dit_mem = true;
            }
            return true;
        }
        if self.el_gap > 0 {
            self.el_gap -= 1;
            return false;
        }
        let want_dit = self.dit || self.dit_mem;
        let want_dah = self.dah || self.dah_mem;
        let next_dit = match (want_dit, want_dah) {
            (true, true) => Some(!self.last_dit),
            (true, false) => Some(true),
            (false, true) => Some(false),
            (false, false) => None,
        };
        if let Some(d) = next_dit {
            self.last_dit = d;
            self.dit_mem = false;
            self.dah_mem = false;
            self.el_on = if d { unit } else { 3 * unit } - 1;
            self.el_gap = unit;
            return true;
        }
        self.straight
    }

    /// Appends `n` samples of keyed tone (amplitude `drive` × the level).
    pub fn render(&mut self, n: usize, drive: f32, out: &mut Vec<f32>) {
        let step_env = 1.0 / (RAMP_S * self.rate);
        let step_phase = 2.0 * PI * TONE_HZ / self.rate;
        for _ in 0..n {
            let target = if self.next_key() { 1.0 } else { 0.0 };
            self.env = if target > self.env { (self.env + step_env).min(1.0) } else { (self.env - step_env).max(0.0) };
            // Raised-cosine edges.
            let shape = 0.5 - 0.5 * (PI * self.env).cos();
            self.phase = (self.phase + step_phase) % (2.0 * PI);
            out.push(LEVEL * drive * (shape * self.phase.sin()) as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 8_000.0;

    /// Key-down stretches (in units) of rendered audio.
    fn marks(audio: &[f32], unit: usize) -> Vec<f64> {
        let mut out = Vec::new();
        let mut run = 0usize;
        // Envelope by the peak over half a tone period.
        let win = (RATE / TONE_HZ) as usize;
        for chunk in audio.chunks(win) {
            let on = chunk.iter().any(|v| v.abs() > 0.3);
            if on {
                run += chunk.len();
            } else if run > 0 {
                out.push(run as f64 / unit as f64);
                run = 0;
            }
        }
        out
    }

    #[test]
    fn paris_takes_fifty_units() {
        // PARIS: the standard 43 units, then the gap after the last element and the
        // closing silence.
        let u = units("PARIS");
        assert_eq!(u.len(), 43 + 1 + 3, "{}", u.len());
        let k = Keyer::new(RATE, 20.0);
        let mut k2 = k;
        k2.send("PARIS PARIS");
        let mut a = Vec::new();
        while k2.text_left() > 0 {
            k2.render(800, 1.0, &mut a);
        }
        // Two words with a 7-unit gap: 43 + 7 + 43 + 1 + 3 (60 ms units at 20 WPM).
        let secs = a.len() as f64 / RATE;
        assert!((secs - 97.0 * 0.06).abs() < 0.1, "{secs}");
    }

    #[test]
    fn letters_and_prosigns() {
        assert_eq!(units("E"), vec![true, false, false, false, false]);
        assert_eq!(units("<SK>").iter().filter(|&&b| b).count(), units("SK").iter().filter(|&&b| b).count());
        assert!(units("<SK>").len() < units("SK").len(), "a prosign has no letter gap inside");
        assert_eq!(code('Ş'), Some("----"));
        assert!(units("€").iter().all(|b| !b));
    }

    #[test]
    fn text_elements_have_the_right_lengths() {
        let mut k = Keyer::new(RATE, 20.0);
        let unit = k.unit();
        k.send("TEST");
        let mut a = Vec::new();
        while k.text_left() > 0 {
            k.render(400, 1.0, &mut a);
        }
        let m: Vec<f64> = marks(&a, unit).iter().map(|v| v.round()).collect();
        assert_eq!(m, vec![3.0, 1.0, 1.0, 1.0, 1.0, 3.0]);
    }

    #[test]
    fn paddles_repeat_and_alternate() {
        let mut k = Keyer::new(RATE, 20.0);
        let unit = k.unit();
        let mut a = Vec::new();
        k.dit = true;
        k.render((RATE * 0.6) as usize, 1.0, &mut a);
        k.dit = false;
        k.render((RATE * 0.3) as usize, 1.0, &mut a);
        let dits = marks(&a, unit);
        assert_eq!(dits.len(), 5, "0.6 s of dits at 20 WPM: {dits:?}");
        assert!(dits.iter().all(|d| (d - 1.0).abs() < 0.2));
        // Both paddles: dit, dah, dit, dah...
        let mut a = Vec::new();
        k.dit = true;
        k.dah = true;
        k.render((RATE * 0.95) as usize, 1.0, &mut a);
        k.dit = false;
        k.dah = false;
        k.render((RATE * 0.4) as usize, 1.0, &mut a);
        let m: Vec<f64> = marks(&a, unit).iter().map(|v| v.round()).collect();
        assert!(m.windows(2).all(|w| w[0] != w[1]), "alternating: {m:?}");
        assert!(!k.busy());
    }

    #[test]
    fn straight_key_follows_the_hand_without_clicks() {
        let mut k = Keyer::new(RATE, 20.0);
        let mut a = Vec::new();
        k.straight = true;
        k.render(800, 1.0, &mut a);
        k.straight = false;
        k.render(800, 1.0, &mut a);
        // Soft start: the first millisecond stays low.
        assert!(a[..8].iter().all(|v| v.abs() < 0.1));
        assert!(a[400..800].iter().any(|v| v.abs() > 0.85));
        assert!(a[1_000..].iter().all(|v| v.abs() < 1e-6));
    }
}
