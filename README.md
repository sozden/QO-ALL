# QO-ALL

Full-duplex SSB transceiver for the narrow-band transponder of the QO-100 (Es'hail-2)
geostationary satellite. Listen on 10 GHz and transmit on 2.4 GHz at the same time with
one ADALM-Pluto (or a similar AD936x radio over Ethernet or USB).

**Status: early development.** Nothing transmits yet.

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

**Durum: geliştirmenin başı.** Henüz yayın yapmıyor.

Planlananlar: tüm transponderin spektrum ve şelalesi; tıklayarak dinleme (USB, LSB,
CW), otomatik veya elle kilit; dinlenen frekansta ya da shift ile yayın; alt CW
beacon'dan LNB kayması düzeltmesi (uplink'e de uygulanır); mikrofon ve hoparlör seçimi,
kulaklıklı mikrofon için VOX (eşik, bekleme, ön tampon, anti-VOX), PTT, çıkış gücü,
zaman aşımı; CW, FT8, FT4 çözme. Yayın yalnızca dar bant uplink içinde, beacon'larda asla.
