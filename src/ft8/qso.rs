//! FT8/FT4 contacts as WSJT-X runs them: the six standard messages and the automatic
//! sequence from a CQ or an answer to the final 73, with logging when complete.
//!
//! Answering a CQ:  Tx1 "DX ME GRID" → their report → Tx3 "DX ME R-05" → their RR73 →
//! Tx5 "DX ME 73" (logged). Calling CQ: Tx6 "CQ ME GRID" → their grid → Tx2
//! "DX ME -05" → their R-report → Tx4 "DX ME RR73" (logged).

use super::message::is_standard_call;

/// What a message says after the two calls.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    None,
    Grid(String),
    Report(i32),
    RReport(i32),
    Rrr,
    Rr73,
    Bye73,
}

/// A decoded message, taken apart.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    Cq { from: String, grid: Option<String> },
    Directed { to: String, from: String, payload: Payload },
    Other,
}

fn call(token: &str) -> String {
    token.trim_matches(|c| c == '<' || c == '>').to_ascii_uppercase()
}

fn is_grid(t: &str) -> bool {
    let b = t.as_bytes();
    t.len() == 4 && t != "RR73" && b[0].is_ascii_uppercase() && b[1].is_ascii_uppercase() && b[2].is_ascii_digit() && b[3].is_ascii_digit()
}

fn report(t: &str) -> Option<i32> {
    (t.starts_with('-') || t.starts_with('+')).then(|| t.parse().ok()).flatten()
}

/// Takes a decoded message apart.
pub fn parse(text: &str) -> Parsed {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["CQ", rest @ ..] => {
            // "CQ CALL", "CQ CALL GRID", "CQ DX CALL GRID" (a modifier before the call).
            let rest: Vec<&str> = rest.to_vec();
            let (from, grid) = match rest.as_slice() {
                [c] => (*c, None),
                [c, g] if is_grid(g) => (*c, Some(*g)),
                [_, c] => (*c, None),
                [_, c, g] if is_grid(g) => (*c, Some(*g)),
                _ => return Parsed::Other,
            };
            Parsed::Cq { from: call(from), grid: grid.map(str::to_string) }
        }
        [to, from] => Parsed::Directed { to: call(to), from: call(from), payload: Payload::None },
        [to, from, x] => {
            let payload = match *x {
                "RRR" => Payload::Rrr,
                "RR73" => Payload::Rr73,
                "73" => Payload::Bye73,
                g if is_grid(g) => Payload::Grid(g.to_string()),
                r => match (r.strip_prefix('R').and_then(report), report(r)) {
                    (Some(v), _) => Payload::RReport(v),
                    (None, Some(v)) => Payload::Report(v),
                    _ => return Parsed::Other,
                },
            };
            Parsed::Directed { to: call(to), from: call(from), payload }
        }
        _ => Parsed::Other,
    }
}

/// The six standard messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Tx1,
    Tx2,
    Tx3,
    Tx4,
    Tx5,
    Tx6,
}

impl Step {
    pub const ALL: [Step; 6] = [Step::Tx1, Step::Tx2, Step::Tx3, Step::Tx4, Step::Tx5, Step::Tx6];
}

/// Report as sent: "-05", "+03".
pub fn format_report(r: i32) -> String {
    format!("{:+03}", r.clamp(-50, 49))
}

/// The other station of a contact.
#[derive(Clone, Debug, PartialEq)]
pub struct Partner {
    pub call: String,
    pub grid: Option<String>,
    /// Report given to them (their signal here) and received from them.
    pub sent: Option<i32>,
    pub rcvd: Option<i32>,
    /// When the contact started (Unix seconds).
    pub start: u64,
}

/// A finished contact, to be logged.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub partner: Partner,
    pub end: u64,
}

/// One decode as the sequencer needs it.
#[derive(Clone, Debug)]
pub struct Heard {
    pub text: String,
    pub snr: i32,
    /// Whether it was sent in an odd slot.
    pub odd: bool,
}

/// The automatic sequence.
#[derive(Clone, Debug)]
pub struct Sequencer {
    pub my_call: String,
    /// Four-character grid.
    pub my_grid: String,
    pub dx: Option<Partner>,
    /// The message to send next, and the slot parity to send it in.
    pub next: Step,
    pub tx_odd: bool,
    /// Times the current message went out without an answer.
    pub repeats: u32,
    /// A directed message is given up after this many repeats.
    pub max_repeats: u32,
    /// Nothing more to send (contact over, or given up).
    pub done: bool,
    logged: bool,
    finished: Option<Finished>,
}

impl Sequencer {
    pub fn new(my_call: &str, my_grid: &str) -> Self {
        Self {
            my_call: my_call.trim().to_ascii_uppercase(),
            my_grid: my_grid.trim().to_ascii_uppercase().chars().take(4).collect(),
            dx: None,
            next: Step::Tx6,
            tx_odd: false,
            repeats: 0,
            max_repeats: 6,
            done: true,
            logged: false,
            finished: None,
        }
    }

