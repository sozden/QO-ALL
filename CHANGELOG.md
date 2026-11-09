# Changelog

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
