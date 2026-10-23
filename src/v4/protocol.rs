use crate::error::{other, other_without_source, parse_error, Result};
use aes::Aes128;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use flate2::write::GzEncoder;
use flate2::Compression;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::{thread_rng, Rng};
use reqwest::blocking::Client;
use rsa::{BigUint, Pkcs1v15Encrypt, RsaPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::Sha256;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const BASE_URL: &str = "https://gcaptcha4.geetest.com";
const IMG_BASE: &str = "https://static.geetest.com";
const RSA_N: &str = "00C1E3934D1614465B33053E7F48EE4EC87B14B95EF88947713D25EECBFF7E74C7977D02DC1D9451F79DD5D1C10C29ACB6A9B4D6FB7D0A0279B6719E1772565F09AF627715919221AEF91899CAE08C0D686D748B20A3603BE2318CA6BC2B59706592A9219D0BF05C9F65023A21D2330807252AE0066D59CEEFA5F2748EA80BAB81";
const RSA_E: &str = "010001";
const AES_IV: [u8; 16] = [b'0'; 16];
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PowDetail {
    #[serde(default = "default_pow_version")]
    pub version: String,
    #[serde(default)]
    pub bits: u32,
    #[serde(default)]
    pub datetime: String,
    #[serde(default = "default_hashfunc")]
    pub hashfunc: String,
}

fn default_pow_version() -> String {
    "1".to_string()
}

fn default_hashfunc() -> String {
    "md5".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadData {
    #[serde(default)]
    pub captcha_id: String,
    pub lot_number: String,
    pub captcha_type: String,
    pub imgs: String,
    pub ques: Vec<String>,
    pub static_path: String,
    #[serde(default = "default_pt")]
    pub pt: String,
    pub pow_detail: PowDetail,
    pub payload: String,
    pub process_token: String,
    pub payload_protocol: u8,
    #[serde(default)]
    pub guard: bool,
}

fn default_pt() -> String {
    "1".to_string()
}

#[derive(Debug, Default, Deserialize)]
pub struct VerifyData {
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub payload: Option<String>,
    #[serde(default)]
    pub process_token: Option<String>,
    #[serde(default)]
    pub seccode: Option<Seccode>,
    #[serde(default)]
    pub fail_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Seccode {
    pub lot_number: String,
    pub captcha_output: String,
    pub pass_token: String,
    pub gen_time: String,
}

#[derive(Debug, Clone)]
struct Patrol {
    biht: &'static str,
    lib_key: &'static str,
    lib_val: &'static str,
    abo_key: &'static str,
    abo_val: &'static str,
}

impl Patrol {
    fn from_static_path(path: &str) -> Result<Self> {
        let version = path
            .strip_prefix("/v4/static/")
            .unwrap_or(path)
            .trim_end_matches('/');
        let patrol = match version {
            "v1.9.7-0ad79a" => Patrol {
                biht: "1426265548",
                lib_key: "dQFB",
                lib_val: "BoHp",
                abo_key: "(n[5:7]+n[7:9])+.+(n[20:27])+.+(n[10:10]+n[12:12]+n[3:3]+n[7:7])",
                abo_val: "n[7:14]",
            },
            "v1.9.7-fc2ddc" => Patrol {
                biht: "1426265548",
                lib_key: "d4tf",
                lib_val: "9342",
                abo_key: "(n[16:19])+.+(n[2:5]+n[20:23])+.+(n[28:28]+n[18:18]+n[9:9]+n[13:13])",
                abo_val: "n[22:25]",
            },
            "v1.9.8-0748f2" => Patrol {
                biht: "1426265548",
                lib_key: "o6YJ",
                lib_val: "NvAf",
                abo_key: "n[26:28]+n[24:26]",
                abo_val: "n[1:8]",
            },
            "v1.9.8-0c99e9" => Patrol {
                biht: "1426265548",
                lib_key: "seNY",
                lib_val: "QAUJ",
                abo_key: "(n[18:23])+.+(n[20:20]+n[29:29]+n[11:11]+n[6:6])+.+(n[24:27]+n[20:23])",
                abo_val: "n[23:30]",
            },
            "v1.9.8-0d53fd" => Patrol {
                biht: "1426265548",
                lib_key: "oQ9h",
                lib_val: "VQzx",
                abo_key: "(n[16:19]+n[13:16])+.+(n[12:19])",
                abo_val: "n[22:27]",
            },
            _ => {
                return Err(other_without_source(&format!(
                    "unsupported GeeTest static build: {version}"
                )))
            }
        };
        Ok(patrol)
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn callback() -> String {
    format!("geetest_{}", now_millis())
}

fn unwrap_jsonp(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    let body = match (trimmed.find('('), trimmed.rfind(')')) {
        (Some(start), Some(end)) if end > start => &trimmed[start + 1..end],
        _ => trimmed,
    };
    serde_json::from_str(body).map_err(parse_error)
}

pub struct V4Client {
    client: Client,
    fallback: Client,
    has_proxy: bool,
}

impl V4Client {
    pub fn new(proxy: Option<&str>) -> Result<Self> {
        let fallback = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| other("build direct GeeTest client failed", e))?;

        let has_proxy = proxy.is_some_and(|value| !value.trim().is_empty());
        let client = match proxy.filter(|value| !value.trim().is_empty()) {
            Some(proxy_url) => {
                let proxy = reqwest::Proxy::all(proxy_url)
                    .map_err(|e| other("invalid GeeTest proxy URL", e))?;
                Client::builder()
                    .user_agent(USER_AGENT)
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .timeout(std::time::Duration::from_secs(30))
                    .pool_idle_timeout(std::time::Duration::from_secs(90))
                    .tcp_keepalive(std::time::Duration::from_secs(60))
                    .proxy(proxy)
                    .build()
                    .map_err(|e| other("build proxied GeeTest client failed", e))?
            }
            None => fallback.clone(),
        };
        Ok(Self {
            client,
            fallback,
            has_proxy,
        })
    }

    fn get_json(&self, client: &Client, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let response = client
            .get(format!("{BASE_URL}{path}"))
            .header("Accept", "*/*")
            .header(
                "Accept-Language",
                "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6",
            )
            .header("Sec-Fetch-Site", "cross-site")
            .header("Sec-Fetch-Mode", "no-cors")
            .header("Sec-Fetch-Dest", "script")
            .query(query)
            .send()
            .map_err(|e| other("GeeTest request failed", e))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|e| other("read GeeTest response failed", e))?;
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(other_without_source("GeeTest rate limited (HTTP 429)"));
        }
        if !status.is_success() {
            return Err(other_without_source(&format!(
                "GeeTest HTTP {}: {}",
                status,
                body.chars().take(240).collect::<String>()
            )));
        }
        let value = unwrap_jsonp(&body)?;
        if value.get("status").and_then(Value::as_str) == Some("error") {
            return Err(other_without_source(&format!(
                "GeeTest API error: {}",
                value
            )));
        }
        Ok(value)
    }

    fn call_with_fallback(
        &self,
        path: &str,
        query: &[(&str, String)],
        allow_fallback: bool,
    ) -> Result<Value> {
        match self.get_json(&self.client, path, query) {
            Ok(value) => Ok(value),
            Err(proxy_error) if allow_fallback && self.has_proxy => {
                tracing::warn!(error = %proxy_error, "GeeTest proxy request failed; retrying direct");
                self.get_json(&self.fallback, path, query)
            }
            Err(error) => Err(error),
        }
    }

    pub fn load(
        &self,
        captcha_id: &str,
        challenge: &str,
        risk_type: &str,
        allow_fallback: bool,
    ) -> Result<LoadData> {
        let effective_challenge = if challenge.trim().is_empty() {
            Uuid::new_v4().to_string()
        } else {
            challenge.to_string()
        };
        let query = vec![
            ("callback", callback()),
            ("captcha_id", captcha_id.to_string()),
            ("challenge", effective_challenge),
            ("client_type", "web".to_string()),
            ("risk_type", risk_type.to_string()),
            ("lang", "zh".to_string()),
        ];
        let value = self.call_with_fallback("/load", &query, allow_fallback)?;
        let mut data: LoadData = serde_json::from_value(
            value
                .get("data")
                .cloned()
                .ok_or_else(|| other_without_source("GeeTest load response missing data"))?,
        )
        .map_err(parse_error)?;
        if data.captcha_id.is_empty() {
            data.captcha_id = captcha_id.to_string();
        }
        Ok(data)
    }

    pub fn load_resource(&self, path: &str, allow_fallback: bool) -> Result<Vec<u8>> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{IMG_BASE}/{}", path.trim_start_matches('/'))
        };
        let fetch = |client: &Client| -> Result<Vec<u8>> {
            let response = client
                .get(&url)
                .send()
                .map_err(|e| other("load GeeTest resource failed", e))?;
            if !response.status().is_success() {
                return Err(other_without_source(&format!(
                    "GeeTest resource HTTP {}",
                    response.status()
                )));
            }
            response
                .bytes()
                .map(|bytes| bytes.to_vec())
                .map_err(|e| other("read GeeTest resource failed", e))
        };
        match fetch(&self.client) {
            Ok(bytes) => Ok(bytes),
            Err(proxy_error) if allow_fallback && self.has_proxy => {
                tracing::warn!(error = %proxy_error, url = %url, "GeeTest resource proxy failed; retrying direct");
                fetch(&self.fallback)
            }
            Err(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn verify(
        &self,
        captcha_id: &str,
        risk_type: &str,
        data: &LoadData,
        clicks: &[(f64, f64)],
        allow_fallback: bool,
    ) -> Result<VerifyData> {
        let track = build_click_track(clicks)?;
        let track_json = serde_json::to_string(&track).map_err(parse_error)?;
        let td = track_zip(&track_json)?;
        let td_sign = track_sign(&data.lot_number, &td)?;
        let passtime = track.e.saturating_sub(track.s) as u32;
        let w = build_w(data, clicks, passtime, &td_sign)?;

        let query = vec![
            ("callback", callback()),
            ("captcha_id", captcha_id.to_string()),
            ("client_type", "web".to_string()),
            ("risk_type", risk_type.to_string()),
            ("lot_number", data.lot_number.clone()),
            ("payload", data.payload.clone()),
            ("process_token", data.process_token.clone()),
            ("payload_protocol", data.payload_protocol.to_string()),
            ("pt", data.pt.clone()),
            ("w", w),
            ("td", td),
        ];
        let value = self.call_with_fallback("/verify", &query, allow_fallback)?;
        let verify_data = value
            .get("data")
            .cloned()
            .ok_or_else(|| other_without_source("GeeTest verify response missing data"))?;
        serde_json::from_value(verify_data).map_err(parse_error)
    }
}

fn parse_abo_group(group: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let bytes = group.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if &bytes[index..index + 2] == b"n[" {
            let start = index + 2;
            if let Some(relative_end) = group[start..].find(']') {
                let end = start + relative_end;
                let inner = &group[start..end];
                let mut parts = inner.split(':');
                if let Some(first) = parts.next().and_then(|v| v.parse::<usize>().ok()) {
                    let last = parts
                        .next()
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(first);
                    ranges.push((first, last));
                }
                index = end + 1;
                continue;
            }
        }
        index += 1;
    }
    ranges
}

fn extract_indexes(pattern: &str, value: &str) -> String {
    pattern
        .split("+.+")
        .map(|group| {
            parse_abo_group(group)
                .into_iter()
                .filter_map(|(start, end)| {
                    value
                        .get(start..=end.min(value.len().saturating_sub(1)))
                        .map(str::to_string)
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn insert_abo(map: &mut Map<String, Value>, key: &str, value: &str) {
    let parts = key
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return;
    }
    let mut current = map;
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry.as_object_mut().expect("object inserted above");
    }
    current.insert(
        parts[parts.len() - 1].to_string(),
        Value::String(value.to_string()),
    );
}

fn generate_pow(captcha_id: &str, data: &LoadData) -> Result<(String, String)> {
    let detail = &data.pow_detail;
    let prefix_len = (detail.bits / 4) as usize;
    let remainder = detail.bits % 4;
    let threshold = match remainder {
        0 => b'f',
        1 => b'7',
        2 => b'3',
        3 => b'1',
        _ => unreachable!(),
    };
    let header = format!(
        "{}|{}|{}|{}|{}|{}||",
        detail.version, detail.bits, detail.hashfunc, detail.datetime, captcha_id, data.lot_number
    );
    for _ in 0..1_000_000 {
        let message = format!("{header}{}", Uuid::new_v4().simple());
        let digest = match detail.hashfunc.as_str() {
            "md5" => format!("{:x}", md5::compute(message.as_bytes())),
            "sha256" => {
                use sha2::Digest;
                format!("{:x}", Sha256::digest(message.as_bytes()))
            }
            other => {
                return Err(other_without_source(&format!(
                    "unsupported GeeTest PoW hash {other}"
                )))
            }
        };
        if digest.as_bytes()[..prefix_len]
            .iter()
            .all(|byte| *byte == b'0')
            && digest.as_bytes()[prefix_len] <= threshold
        {
            return Ok((message, digest));
        }
    }
    Err(other_without_source("GeeTest PoW did not converge"))
}

fn build_w(data: &LoadData, clicks: &[(f64, f64)], passtime: u32, td_sign: &str) -> Result<String> {
    let patrol = Patrol::from_static_path(&data.static_path)?;
    let (pow_msg, pow_sign) = generate_pow(&data.captcha_id, data)?;
    let mut payload = Map::new();
    payload.insert("pow_msg".to_string(), Value::String(pow_msg));
    payload.insert("pow_sign".to_string(), Value::String(pow_sign));

    let abo_key = extract_indexes(patrol.abo_key, &data.lot_number);
    let abo_val = extract_indexes(patrol.abo_val, &data.lot_number);
    insert_abo(&mut payload, &abo_key, &abo_val);

    payload.insert(
        patrol.lib_key.to_string(),
        Value::String(patrol.lib_val.to_string()),
    );
    payload.insert("biht".to_string(), Value::String(patrol.biht.to_string()));
    payload.insert("device_id".to_string(), Value::String(String::new()));
    payload.insert(
        "em".to_string(),
        json!({"cp":0,"ek":"11","nt":0,"ph":0,"sc":0,"si":0,"wd":1}),
    );
    payload.insert("ep".to_string(), Value::String("123".to_string()));
    payload.insert("geetest".to_string(), Value::String("captcha".to_string()));
    payload.insert("lang".to_string(), Value::String("zh".to_string()));
    payload.insert(
        "lot_number".to_string(),
        Value::String(data.lot_number.clone()),
    );
    let userresponse = clicks
        .iter()
        .map(|(x, y)| json!([(x * 10_000.0).round() as i64, (y * 10_000.0).round() as i64]))
        .collect::<Vec<_>>();
    payload.insert("userresponse".to_string(), Value::Array(userresponse));
    payload.insert("passtime".to_string(), json!(passtime));
    payload.insert("td_sign".to_string(), Value::String(td_sign.to_string()));

    if data.guard {
        payload.insert(
            "roe".to_string(),
            json!({"aup":"3","sep":"3","egp":"3","auh":"3","rew":"3","snh":"3","res":"3","cdc":"3"}),
        );
    }

    let plaintext = serde_json::to_string(&payload).map_err(parse_error)?;
    match data.pt.as_str() {
        "0" => Ok(URL_SAFE.encode(plaintext.as_bytes())),
        "1" => encrypt_pt1(&plaintext),
        "2" => Err(other_without_source(
            "GeeTest payload_protocol 2 (SM4/SM2) is not enabled in this CPU build",
        )),
        other => Err(other_without_source(&format!(
            "unsupported GeeTest payload protocol {other}"
        ))),
    }
}

fn encrypt_pt1(plaintext: &str) -> Result<String> {
    let mut key = [0u8; 16];
    thread_rng().fill(&mut key);
    let cipher = cbc::Encryptor::<Aes128>::new_from_slices(&key, &AES_IV)
        .map_err(|e| other("initialize GeeTest AES cipher failed", e))?;
    let message = plaintext.as_bytes();
    let mut buffer = vec![0u8; message.len() + 16];
    buffer[..message.len()].copy_from_slice(message);
    let encrypted = cipher
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, message.len())
        .map_err(|_| other_without_source("GeeTest AES encryption failed"))?;
    let public_key = rsa_public_key()?;
    let encrypted_key = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, &key)
        .map_err(|e| other("GeeTest RSA key encryption failed", e))?;
    Ok(format!(
        "{}{}",
        hex::encode(encrypted),
        hex::encode(encrypted_key)
    ))
}

fn rsa_public_key() -> Result<RsaPublicKey> {
    let modulus = hex::decode(RSA_N).map_err(|e| other("decode GeeTest RSA modulus failed", e))?;
    let exponent =
        hex::decode(RSA_E).map_err(|e| other("decode GeeTest RSA exponent failed", e))?;
    RsaPublicKey::new(
        BigUint::from_bytes_be(&modulus),
        BigUint::from_bytes_be(&exponent),
    )
    .map_err(|e| other("build GeeTest RSA public key failed", e))
}

#[derive(Debug, Clone, Serialize)]
struct TrackPayload {
    m: u8,
    w: f64,
    h: f64,
    s: u64,
    e: u64,
    p: Vec<(u64, f64, f64, u8)>,
}

#[derive(Debug, Clone)]
struct TrackBuilder {
    x: f64,
    y: f64,
    t: u64,
    events: Vec<(u64, f64, f64, u8)>,
}

impl TrackBuilder {
    fn new(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            t: 0,
            events: vec![(0, x, y, 0)],
        }
    }

    fn move_to(&mut self, x: f64, y: f64, duration: u64) {
        let start_x = self.x;
        let start_y = self.y;
        let steps = thread_rng().gen_range(5..=11);
        for step in 1..=steps {
            let linear = step as f64 / steps as f64;
            let eased = 0.35 * linear + 0.65 * (1.0 - (1.0 - linear).powi(2));
            let wobble = if step == steps {
                0.0
            } else {
                thread_rng().gen_range(-0.003..=0.003)
            };
            let px = start_x + (x - start_x) * eased + wobble;
            let py = start_y + (y - start_y) * eased - wobble * 0.5;
            self.t += ((duration as f64) / steps as f64).round() as u64;
            self.events.push((self.t, px, py, 1));
        }
        self.x = x;
        self.y = y;
    }

    fn click(&mut self, delay: u64) {
        self.events.push((self.t, self.x, self.y, 3));
        self.t += delay;
        self.events.push((self.t, self.x, self.y, 2));
    }
}

fn jitter_inside(x: f64, y: f64, amount: f64) -> (f64, f64) {
    let mut rng = thread_rng();
    (
        (x + rng.gen_range(-amount..=amount)).clamp(0.0, 1.0),
        (y + rng.gen_range(-amount..=amount)).clamp(0.0, 1.0),
    )
}

fn build_click_track(clicks: &[(f64, f64)]) -> Result<TrackPayload> {
    if clicks.is_empty() {
        return Err(other_without_source(
            "click track requires at least one click",
        ));
    }
    let mut rng = thread_rng();
    let mut targets = clicks
        .iter()
        .map(|(x, y)| {
            let ex = x * 300.0 / 300.015625;
            let ey = y * 200.0 / 259.6015625;
            jitter_inside(ex, ey, 0.02)
        })
        .collect::<Vec<_>>();
    targets.push(jitter_inside(0.5, 0.92, 0.01));

    let release_delays = (0..targets.len())
        .map(|_| rng.gen_range(80..=120))
        .collect::<Vec<_>>();
    let total: u64 = rng.gen_range(2500..=4000);
    let move_total = total.saturating_sub(release_delays.iter().sum::<u64>());
    let weights = (0..targets.len())
        .map(|_| rng.gen::<f64>() + 0.1)
        .collect::<Vec<_>>();
    let sum = weights.iter().sum::<f64>();
    let mut moves = weights
        .iter()
        .map(|weight| ((move_total as f64) * weight / sum).round() as u64)
        .collect::<Vec<_>>();
    let allocated = moves.iter().sum::<u64>();
    if let Some(last) = moves.last_mut() {
        *last = last
            .saturating_add(move_total.saturating_sub(allocated))
            .max(1);
    }

    let start = (rng.gen_range(0.4..=0.6), rng.gen_range(0.9..=1.0));
    let mut builder = TrackBuilder::new(start.0, start.1);
    for ((target, delay), duration) in targets.into_iter().zip(release_delays).zip(moves) {
        builder.move_to(target.0, target.1, duration);
        builder.click(delay);
    }
    let end = builder.t;
    let started = now_millis();
    Ok(TrackPayload {
        m: 1,
        w: 300.015625,
        h: 259.6015625,
        s: started,
        e: started + end,
        p: builder.events,
    })
}

fn track_zip(track_json: &str) -> Result<String> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(track_json.as_bytes())
        .map_err(|e| other("compress GeeTest track failed", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| other("finish GeeTest track compression failed", e))?;
    Ok(URL_SAFE_NO_PAD.encode(compressed))
}

use base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn track_sign(lot_number: &str, track: &str) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(lot_number.as_bytes())
        .map_err(|e| other("initialize GeeTest track HMAC failed", e))?;
    mac.update(track.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abo_indexes_follow_lot_number() {
        let lot = "0123456789abcdefghijklmnopqrstuv";
        assert_eq!(extract_indexes("n[26:28]+n[24:26]", lot), "qrsopq");
        assert_eq!(extract_indexes("n[1:8]", lot), "12345678");
    }

    #[test]
    fn click_track_has_pointer_protocol_shape() {
        let track = build_click_track(&[(0.2, 0.3), (0.5, 0.4), (0.7, 0.6)]).unwrap();
        assert_eq!(track.m, 1);
        assert!(track.p.len() > 20);
        assert_eq!(track.p.first().unwrap().3, 0);
        assert_eq!(track.p.last().unwrap().3, 2);
    }
}