    /// Text of a message with the current partner. A non-standard call (a prefix such as
    /// C9/ZS6WAB) goes as WSJT-X sends it: hashed in <> beside a standard call, or in a
    /// type 4 message without grid or report when it is our own.
    pub fn message(&self, step: Step) -> String {
        let me = &self.my_call;
        let dx = self.dx.as_ref().map_or("DX", |d| d.call.as_str());
        let rpt = format_report(self.dx.as_ref().and_then(|d| d.sent).unwrap_or(-10));
        let (me_std, dx_std) = (is_standard_call(me), dx == "DX" || is_standard_call(dx));
        if me_std && !dx_std {
            return match step {
                Step::Tx1 => format!("<{dx}> {me} {}", self.my_grid),
                Step::Tx2 => format!("<{dx}> {me} {rpt}"),
                Step::Tx3 => format!("<{dx}> {me} R{rpt}"),
                Step::Tx4 => format!("<{dx}> {me} RR73"),
                Step::Tx5 => format!("<{dx}> {me} 73"),
                Step::Tx6 => format!("CQ {me} {}", self.my_grid),
            };
        }
        if !me_std && dx_std {
            return match step {
                Step::Tx1 => format!("<{dx}> {me}"),
                Step::Tx2 => format!("{dx} <{me}> {rpt}"),
                Step::Tx3 => format!("{dx} <{me}> R{rpt}"),
                Step::Tx4 => format!("<{dx}> {me} RR73"),
                Step::Tx5 => format!("<{dx}> {me} 73"),
                Step::Tx6 => format!("CQ {me}"),
            };
        }
        match step {
            Step::Tx1 => format!("{dx} {me} {}", self.my_grid),
            Step::Tx2 => format!("{dx} {me} {rpt}"),
            Step::Tx3 => format!("{dx} {me} R{rpt}"),
            Step::Tx4 => format!("{dx} {me} RR73"),
            Step::Tx5 => format!("{dx} {me} 73"),
            Step::Tx6 => format!("CQ {me} {}", self.my_grid),
        }
    }

    fn begin(&mut self, call: &str, grid: Option<String>, snr: i32, now: u64, their_odd: bool, next: Step) {
        self.dx = Some(Partner { call: call.to_string(), grid, sent: Some(snr), rcvd: None, start: now });
        self.next = next;
        self.tx_odd = !their_odd;
        self.repeats = 0;
        self.done = false;
        self.logged = false;
    }

    /// Calling CQ in even (first) or odd slots.
    pub fn call_cq(&mut self, odd: bool) {
        self.dx = None;
        self.next = Step::Tx6;
        self.tx_odd = odd;
        self.repeats = 0;
        self.done = false;
        self.logged = false;
    }

    /// The operator picked a decode (double click): answer a CQ, take up a message to
    /// us, or call the sender of any other message.
    pub fn pick(&mut self, h: &Heard, now: u64) -> bool {
        match parse(&h.text) {
            Parsed::Cq { from, grid } if from != self.my_call => {
                self.begin(&from, grid, h.snr, now, h.odd, Step::Tx1);
                true
            }
            Parsed::Directed { to, from, .. } if to == self.my_call && from != self.my_call => {
                self.dx = None;
                self.done = false;
                self.heard(std::slice::from_ref(h), now);
                true
            }
            Parsed::Directed { from, .. } if from != self.my_call => {
                self.begin(&from, None, h.snr, now, h.odd, Step::Tx1);
                true
            }
            _ => false,
        }
    }

    /// Takes one slot's decodes; moves the sequence on with what was sent to us.
    pub fn heard(&mut self, decodes: &[Heard], now: u64) {
        for h in decodes {
            let Parsed::Directed { to, from, payload } = parse(&h.text) else { continue };
            if to != self.my_call || from == self.my_call {
                continue;
            }
            // Busy with someone else: the call waits. After a finished (logged) contact
            // a new caller is taken up; the same partner again goes on with the old one.
            let mut partner = self.dx.as_ref().map(|d| d.call.clone());
            if partner.as_deref().is_some_and(|p| p != from) {
                if !(self.done && self.logged) {
                    continue;
                }
                self.dx = None;
                partner = None;
            }
            // Not working anyone (calling CQ, or idle): the caller is answered, as WSJT-X
            // does with auto-sequence on.
            if partner.is_none() {
                self.dx = Some(Partner { call: from.clone(), grid: None, sent: Some(h.snr), rcvd: None, start: now });
                self.logged = false;
            }
            let d = self.dx.as_mut().expect("partner");
            d.sent = d.sent.or(Some(h.snr));
            let before = self.next;
            let bye = payload == Payload::Bye73;
            self.next = match payload {
                Payload::None => Step::Tx2,
                Payload::Grid(g) => {
                    d.grid = Some(g);
                    Step::Tx2
                }
                Payload::Report(r) => {
                    d.rcvd = Some(r);
                    Step::Tx3
                }
                Payload::RReport(r) => {
                    d.rcvd = Some(r);
                    Step::Tx4
                }
                Payload::Rrr | Payload::Rr73 => Step::Tx5,
                Payload::Bye73 => {
                    // Their last word: over, if we had sent our RR73 or 73.
                    if matches!(before, Step::Tx4 | Step::Tx5) {
                        self.done = true;
                        self.complete(now);
                    }
                    before
                }
            };
            if !bye {
                self.done = false;
            }
            self.tx_odd = !h.odd;
            if self.next != before {
                self.repeats = 0;
            }
        }
    }

