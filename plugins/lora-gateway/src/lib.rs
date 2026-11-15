//! LoRa + virtual gateway (PLUG-5): enables bb-radio's LoRa medium (path loss by distance and
//! terrain, airtime from SF/BW/CR, duty-cycle limits) and its multi-SF gateway, adds the
//! REYAX RYLR896 module part, and runs a payload decoder next to the gateway: every uplink
//! the gateway forwards to `lora/<net>/<addr>/up` is parsed (`k=v,k=v` text or JSON) and
//! republished as JSON on `lora/<net>/<addr>/decoded` with RSSI/SNR — like a network server's
//! payload formatter, ready for dashboards.

use bb_plugin_sdk::prelude::*;

struct Decoder {
    subscribed: bool,
    count: u64,
}

fn parse_payload(data: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(data)
        && v.is_object()
    {
        return v;
    }
    let mut m = serde_json::Map::new();
    for (i, item) in data
        .split([',', ';', ' '])
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        match item.split_once(['=', ':']) {
            Some((k, v)) => {
                let val = v
                    .trim()
                    .parse::<f64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::from(v.trim()));
                m.insert(k.trim().to_string(), val);
            }
            None => {
                m.insert(format!("field{i}"), Value::from(item));
            }
        }
    }
    Value::Object(m)
}

impl RadioExtension for Decoder {
    fn init(_cfg: &Config) -> Result<Self, String> {
        Ok(Decoder {
            subscribed: false,
            count: 0,
        })
    }
    fn step(&mut self, ctx: &Ctx, _now_s: f64) -> Option<f64> {
        if !self.subscribed {
            self.subscribed = ctx.mqtt_subscribe("lora/+/+/up");
        }
        while let Some((topic, payload)) = ctx.mqtt_poll() {
            let parts: Vec<&str> = topic.split('/').collect();
            let [_, net, addr, "up"] = parts.as_slice() else {
                continue;
            };
            let Ok(up) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            let data = up.get("data").and_then(|d| d.as_str()).unwrap_or("");
            let mut out = parse_payload(data);
            if let Some(o) = out.as_object_mut() {
                for k in ["rssi", "snr", "sf"] {
                    if let Some(v) = up.get(k) {
                        o.insert(format!("_{k}"), v.clone());
                    }
                }
            }
            self.count += 1;
            let t = format!("lora/{net}/{addr}/decoded");
            ctx.mqtt_publish(&t, out.to_string().as_bytes(), 0, false);
        }
        None
    }
}

export_radio!(Decoder);
