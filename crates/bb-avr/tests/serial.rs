//! USART and SoftwareSerial tests on real firmware: fast-path bytes and decoded pin waveforms.

mod common;
use bb_avr::Pin;
use common::*;

const TXD: Pin = Pin::new('D', 1);
const RXD: Pin = Pin::new('D', 0);

#[test]
fn serial_println_at_9600_and_115200_bytes_and_waveform() {
    let mut avr = load("serial_hello", "uno");
    let ev = run_ms(&mut avr, 400.0);
    let bytes = avr.take_usart_tx(0);
    let text: String = bytes.iter().map(|b| b.data as u8 as char).collect();
    assert!(
        text.starts_with("Hello, Breadbox!\r\nFast lane 115200\r\nn=0\r\nn=1\r\n"),
        "{text:?}"
    );
    // Split the TX pin waveform at the end of the 9600-baud line.
    let split = bytes[17].cycle; // '\n' of the first line
    let e = line_edges(&ev, TXD);
    let slow: Vec<(u64, bool)> = e.iter().copied().filter(|x| x.0 <= split).collect();
    let fast: Vec<(u64, bool)> = e.iter().copied().filter(|x| x.0 > split).collect();
    // Arduino's Serial.begin uses U2X: 9600 → UBRR 207 (9615.4 baud), 115200 → UBRR 16 (117647).
    let slow_bytes: Vec<u8> = decode_frames(&slow, 16e6 / (8.0 * 208.0), 8, None, 1)
        .iter()
        .map(|f| f.data as u8)
        .collect();
    let fast_frames = decode_frames(&fast, 16e6 / (8.0 * 17.0), 8, None, 1);
    let fast_bytes: Vec<u8> = fast_frames.iter().map(|f| f.data as u8).collect();
    assert_eq!(text_of(&slow_bytes), "Hello, Breadbox!\r\n");
    assert!(
        text_of(&fast_bytes).starts_with("Fast lane 115200\r\nn=0\r\n"),
        "{}",
        text_of(&fast_bytes)
    );
    assert!(fast_frames.iter().all(|f| f.stop_ok));
    // Bit timing on the pin: every edge sits on the 9615.4-baud bit grid of its frame.
    // Frames start on the free-running baud clock, so every edge of every frame sits on one grid.
    let bit = 8 * 208;
    let first_start = slow.iter().find(|x| !x.1).unwrap().0;
    for &(c, _) in slow.iter().filter(|x| x.0 >= first_start) {
        assert_eq!((c - first_start) % bit, 0, "edge off the bit grid");
    }
    assert!((avr.usart_baud(0) - 117_647.06).abs() < 0.1);
}

fn avr_bytes_end(f: &Frame, baud: f64, bits: u32) -> u64 {
    f.start + (16e6 / baud * bits as f64).round() as u64
}