    /// The current message went out (call when its transmission starts).
    pub fn sent(&mut self, now: u64) {
        self.repeats += 1;
        match self.next {
            // RR73 and 73 close the contact: logged, nothing more unless they repeat.
            Step::Tx4 | Step::Tx5 => {
                self.complete(now);
                self.done = true;
            }
            Step::Tx6 => {}
            _ => {
                if self.repeats >= self.max_repeats {
                    self.done = true;
                }
            }
        }
    }

    pub fn halt(&mut self) {
        self.done = true;
    }

    fn complete(&mut self, now: u64) {
        if self.logged {
            return;
        }
        if let Some(d) = &self.dx
            && d.rcvd.is_some()
        {
            self.logged = true;
            self.finished = Some(Finished { partner: d.clone(), end: now });
        }
    }

    /// A contact that has just finished, once.
    pub fn take_finished(&mut self) -> Option<Finished> {
        self.finished.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(text: &str, snr: i32, odd: bool) -> Heard {
        Heard { text: text.into(), snr, odd }
    }

    #[test]
    fn parses_the_usual_messages() {
        assert_eq!(parse("CQ TA4SO KM46"), Parsed::Cq { from: "TA4SO".into(), grid: Some("KM46".into()) });
        assert_eq!(parse("CQ DX DL1ABC JO62"), Parsed::Cq { from: "DL1ABC".into(), grid: Some("JO62".into()) });
        assert_eq!(parse("CQ <PA/F4MFS>"), Parsed::Cq { from: "PA/F4MFS".into(), grid: None });
        assert_eq!(parse("TA4SO DL1ABC JO62"), Parsed::Directed { to: "TA4SO".into(), from: "DL1ABC".into(), payload: Payload::Grid("JO62".into()) });
        assert_eq!(parse("TA4SO DL1ABC -07"), Parsed::Directed { to: "TA4SO".into(), from: "DL1ABC".into(), payload: Payload::Report(-7) });
        assert_eq!(parse("TA4SO DL1ABC R+03"), Parsed::Directed { to: "TA4SO".into(), from: "DL1ABC".into(), payload: Payload::RReport(3) });
        assert_eq!(parse("TA4SO DL1ABC RR73").to_owned(), Parsed::Directed { to: "TA4SO".into(), from: "DL1ABC".into(), payload: Payload::Rr73 });
        assert_eq!(parse("<PA/F4MFS> EU6DZ KO35"), Parsed::Directed { to: "PA/F4MFS".into(), from: "EU6DZ".into(), payload: Payload::Grid("KO35".into()) });
        assert_eq!(parse("TNX 73 GL"), Parsed::Other);
        assert_eq!(format_report(-5), "-05");
        assert_eq!(format_report(3), "+03");
    }

    #[test]
    fn answering_a_cq_to_the_log() {
        let mut s = Sequencer::new("TA4SO", "KM46ab");
        assert!(s.pick(&heard("CQ DL1ABC JO62", -12, false), 100));
        assert_eq!((s.next, s.tx_odd), (Step::Tx1, true));
        assert_eq!(s.message(Step::Tx1), "DL1ABC TA4SO KM46");
        s.sent(115);
        // Someone else's traffic and a call to another station change nothing.
        s.heard(&[heard("CQ EA7HJ IM76", -3, false), heard("G4ABC DL1ABC -10", -11, false)], 130);
        assert_eq!(s.next, Step::Tx1);
        s.heard(&[heard("TA4SO DL1ABC -09", -11, false)], 130);
        assert_eq!(s.next, Step::Tx3);
        assert_eq!(s.message(Step::Tx3), "DL1ABC TA4SO R-12");
        s.sent(145);
        s.heard(&[heard("TA4SO DL1ABC RR73", -10, false)], 160);
        assert_eq!(s.next, Step::Tx5);
        assert!(s.take_finished().is_none());
        s.sent(175);
        let f = s.take_finished().expect("logged after 73");
        assert_eq!((f.partner.call.as_str(), f.partner.grid.as_deref(), f.partner.sent, f.partner.rcvd), ("DL1ABC", Some("JO62"), Some(-12), Some(-9)));
        assert!(s.done);
        assert!(s.take_finished().is_none(), "logged once");
    }

    #[test]
    fn calling_cq_to_the_log() {
        let mut s = Sequencer::new("TA4SO", "KM46");
        s.call_cq(false);
        assert_eq!(s.message(Step::Tx6), "CQ TA4SO KM46");
        s.sent(0);
        s.sent(30);
        assert!(!s.done, "CQ goes on");
        s.heard(&[heard("TA4SO OE3XYZ JN88", 2, true)], 45);
        assert_eq!((s.next, s.tx_odd), (Step::Tx2, false));
        assert_eq!(s.message(Step::Tx2), "OE3XYZ TA4SO +02");
        s.sent(60);
        // A second caller is left alone while we work the first.
        s.heard(&[heard("TA4SO EA7HJ IM76", -5, true), heard("TA4SO OE3XYZ R-04", 1, true)], 75);
        assert_eq!(s.next, Step::Tx4);
        assert_eq!(s.dx.as_ref().unwrap().call, "OE3XYZ");
        s.sent(90);
        let f = s.take_finished().expect("logged with RR73");
        assert_eq!((f.partner.sent, f.partner.rcvd), (Some(2), Some(-4)));
        // They did not get our RR73 and send the report again: RR73 again, not logged twice.
        s.heard(&[heard("TA4SO OE3XYZ R-04", 0, true)], 105);
        assert!(!s.done && s.next == Step::Tx4);
        s.sent(120);
        assert!(s.take_finished().is_none());
        s.heard(&[heard("TA4SO OE3XYZ 73", 0, true)], 135);
        assert!(s.done);
        // The next caller after the finished contact is answered.
        s.heard(&[heard("TA4SO EA7HJ IM76", -5, true)], 150);
        assert_eq!((s.dx.as_ref().unwrap().call.as_str(), s.next, s.done), ("EA7HJ", Step::Tx2, false));
        assert_eq!(s.message(Step::Tx2), "EA7HJ TA4SO -05");
    }

    #[test]
    fn non_standard_calls_as_wsjtx_sends_them() {
        let mut s = Sequencer::new("TA4SO", "KM47");
        assert!(s.pick(&heard("CQ C9/ZS6WAB", -3, false), 0));
        let all: Vec<String> = Step::ALL.iter().map(|&st| s.message(st)).collect();
        assert_eq!(all, ["<C9/ZS6WAB> TA4SO KM47", "<C9/ZS6WAB> TA4SO -03", "<C9/ZS6WAB> TA4SO R-03", "<C9/ZS6WAB> TA4SO RR73", "<C9/ZS6WAB> TA4SO 73", "CQ TA4SO KM47"]);
        for m in &all {
            assert!(crate::ft8::message::pack(m).is_some(), "{m}");
        }
        // Their report with our call plain and theirs hashed goes on as usual.
        s.heard(&[heard("TA4SO <C9/ZS6WAB> -07", -4, false)], 15);
        assert_eq!(s.next, Step::Tx3);

        let mut s = Sequencer::new("TA4SO/MM", "KM47");
        s.pick(&heard("CQ DL1ABC JO62", -8, false), 0);
        let all: Vec<String> = Step::ALL.iter().map(|&st| s.message(st)).collect();
        assert_eq!(all, ["<DL1ABC> TA4SO/MM", "DL1ABC <TA4SO/MM> -08", "DL1ABC <TA4SO/MM> R-08", "<DL1ABC> TA4SO/MM RR73", "<DL1ABC> TA4SO/MM 73", "CQ TA4SO/MM"]);
        for m in &all {
            assert!(crate::ft8::message::pack(m).is_some(), "{m}");
        }
        s.heard(&[heard("<TA4SO/MM> DL1ABC -05", -8, false)], 15);
        assert_eq!(s.next, Step::Tx3);
    }

    #[test]
    fn gives_up_without_an_answer() {
        let mut s = Sequencer::new("TA4SO", "KM46");
        s.pick(&heard("CQ DL1ABC JO62", -12, false), 0);
        for i in 0..6 {
            assert!(!s.done);
            s.sent(i * 30);
        }
        assert!(s.done);
    }

    #[test]
    fn an_unasked_caller_is_answered() {
        let mut s = Sequencer::new("TA4SO", "KM46");
        s.pick(&heard("TA4SO SP9UNU -05", -8, false), 0);
        assert_eq!((s.next, s.dx.as_ref().unwrap().rcvd), (Step::Tx3, Some(-5)));
        assert_eq!(s.message(Step::Tx3), "SP9UNU TA4SO R-08");
    }
}
