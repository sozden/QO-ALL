//! QO-ALL - full-duplex SSB transceiver for QO-100.

fn main() {
    println!("QO-ALL {}", env!("CARGO_PKG_VERSION"));
    println!("libiio: {:?}", qo_all::iio::library_status());
    for (uri, description) in qo_all::iio::scan_usb() {
        println!("  {uri} | {description}");
    }
}
