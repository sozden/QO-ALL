# QO-ALL

Full-duplex SSB and FT8/FT4 transceiver for the narrow-band transponder of the QO-100
(Es'hail-2) geostationary satellite. Listen on 10 GHz and transmit on 2.4 GHz at the same
time with one ADALM-Pluto (or a similar AD936x radio over Ethernet or USB).

**Status: test versions (0.3).** SSB with PTT and VOX, the beacon lock and the automatic
uplink correction are in use with a Pluto on Ethernet. FT8/FT4 transmitting is new and
has been tried on the built-in simulator.

## Trying it

Windows: unpack `QO-ALL-<version>-windows-x64.zip` and run `qo-all.exe`. It starts on
the **simulator** (no radio needed): the transponder with beacons and some users, a
drifting LNB, and your own transmission coming back a quarter second later. Choose the
radio under Settings (e.g. `192.168.2.1`). `qo-all.exe --diagnose 192.168.2.1` prints
what the program finds without opening the window.

Transmitting is off until **TX enabled** is switched on, and stays inside the
narrow-band uplink, never on the beacons.

## What it does

- Spectrum and waterfall of the whole transponder (10489.500–10490.000 MHz); click a
  signal to listen (USB, CW), with automatic or manual lock; RIT and XIT
- Transmit on the listened frequency, or with a shift; uplink = downlink − 8089.5 MHz
- LNB drift corrected from the lower CW beacon
- **Automatic uplink correction**: your own transmission is found on the downlink
  (voice, tone or FT8) and the uplink moved so that it comes down where you listen
- Microphone and speaker selection, VOX for headsets (threshold, hang time, pre-roll,
  anti-VOX), PTT, compressor, output power, time-out timer, tune tone
- **CW sending**: a text box with ready messages, a straight key and an iambic paddle
  (mouse or keyboard: Space, [ and ]), speed in WPM, sidetone; the carrier on the dial
- **Drive** for FT8/FT4 and CW: the output level through the audio, as WSJT-X's Pwr
- **FT8 and FT4** in a WSJT-X-like panel: band activity, the contact's messages, Tx1–Tx6,
  CQ, Enable Tx / Halt, automatic sequencing from a CQ or an answer to the 73
- Finished contacts go into an **ADIF log** (QO-ALL's own, or shared with WSJT-X's
  `wsjtx_log.adi`), with the satellite fields LoTW expects (`PROP_MODE SAT`,
  `SAT_NAME QO-100`, 13cm/3cm). Decodes are coloured against the log: a new station's
  CQ green, a worked station from a new locator purple, worked ones dim, calls to you red;
  the country of each CQ is shown, and a new DXCC entity or grid square can be shown
  and/or sounded as an alert (country file cty.dat taken from WSJT-X or JTDX)
- Ethernet radios run at 0.6 MS/s with FIR filters in the radio (a Pluto on Ethernet
  takes only about 5 MB/s); USB at 2.4 MS/s

## Building

Rust 2024 (`cargo build --release`). At run time libiio 0.x is needed (`libiio.dll` on
Windows, `libiio.so.0` on Linux).

## Heritage

The receiver, beacon lock, CW and FT8/FT4 decoders come from
[SeDiR](https://github.com/sozden/SeDiR). The FT8/FT4 code tables and message encoding
come from WSJT-X (GPL-3.0); see `tools/gen_ft8_tables.sh`.

## License

GPL-3.0-or-later.

---

# QO-ALL (Türkçe)

QO-100 (Es'hail-2) uydusunun dar bant transponderi için full duplex SSB ve FT8/FT4
alıcı-verici. Tek bir ADALM-Pluto (veya Ethernet/USB ile bağlı benzer AD936x cihaz) ile
10 GHz'de dinlerken aynı anda 2.4 GHz'de yayın yapılır.

**Durum: deneme sürümleri (0.3).** PTT ve VOX'lu SSB, beacon kilidi ve otomatik uplink
düzeltmesi Ethernet'li Pluto ile kullanılıyor. FT8/FT4 gönderimi yeni; yerleşik
simülatörde denendi.

Program simülatörle açılır; cihaz Ayarlar'dan seçilir (ör. `192.168.2.1`). Yayın
"TX etkin" açılmadan yapılmaz ve yalnızca dar bant uplink içinde, beacon'lar dışında olur.

Özellikler: tüm transponderin spektrum ve şelalesi, tıklayarak dinleme (USB, CW),
otomatik/elle kilit, RIT ve XIT; dinlenen frekansta ya da shift ile yayın; beacon'dan LNB
düzeltmesi; kendi sinyalinden **otomatik uplink düzeltmesi**; mikrofon/hoparlör seçimi,
kulaklık için VOX, PTT, kompresör, çıkış gücü, zaman aşımı, ton; WSJT-X benzeri
**FT8/FT4** paneli (bant etkinliği, QSO, Tx1–Tx6, CQ, otomatik sıra); **CW gönderme**
(metin, hazır mesajlar, düz anahtar, iambik paddle, WPM, yan ton); FT8/FT4 ve CW için
**Drive**; yeni DXCC ve yeni lokatör için görsel/sesli uyarı (cty.dat WSJT-X/JTDX'ten); biten QSO'lar
**ADIF loguna** (QO-ALL'ın kendi ya da WSJT-X'in `wsjtx_log.adi` dosyası), LoTW'nin
istediği uydu alanlarıyla. Çözülen mesajlar loga göre renklenir: yeni istasyonun CQ'su
yeşil, çalışılmış istasyonun yeni lokatörü mor, çalışılmışlar soluk, size gelenler kırmızı.
Ethernet'te 0.6 MS/s (cihazda FIR filtresi), USB'de 2.4 MS/s.
