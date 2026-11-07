# QO-ALL

Full-duplex SSB transceiver for the narrow-band transponder of the QO-100 (Es'hail-2)
geostationary satellite. Listen on 10 GHz and transmit on 2.4 GHz at the same time with
one ADALM-Pluto (or a similar AD936x radio over Ethernet or USB).

**Status: first test version (0.1.0).** Receiving, beacon lock, FT8/FT4/CW decoding
and SSB transmitting with PTT, VOX and a tune tone work on the built-in simulator; they
have not been tried with a real ADALM-Pluto yet.

## Trying it

Windows: unpack `QO-ALL-<version>-windows-x64.zip` and run `qo-all.exe`. It starts on
the **simulator** (no radio needed): the transponder with beacons and some users, a
drifting LNB, and your own transmission coming back a quarter second later. Choose the
radio under Settings (e.g. `ip:192.168.2.1`). `qo-all.exe --diagnose ip:192.168.2.1`
prints what the program finds without opening the window.

Transmitting is off until **TX enabled** is switched on, and stays inside the
narrow-band uplink, never on the beacons. The Pluto's transmit LO sits at 2399.9 MHz,
below the uplink, so its leakage and image stay out of the transponder.

## Planned

- Spectrum and waterfall of the whole transponder (10489.500–10490.000 MHz)
- Click a signal to listen (USB, LSB, CW); automatic or manual lock
- Transmit on the listened frequency, or with a shift; uplink = downlink − 8089.5 MHz
- LNB drift corrected from the lower CW beacon, also applied to the uplink
- Microphone and speaker selection, VOX for headsets (threshold, hang time, pre-roll,
  anti-VOX), PTT, output power, time-out timer
- CW, FT8 and FT4 decoding (transmitting them comes later)
- Transmission only inside the narrow-band uplink, never on the beacons

## Building

Rust 2024 (`cargo build --release`). At run time libiio 0.x is needed (`libiio.dll` on
Windows, `libiio.so.0` on Linux).

## Heritage

The receiver, beacon lock, CW and FT8/FT4 decoders come from
[SeDiR](https://github.com/sozden/SeDiR). The FT8/FT4 code tables come from WSJT-X
(GPL-3.0); see `tools/gen_ft8_tables.sh`.

## License

GPL-3.0-or-later.

---

# QO-ALL (Türkçe)

QO-100 (Es'hail-2) uydusunun dar bant transponderi için full duplex SSB alıcı-verici.
Tek bir ADALM-Pluto (veya Ethernet/USB ile bağlı benzer AD936x cihaz) ile 10 GHz'de
dinlerken aynı anda 2.4 GHz'de yayın yapılır.

**Durum: ilk deneme sürümü (0.1.0).** Alma, beacon kilidi, FT8/FT4/CW çözme ve PTT,
VOX ve ton ile SSB yayın yerleşik simülatörde çalışıyor; gerçek ADALM-Pluto ile henüz
denenmedi. Program simülatörle açılır; cihaz Ayarlar'dan seçilir (ör. `ip:192.168.2.1`).
Yayın "TX etkin" açılmadan yapılmaz ve yalnızca dar bant uplink içinde, beacon'lar
dışında olur.

Planlananlar: tüm transponderin spektrum ve şelalesi; tıklayarak dinleme (USB, LSB,
CW), otomatik veya elle kilit; dinlenen frekansta ya da shift ile yayın; alt CW
beacon'dan LNB kayması düzeltmesi (uplink'e de uygulanır); mikrofon ve hoparlör seçimi,
kulaklıklı mikrofon için VOX (eşik, bekleme, ön tampon, anti-VOX), PTT, çıkış gücü,
zaman aşımı; CW, FT8, FT4 çözme. Yayın yalnızca dar bant uplink içinde, beacon'larda asla.
