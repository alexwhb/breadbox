//! ESP-AT modem conformance: commands, response formats, unsolicited messages and timing,
//! driven byte by byte at the modem's baud (no MCU).

use bb_core::SimTime;
use bb_radio::espat::EspAtConfig;
use bb_radio::geo::Pos;
use bb_radio::mqtt::Message;
use bb_radio::uart::UartFrame;
use bb_radio::{DeviceId, RadioSpace};

struct Rig {
    space: RadioSpace,
    esp: DeviceId,
    t: SimTime,
    frames: Vec<UartFrame>,
}

impl Rig {
    fn new(baud: u32) -> Rig {
        let mut space = RadioSpace::default();
        let esp = space.add_esp01("U2", Pos::new(2.0, 0.0), EspAtConfig::with_baud(baud));
        space.power(esp, true, SimTime::ZERO);
        let mut r = Rig {
            space,
            esp,
            t: SimTime::ZERO,
            frames: vec![],
        };
        r.run_for(400);
        r
    }

    fn collect(&mut self) {
        let f = self.space.take_uart_tx(self.esp);
        self.frames.extend(f);
    }

    fn run_for(&mut self, ms: u64) {
        let end = self.t + SimTime::from_ms(ms);
        while self.t < end {
            self.t = (self.t + SimTime::from_us(500)).min(end);
            self.space.advance(self.t);
            self.collect();
        }
    }

    /// Sends bytes back to back at the modem's current baud.
    fn send_bytes(&mut self, b: &[u8]) {
        let cfg = self.space.uart_config(self.esp).unwrap();
        for &x in b {
            self.t += cfg.frame_time();
            self.space.uart_rx(self.esp, x, self.t);
            self.collect();
        }
    }

    fn send(&mut self, line: &str) {
        self.send_bytes(format!("{line}\r\n").as_bytes());
    }