fn text_of(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn usart_frame_formats_parity_and_errors() {
    let mut avr = load("usart_formats", "uno");
    avr.set_pin_input(RXD, Some(true));
    let mut ev = Vec::new();
    let until = ms(30.0);
    run_until(&mut avr, until, &mut (), &mut ev);
    let e = line_edges(&ev, TXD);
    // 7E2 at 9615 baud.
    let f72 = decode_frames(&e, 16e6 / (8.0 * 208.0), 7, Some(false), 2);
    let s72: String = f72.iter().take(3).map(|f| f.data as u8 as char).collect();
    assert_eq!(s72, "7E2");
    assert!(f72.iter().take(3).all(|f| f.parity_ok && f.stop_ok));
    // 8O1 at 19230.8 baud (U2X off, UBRR=51).
    let baud = 16e6 / (16.0 * 52.0);
    let end_7e2 = avr_bytes_end(&f72[2], 16e6 / (8.0 * 208.0), 11);
    let later: Vec<(u64, bool)> = e.iter().copied().filter(|x| x.0 >= end_7e2).collect();
    let f81 = decode_frames(&later, baud, 8, Some(true), 1);
    let s81: String = f81.iter().take(3).map(|f| f.data as u8 as char).collect();
    assert_eq!(s81, "8O1");
    assert!(f81.iter().take(3).all(|f| f.parity_ok && f.stop_ok));
    avr.take_usart_tx(0);
    // RX: good frame, bad parity, bad stop bit, good frame.
    let start = avr.cycle() + 1_000;
    let end = bitbang(
        &mut avr,
        RXD,
        start,
        baud,
        b"xyzw",
        20_000,
        Some(true),
        &[(1, "parity"), (2, "stop")],
    );
    run_until(&mut avr, end + ms(5.0), &mut (), &mut ev);
    assert_eq!(usart_text(&mut avr, 0), "x!#w");
}

#[test]
fn usart_receiver_tolerates_baud_mismatch() {
    // An ESP-01 sends at exactly 115200 while the UNO runs at 117647 (U2X, UBRR=16): -2.1 %.
    let mut avr = load("usart_formats", "uno");
    avr.set_pin_input(RXD, Some(true));
    let mut ev = Vec::new();
    run_until(&mut avr, ms(30.0), &mut (), &mut ev);
    avr.take_usart_tx(0);
    // The sketch is at 19230.8 baud now; send at 19230.8 * 1.02.
    let start = avr.cycle() + 100;
    let end = bitbang(
        &mut avr,
        RXD,
        start,
        16e6 / (16.0 * 52.0) * 1.02,
        b"ok",
        12_000,
        Some(true),
        &[],
    );
    run_until(&mut avr, end + ms(3.0), &mut (), &mut ev);
    assert_eq!(usart_text(&mut avr, 0), "ok");
}

#[test]
fn usart_injected_bytes_fast_path() {
    let mut avr = load("acomp", "uno");
    let mut ev = Vec::new();
    run_until(&mut avr, ms(5.0), &mut (), &mut ev);
    avr.usart_inject(0, b"?");
    run_until(&mut avr, ms(10.0), &mut (), &mut ev);
    assert_eq!(usart_text(&mut avr, 0), "aco=0 rises=0\r\n");
}

#[test]
fn software_serial_tx_and_rx_at_9600() {
    let mut avr = load("softserial", "uno");
    let rx = Pin::new('B', 2); // D10
    let tx = Pin::new('B', 3); // D11
    avr.set_pin_input(rx, Some(true));
    let mut ev = Vec::new();
    run_until(&mut avr, ms(30.0), &mut (), &mut ev);
    let tx_frames = decode_frames(&line_edges(&ev, tx), 9600.0, 8, None, 1);
    let s: String = tx_frames.iter().map(|f| f.data as u8 as char).collect();
    assert_eq!(s, "hello soft\r\n");
    assert!(tx_frames.iter().all(|f| f.stop_ok));
    // SoftwareSerial bit timing is calibrated in software: check the bit period ≈ 104.17 µs.
    let e = line_edges(&ev, tx);
    let first = e.iter().find(|x| !x.1).unwrap().0;
    let h_end = e.iter().find(|x| x.0 > first && x.1).unwrap().0; // 'h' = 0x68: start + bits 0..2 low
    let bit_us = (h_end - first) as f64 / 16.0 / 4.0;
    assert!((bit_us - 104.17).abs() < 1.5, "bit {bit_us} µs");
    // RX: bytes bit-banged with exact 9600-baud timing into D10, 3 ms apart (half duplex).
    let n0 = ev.len();
    let start = avr.cycle() + 1_000;
    let end = bitbang(&mut avr, rx, start, 9600.0, b"abc", 48_000, None, &[]);
    run_until(&mut avr, end + ms(5.0), &mut (), &mut ev);
    assert_eq!(usart_text(&mut avr, 0), "abc");
    let echo = decode_frames(&line_edges(&ev[n0..], tx), 9600.0, 8, None, 1);
    let s: String = echo.iter().map(|f| f.data as u8 as char).collect();
    assert_eq!(s, "ABC");
}
