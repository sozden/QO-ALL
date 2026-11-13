# Changelog

## 0.7.1 — 2026-11-13

- **Calls with a prefix in FT8/FT4**: a station such as C9/ZS6WAB can be answered. Its
  call goes as WSJT-X sends it, hashed in angle brackets beside ours
  (`<C9/ZS6WAB> TA4SO KM47`); before, the message could not be encoded and nothing was
  sent. With a non-standard call of our own (TA4SO/MM) CQ, Tx1, RR73 and 73 go as type 4
  messages and the reports with our call hashed. Calls with /P and /R can be sent too.
- **Calmer, clearer window**: the modes (USB, CW, FT8, FT4) as one row of keys at the
  top; receive in blue, transmit in amber, red kept for being on the air, errors and
  the beacons; a line of the role's colour on the receive and transmit displays, the
  signal in figures under the receive frequency (S units, dBm, S/N), the uplink, linked
  or shift and "ON AIR" under the transmit one; the spectrum filled brighter under the
  peaks.

## 0.7.0 — 2026-11-12

- **S-meter**: a classic needle meter beside the receive frequency, S1–S9 and +10…+60 dB
  over an S/N scale in dB. The S units count from the noise floor (S0 = the noise, 6 dB
  per S unit, S9 = 54 dB above it), so it needs no calibration. Backlit warm on the paper
  theme, a glowing night face on the night theme.
- **dBm**: the receive level in dBm in the receiver's caption and a dBm scale on the
  spectrum, estimated from the radio's gain; calibrated in the settings in one step
  (feed a known level, press Calibrate).
- **Band plan on the spectrum**: CW, digital (FT8), digital/SSB, SSB and the beacons as a
  coloured strip under the spectrum.
- **FT8/FT4 transmit at once**: a call picked (or an answer due) in the own slot goes out
  straight away, as in WSJT-X, its beginning cut so the rest keeps the slot's timing (up
  to 4.5 s late in FT8, 1.8 s in FT4); later than that the next slot is used.
- **LibreSDR**: an AD936x radio on the network that is not a Pluto (LibreSDR, gigabit)
  is switched to 2.4 MS/s by itself; at 0.6 MS/s the Tezuka firmware dropped the
  connection after a few seconds (error -10054).
- **Larger log form**: the new/correct contact fields are bigger, with a larger font.

## 0.6.0 — 2026-11-09

- **Receivers too**: besides the ADALM-Pluto and LibreSDR (AD936x, receive and
  transmit), an RTL-SDR on USB or over the network (rtl_tcp) and a HackRF One on USB,
  receive only. A frequency error setting (ppm) for radios without a TCXO.
- **TX enabled** in the settings: a radio that can transmit is used for it only when
  ticked, after a warning to check the antenna and its SWR; otherwise receive only.
- **Serial PTT**: DTR or RTS goes high while transmitting, in every mode (for an
  amplifier, a sequencer or another radio).
- **Serial CW key**: a straight key or a paddle on CTS/DSR, supplied from DTR/RTS.
- **Microphone and speaker on the main screen**, changed at once without restarting.
- **Settings**: only a change of the radio's set-up restarts it; the rest (gain, station,
  log, serial ports) applies at once.
- **The log is always in view** at the bottom of the window; the new/correct form takes
  two rows.

The 32-bit Windows package has no RTL-SDR/HackRF USB libraries yet: there it works with
the AD936x radios and with an RTL-SDR over rtl_tcp.

## 0.5.0 — 2026-11-09

- **Tune in every mode**: the tune button is there in SSB too, a steady 1 kHz tone, to
  see where the transmission comes down (in CW on the carrier, in FT8/FT4 in the middle
  of the chosen audio frequency).
- **Log window** (LOG tab, and QSO | LOG above the FT8/FT4 contact column): the whole
  ADIF log with date, UTC, call, mode, reports sent and received, name and locator;
  follows the file as contacts are added, searchable by call.
- **Contacts by hand**: SSB and CW contacts entered into the same ADIF log as FT8/FT4
  (with the satellite fields); a double click on a contact opens it for correction, or
  deletion. Before the first change the log is kept once beside it (`.adi.bak`).
- The name of the other operator goes into the log (`NAME`).
- **Linux** package (x86-64, Ubuntu 24.04 or newer): libiio from the distribution, a udev
  rule for a Pluto on USB, a menu entry.
- The chosen decoder tab is remembered.