    /// Output (at `baud`) of frames starting at or after `from`.
    fn text_from(&self, from: SimTime, baud: u32) -> String {
        let b: Vec<u8> = self
            .frames
            .iter()
            .filter(|f| f.start >= from && f.config.baud == baud)
            .map(|f| f.data as u8)
            .collect();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// Sends `line`, runs until the output since then contains `until` (or 30 s pass). Returns
    /// the output and when the last byte of `until` ended.
    /// Runs until everything the modem queued has left the UART.
    fn settle(&mut self) {
        let end = self
            .frames
            .iter()
            .map(|f| f.end())
            .max()
            .unwrap_or(SimTime::ZERO);
        while self.t <= end {
            self.run_for(1);
        }
    }

    fn cmd(&mut self, line: &str, until: &str) -> (String, SimTime) {
        self.settle();
        let baud = self.space.uart_config(self.esp).unwrap().baud;
        let from = self.t;
        self.send(line);
        let limit = self.t + SimTime::from_secs(30);
        loop {
            let text = self.text_from(from, baud);
            if let Some(i) = text.find(until) {
                let k = i + until.len();
                let frames: Vec<&UartFrame> = self
                    .frames
                    .iter()
                    .filter(|f| f.start >= from && f.config.baud == baud)
                    .collect();
                let end = frames[k - 1].end();
                self.settle();
                return (text, end);
            }
            assert!(self.t < limit, "no {until:?} after {line:?}; got {text:?}");
            self.run_for(5);
        }
    }
}

#[test]
fn boot_banner_then_ready_and_basic_commands() {
    let mut r = Rig::new(115_200);
    let rom: Vec<u8> = r
        .frames
        .iter()
        .filter(|f| f.config.baud == 74_880)
        .map(|f| f.data as u8)
        .collect();
    assert!(String::from_utf8_lossy(&rom).contains("ets Jan  8 2013,rst cause:1, boot mode:(3,6)"));
    assert_eq!(r.text_from(SimTime::ZERO, 115_200), "\r\nready\r\n");
    let ready_at = r
        .frames
        .iter()
        .find(|f| f.config.baud == 115_200)
        .unwrap()
        .start;
    assert!(ready_at >= SimTime::from_ms(300) && ready_at <= SimTime::from_ms(400));
    // Echo on by default: "AT\r\r\n\r\nOK\r\n".
    let (t, _) = r.cmd("AT", "OK\r\n");
    assert_eq!(t, "AT\r\r\n\r\nOK\r\n");
    let (t, _) = r.cmd("ATE0", "OK\r\n");
    assert!(t.ends_with("\r\nOK\r\n"));
    let start = r.t;
    let (t, end) = r.cmd("AT", "OK\r\n");
    assert_eq!(t, "\r\nOK\r\n");
    // Response latency: ~1.2 ms of processing + 6 bytes at 115200.
    let lat = (end - start).as_secs_f64() * 1e3;
    assert!(
        (1.5..2.5).contains(&lat),
        "{lat} ms (includes sending \"AT\\r\\n\")"
    );
    assert_eq!(r.cmd("AT+NOPE", "ERROR\r\n").0, "\r\nERROR\r\n");
    let (t, _) = r.cmd("AT+GMR", "OK\r\n");
    assert!(t.starts_with("AT version:2.2.0.0"));
    assert_eq!(r.cmd("AT+CWMODE?", "OK\r\n").0, "+CWMODE:1\r\n\r\nOK\r\n");
    assert_eq!(r.cmd("AT+CWMODE=3", "OK\r\n").0, "\r\nOK\r\n");
    assert_eq!(r.cmd("AT+CWMODE=9", "ERROR\r\n").0, "\r\nERROR\r\n");
    assert_eq!(r.cmd("AT+CIPMUX?", "OK\r\n").0, "+CIPMUX:0\r\n\r\nOK\r\n");
    assert_eq!(r.cmd("AT+CWJAP?", "OK\r\n").0, "No AP\r\n\r\nOK\r\n");
}

#[test]
fn join_flow_busy_and_failures() {
    let mut r = Rig::new(115_200);
    r.cmd("ATE0", "OK");
    let t0 = r.t;
    r.send("AT+CWJAP=\"SIM-AP\",\"breadbox\"");
    // A command during the join is refused.
    r.run_for(100);
    r.send("AT");
    r.run_for(5);
    let mid = r.text_from(t0, 115_200);
    assert!(mid.contains("busy p...\r\n"), "{mid:?}");
    let mut done = SimTime::ZERO;
    for _ in 0..1000 {
        r.run_for(10);
        if r.text_from(t0, 115_200)
            .contains("WIFI GOT IP\r\n\r\nOK\r\n")
        {
            done = r.t;
            break;
        }
    }
    let t = r.text_from(t0, 115_200);
    assert!(
        t.contains("WIFI CONNECTED\r\nWIFI GOT IP\r\n\r\nOK\r\n"),
        "{t:?}"
    );
    let secs = (done - t0).as_secs_f64();
    assert!((0.6..2.5).contains(&secs), "join took {secs} s");
    let notes = r.space.take_annotations();
    assert!(
        notes
            .iter()
            .any(|n| n.line() == "wifi: AT+CWJAP=\"SIM-AP\" → OK rssi=-33 dBm ip=10.0.0.42"),
        "{notes:?}"
    );
    let (t, _) = r.cmd("AT+CWJAP?", "OK\r\n");
    assert!(
        t.starts_with("+CWJAP:\"SIM-AP\",\"02:00:5e:10:00:01\",6,-33"),
        "{t}"
    );
    assert_eq!(
        r.cmd("AT+CIFSR", "OK\r\n").0,
        "+CIFSR:STAIP,\"10.0.0.42\"\r\n+CIFSR:STAMAC,\"5c:cf:7f:b0:0b:10\"\r\n\r\nOK\r\n"
    );
    assert_eq!(
        r.cmd("AT+CIPSTA?", "OK\r\n").0,
        "+CIPSTA:ip:\"10.0.0.42\"\r\n+CIPSTA:gateway:\"10.0.0.1\"\r\n+CIPSTA:netmask:\"255.255.255.0\"\r\n\r\nOK\r\n"
    );
    let (t, end) = r.cmd("AT+CWLAP", "OK\r\n");
    assert_eq!(
        t,
        "+CWLAP:(3,\"SIM-AP\",-33,\"02:00:5e:10:00:01\",6,-1,-1)\r\n\r\nOK\r\n"
    );
    assert!(
        end - r.frames.iter().rev().find(|f| f.start < end).unwrap().start < SimTime::from_secs(3)
    );
    assert_eq!(
        r.cmd("AT+CWQAP", "DISCONNECT\r\n").0,
        "\r\nOK\r\nWIFI DISCONNECT\r\n"
    );
    // Wrong password → +CWJAP:2 FAIL; unknown SSID → +CWJAP:3 FAIL.
    assert_eq!(
        r.cmd("AT+CWJAP=\"SIM-AP\",\"wrong\"", "FAIL\r\n").0,
        "+CWJAP:2\r\n\r\nFAIL\r\n"
    );
    assert_eq!(
        r.cmd("AT+CWJAP=\"Nope\",\"x\"", "FAIL\r\n").0,
        "+CWJAP:3\r\n\r\nFAIL\r\n"
    );
    // No IP: CIPSTART fails.
    assert!(
        r.cmd("AT+CIPSTART=\"TCP\",\"10.0.0.1\",80", "ERROR\r\n")
            .0
            .contains("ERROR")
    );
}

fn joined() -> Rig {
    let mut r = Rig::new(115_200);
    r.cmd("ATE0", "OK");
    r.cmd(
        "AT+CWJAP=\"SIM-AP\",\"breadbox\"",
        "WIFI GOT IP\r\n\r\nOK\r\n",
    );
    r
}

#[test]
fn tcp_link_send_ipd_status_close() {
    let mut r = joined();
    assert_eq!(
        r.cmd("AT+CIPSTART=\"TCP\",\"echo.sim\",7", "OK\r\n").0,
        "CONNECT\r\n\r\nOK\r\n"
    );
    assert_eq!(
        r.cmd("AT+CIPSTART=\"TCP\",\"echo.sim\",7", "ERROR\r\n").0,
        "ALREADY CONNECTED\r\n\r\nERROR\r\n"
    );
    let (t, _) = r.cmd("AT+CIPSEND=5", "> ");
    assert_eq!(t, "\r\nOK\r\n> ");
    let from = r.t;
    r.send_bytes(b"hello");
    let mut out = String::new();
    for _ in 0..200 {
        r.run_for(2);
        out = r.text_from(from, 115_200);
        if out.contains("+IPD,5:hello") {
            break;
        }
    }
    assert_eq!(out, "\r\nRecv 5 bytes\r\n\r\nSEND OK\r\n\r\n+IPD,5:hello");
    let (t, _) = r.cmd("AT+CIPSTATUS", "OK\r\n");
    assert!(
        t.starts_with("STATUS:3\r\n+CIPSTATUS:0,\"TCP\",\"10.0.0.1\",7,"),
        "{t}"
    );
    assert_eq!(r.cmd("AT+CIPCLOSE", "OK\r\n").0, "CLOSED\r\n\r\nOK\r\n");
    assert_eq!(
        r.cmd("AT+CIPSEND=3", "ERROR\r\n").0,
        "link is not valid\r\n\r\nERROR\r\n"
    );
    assert_eq!(
        r.cmd("AT+CIPSTART=\"TCP\",\"10.0.0.1\",4444", "CLOSED\r\n")
            .0,
        "ERROR\r\nCLOSED\r\n"
    );
    assert_eq!(
        r.cmd("AT+CIPSTART=\"TCP\",\"nowhere.example\",80", "ERROR\r\n")
            .0,
        "DNS Fail\r\n\r\nERROR\r\n"
    );
    // HTTP over the link: the server closes after answering.
    r.cmd("AT+CIPSTART=\"TCP\",\"http.sim\",80", "OK\r\n");
    let req = "GET /status/418 HTTP/1.0\r\n\r\n";
    r.cmd(&format!("AT+CIPSEND={}", req.len()), "> ");
    let from = r.t;
    r.send_bytes(req.as_bytes());
    for _ in 0..300 {
        r.run_for(2);
        if r.text_from(from, 115_200).contains("CLOSED\r\n") {
            break;
        }
    }
    let t = r.text_from(from, 115_200);
    assert!(
        t.contains("+IPD,") && t.contains("HTTP/1.1 418 I'm a teapot") && t.ends_with("CLOSED\r\n"),
        "{t:?}"
    );
}

#[test]
fn multiplexed_links_server_and_two_modems() {
    let mut r = joined();
    // A second ESP-01 on the bench runs a TCP server.
    let b = r
        .space
        .add_esp01("U3", Pos::new(1.0, 2.0), EspAtConfig::default());
    r.space.power(b, true, r.t);
    let mut b_out = vec![];
    let feed = |r: &mut Rig, line: &str| {
        let cfg = r.space.uart_config(b).unwrap();
        for x in format!("{line}\r\n").bytes() {
            r.t += cfg.frame_time();
            r.space.uart_rx(b, x, r.t);
        }
    };
    r.run_for(400);
    for l in [
        "ATE0",
        "AT+CWJAP=\"SIM-AP\",\"breadbox\"",
        "AT+CIPMUX=1",
        "AT+CIPSERVER=1,333",
    ] {
        feed(&mut r, l);
        r.run_for(2500);
        b_out.extend(r.space.take_uart_tx(b));
    }
    r.cmd("AT+CIPMUX=1", "OK\r\n");
    assert_eq!(
        r.cmd("AT+CIPSTART=0,\"TCP\",\"10.0.0.43\",333", "OK\r\n").0,
        "0,CONNECT\r\n\r\nOK\r\n"
    );
    assert_eq!(
        r.cmd("AT+CIPSTART=1,\"UDP\",\"10.0.0.1\",7,1112,0", "OK\r\n")
            .0,
        "1,CONNECT\r\n\r\nOK\r\n"
    );
    assert_eq!(
        r.cmd("AT+CIPMUX=0", "ERROR\r\n").0,
        "link is builded\r\n\r\nERROR\r\n"
    );
    r.cmd("AT+CIPSEND=0,4", "> ");
    r.send_bytes(b"ping");
    r.run_for(200);
    b_out.extend(r.space.take_uart_tx(b));
    let bt: String = b_out
        .iter()
        .filter(|f| f.config.baud == 115_200)
        .map(|f| f.data as u8 as char)
        .collect();
    assert!(
        bt.contains("0,CONNECT\r\n") && bt.contains("+IPD,0,4:ping"),
        "{bt:?}"
    );
    // UDP echo on link 1.
    r.cmd("AT+CIPSEND=1,3", "> ");
    let from = r.t;
    r.send_bytes(b"udp");
    r.run_for(100);
    assert!(
        r.text_from(from, 115_200).contains("+IPD,1,3:udp"),
        "{}",
        r.text_from(from, 115_200)
    );
}

#[test]
fn ping_uart_switch_and_sntp() {
    let mut r = joined();
    let (t, _) = r.cmd("AT+PING=\"10.0.0.1\"", "OK\r\n");
    let ms: u64 = t
        .trim_start_matches("+PING:")
        .split('\r')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((4..=10).contains(&ms), "{t}");
    assert_eq!(
        r.cmd("AT+PING=\"10.0.0.250\"", "ERROR\r\n").0,
        "+PING:TIMEOUT\r\n\r\nERROR\r\n"
    );
    // SNTP from the sim clock (epoch 2026-01-01T00:00:00Z, UTC+1).
    r.cmd("AT+CIPSNTPCFG=1,1,\"pool.ntp.org\"", "OK\r\n");
    r.run_for(500);
    let (t, _) = r.cmd("AT+CIPSNTPTIME?", "OK\r\n");
    assert!(
        t.starts_with("+CIPSNTPTIME:Thu Jan 01 01:00:0") && t.ends_with(" 2026\r\nOK\r\n"),
        "{t}"
    );
    // UART_CUR switches after the OK (sent at the old speed).
    let from = r.t;
    let (t, _) = r.cmd("AT+UART_CUR=9600,8,1,0,0", "OK\r\n");
    assert_eq!(t, "\r\nOK\r\n");
    assert!(
        r.frames
            .iter()
            .filter(|f| f.start >= from)
            .all(|f| f.config.baud == 115_200)
    );
    assert_eq!(r.space.uart_config(r.esp).unwrap().baud, 9600);
    assert_eq!(
        r.cmd("AT+UART_CUR?", "OK\r\n").0,
        "+UART_CUR:9600,8,1,0,0\r\n\r\nOK\r\n"
    );
    // Not persisted: after AT+RST the module is back at 115200.
    r.cmd("AT+RST", "OK\r\n");
    r.run_for(500);
    assert_eq!(r.space.uart_config(r.esp).unwrap().baud, 115_200);
    // UART_DEF persists across resets.
    r.cmd("AT+UART_DEF=57600,8,1,0,0", "OK\r\n");
    r.cmd("AT+RST", "OK\r\n");
    r.run_for(500);
    assert_eq!(r.space.uart_config(r.esp).unwrap().baud, 57_600);
    assert!(
        r.text_from(r.t - SimTime::from_ms(500), 57_600)
            .contains("ready")
    );
    // The saved AP auto-connects after the reboot.
    r.run_for(2500);
    assert!(
        r.text_from(r.t - SimTime::from_ms(3000), 57_600)
            .contains("WIFI GOT IP")
    );
}

#[test]
fn esp_at_mqtt_commands() {
    let mut r = joined();
    let watch = r.space.net.broker_mut().virtual_connect("watch", r.t);
    let out = r
        .space
        .net
        .broker_mut()
        .virtual_subscribe(watch, "esp/#", 0, r.t);
    r.space.net.apply_broker(out);
    assert_eq!(
        r.cmd("AT+MQTTUSERCFG=0,1,\"esp01\",\"\",\"\",0,0,\"\"", "OK\r\n")
            .0,
        "\r\nOK\r\n"
    );
    r.cmd("AT+MQTTCONNCFG=0,30,0,\"esp/lwt\",\"gone\",0,0", "OK\r\n");
    let (t, _) = r.cmd("AT+MQTTCONN=0,\"broker.sim\",1883,1", "OK\r\n");
    assert_eq!(
        t,
        "+MQTTCONNECTED:0,1,\"broker.sim\",\"1883\",\"\",1\r\n\r\nOK\r\n"
    );
    assert_eq!(r.cmd("AT+MQTTSUB=0,\"cmd/#\",1", "OK\r\n").0, "\r\nOK\r\n");
    assert_eq!(
        r.cmd("AT+MQTTSUB=0,\"cmd/#\",1", "OK\r\n").0,
        "ALREADY SUBSCRIBE\r\n\r\nOK\r\n"
    );
    r.cmd("AT+MQTTPUB=0,\"esp/temp\",\"21.5\",0,0", "OK\r\n");
    r.run_for(50);
    let got = r.space.net.broker_mut().virtual_take(watch);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].message, Message::new("esp/temp", "21.5", 0, false));
    // Messages to the subscription arrive as +MQTTSUBRECV.
    let from = r.t;
    let out = r.space.net.broker_mut().virtual_publish(
        watch,
        Message::new("cmd/led", "on", 1, false),
        r.t,
    );
    r.space.net.apply_broker(out);
    r.run_for(50);
    assert_eq!(
        r.text_from(from, 115_200),
        "+MQTTSUBRECV:0,\"cmd/led\",2,on\r\n"
    );
    // Keep-alive pings flow (30 s).
    r.run_for(35_000);
    assert!(
        r.space
            .net
            .log()
            .visible()
            .any(|e| e.text() == "MQTT PINGREQ")
    );
    assert!(
        r.space
            .net
            .broker()
            .clients()
            .contains(&"esp01".to_string())
    );
    assert_eq!(r.cmd("AT+MQTTCLEAN=0", "OK\r\n").0, "\r\nOK\r\n");
    r.run_for(100);
    assert!(
        !r.space
            .net
            .broker()
            .clients()
            .contains(&"esp01".to_string())
    );
    // Clean disconnect → no will.
    assert!(r.space.net.broker_mut().virtual_take(watch).is_empty());
}