- **Windows 32-bit** package (`windows-x86`): Analog Devices publishes libiio for 64-bit
  Windows only, so its `libiio.dll` is built from source (libiio 0.26 with libxml2 and
  libusb inside, `tools/build-libiio-w32.sh`); nothing else to install.

## 0.4.0 — 2026-11-08 (first public version)

Everything below in one package; tried on the air with an ADALM-Pluto on Ethernet
(SSB, FT8), on the built-in simulator otherwise.

- **Receiving**: spectrum and waterfall of the whole narrow-band transponder, click to
  listen (USB, CW) with automatic or manual lock, RIT, LNB drift followed on the lower
  beacon; CW, FT8 and FT4 decoding.
- **Transmitting, full duplex**: SSB with PTT or VOX (threshold, hang, pre-roll,
  anti-VOX), compressor, tune tone, output power, time-out timer; on the listened
  frequency, with a shift, or with XIT; never outside the narrow-band uplink or on a
  beacon. The transmit display shows where the signal comes down (10 GHz), the uplink
  sent (2.4 GHz) under it.
- **Automatic uplink correction**: the own transmission is found on the downlink (voice,
  tone, FT8 or CW) and the uplink moved so that it comes down where you listen.
- **FT8 and FT4** in a WSJT-X-like panel: band activity, the contact's messages,
  Tx1–Tx6, CQ, Enable Tx, Halt, automatic sequence, drive; finished contacts in an ADIF
  log (QO-ALL's own or WSJT-X's) with the satellite fields LoTW expects; decodes
  coloured against the log (new, worked, new locator, to me), the country of each CQ,
  and optional shown/heard alerts for a new DXCC entity or grid square (country file
  taken from WSJT-X or JTDX).
- **CW sending**: text box and ready messages, straight key and iambic paddle (mouse or
  Space, [ and ]), WPM, sidetone, drive.
- **Radios**: ADALM-Pluto and similar AD936x over Ethernet (0.6 MS/s with the radio's
  FIR filters: a Pluto on Ethernet takes only about 5 MB/s) or USB (2.4 MS/s); a
  simulator for trying it without a radio.
- **Window**: night and paper themes, English and Turkish, volume and mute at hand.

### Known issues

- Keying CW by hand has a delay of about 0.1–0.15 s (transmit buffers) and the
  sidetone about 0.1 s; sending text is not affected. Work on the key is planned.
- macOS and Linux packages are not made yet; the code builds there (libiio needed).

---

# Değişiklikler (Türkçe)

## 0.4.0 — 2026-11-08 (ilk herkese açık sürüm)

Ethernet'li ADALM-Pluto ile yayında denendi (SSB, FT8); diğerleri yerleşik simülatörde.

- **Alma:** tüm dar bant transponderin spektrum ve şelalesi, tıklayarak dinleme (USB,
  CW), otomatik/elle kilit, RIT, alt beacon'dan LNB kayması takibi; CW, FT8, FT4 çözme.
- **Full duplex gönderme:** PTT ya da VOX'lu SSB, kompresör, ton, çıkış gücü, zaman
  aşımı; dinlenen frekansta, shift ya da XIT ile; dar bant uplink dışına ve beacon'lara
  asla. TX göstergesi sinyalin indiği 10 GHz frekansını, altında 2.4 GHz uplink'i
  gösterir.
- **Otomatik uplink düzeltmesi:** kendi yayınınız downlink'te bulunur (ses, ton, FT8,
  CW) ve uplink, sinyal dinlediğiniz yere insin diye kaydırılır.
- **FT8/FT4:** WSJT-X benzeri panel, otomatik sıra, drive, ADIF log (LoTW uydu
  alanlarıyla), loga göre renklendirme, ülke adları, yeni DXCC/kare uyarıları.
- **CW gönderme:** metin, hazır mesajlar, düz anahtar, iambik paddle, WPM, yan ton.
- **Cihazlar:** Ethernet (0.6 MS/s, cihazda FIR) ya da USB (2.4 MS/s); simülatör.
- **Pencere:** Gece ve Kâğıt temaları, İngilizce ve Türkçe, ses/sessiz.

### Bilinen konular

- Elle CW anahtarlamada yaklaşık 0.1–0.15 s gecikme var; metin gönderimi etkilenmez.
  Anahtar üzerinde çalışılacak.
- macOS ve Linux paketleri henüz yok.
