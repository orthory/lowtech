// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 orthory

//! HID++ transport over hidapi — framing, request/reply, ping, feature discovery.
//!
//! Mirrors Solaar's `logitech_receiver/base.py`. A Logitech receiver (or a
//! directly-connected device) exposes a vendor HID interface (usage page
//! 0xFF00) that carries the HID++ protocol. Messages are HID reports:
//!
//!   short (report id 0x10): [0x10, devnumber, sub_id/feat_idx, addr/func|sw, p0, p1, p2]   (7 bytes)
//!   long  (report id 0x11): [0x11, devnumber, sub_id/feat_idx, addr/func|sw, ...16 bytes]  (20 bytes)
//!
//! `devnumber` is 0xFF for a directly-connected device, or 1..=6 for a device
//! paired to a receiver slot.

use hidapi::{HidApi, HidDevice};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const VENDOR_LOGITECH: u16 = 0x046D;
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;

const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
const SHORT_LEN: usize = 7;
const LONG_LEN: usize = 20;
const MAX_READ: usize = 32;

// HID++ Software ID claimed by this tool (fixed, like Solaar's 0x0B). The low
// nibble of the request id; replies echo it, notifications use 0 — which lets us
// tell our replies apart from unsolicited events.
const SW_ID: u16 = 0x0B;

// Well-known feature ids we use.
pub const FEAT_ROOT: u16 = 0x0000;
pub const FEAT_FEATURE_SET: u16 = 0x0001;
pub const FEAT_DEVICE_NAME: u16 = 0x0005;
pub const FEAT_UNIFIED_BATTERY: u16 = 0x1004;
pub const FEAT_EXTENDED_DPI: u16 = 0x2202;
pub const FEAT_ADJUSTABLE_DPI: u16 = 0x2201;
pub const FEAT_ONBOARD_PROFILES: u16 = 0x8100;

#[derive(Debug)]
pub enum Error {
    Hid(hidapi::HidError),
    /// HID++ 1.0 error reply (sub_id 0x8F): error code.
    Hidpp10(u8),
    /// HID++ 2.0 feature-call error (0xFF): error code.
    Hidpp20(u8),
    Timeout,
    NotSupported(&'static str),
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Hid(e) => write!(f, "hid error: {e}"),
            Error::Hidpp10(c) => write!(f, "HID++1.0 error 0x{c:02X}"),
            Error::Hidpp20(c) => write!(f, "HID++2.0 feature error 0x{c:02X} ({})", hidpp20_err(*c)),
            Error::Timeout => write!(f, "timeout waiting for reply"),
            Error::NotSupported(s) => write!(f, "feature not supported: {s}"),
            Error::Protocol(s) => write!(f, "protocol error: {s}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Error::Hid(e)
    }
}

fn hidpp20_err(c: u8) -> &'static str {
    match c {
        0 => "no error",
        1 => "unknown",
        2 => "invalid argument",
        3 => "out of range",
        4 => "hw error",
        5 => "logitech internal",
        6 => "invalid feature index",
        7 => "invalid function id",
        8 => "busy",
        9 => "unsupported",
        _ => "?",
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One reachable HID++ device, bound to an open handle + a device number.
pub struct Device {
    dev: HidDevice,
    pub devnumber: u8,
    /// Use long (0x11) frames for requests (BT / direct devices that lack short).
    pub prefer_long: bool,
    pub protocol: f32,
    /// feature id -> (index, version)
    pub features: HashMap<u16, (u8, u8)>,
    /// preserves discovery order for display
    pub feature_order: Vec<u16>,
}

impl Device {
    /// Open a vendor HID path and probe `devnumber`. Returns `None` if nothing
    /// answers there (empty receiver slot / no device).
    pub fn probe(api: &HidApi, path: &std::ffi::CStr, devnumber: u8) -> Result<Option<Device>> {
        let dev = api.open_path(path)?;
        // Try short frames first (receivers), then long (direct/BT devices).
        for prefer_long in [false, true] {
            if let Some(proto) = ping(&dev, devnumber, prefer_long)? {
                let mut d = Device {
                    dev,
                    devnumber,
                    prefer_long,
                    protocol: proto,
                    features: HashMap::new(),
                    feature_order: Vec::new(),
                };
                if proto >= 2.0 {
                    d.discover_features()?;
                }
                return Ok(Some(d));
            }
        }
        Ok(None)
    }

    pub fn feature_index(&self, feature: u16) -> Option<u8> {
        self.features.get(&feature).map(|(i, _)| *i)
    }

    pub fn has(&self, feature: u16) -> bool {
        self.features.contains_key(&feature)
    }

    /// A feature call: `function` is the high nibble byte (0x00, 0x10, 0x20 ...).
    pub fn call(&self, feature: u16, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        let idx = self
            .feature_index(feature)
            .ok_or(Error::NotSupported("feature not present on device"))?;
        self.call_idx(idx, function, params)
    }

    /// A feature call addressed by raw feature index.
    pub fn call_idx(&self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        let req_id: u16 = (((feature_index as u16) << 8) | (function as u16) & 0x00F0) | SW_ID;
        request(&self.dev, self.devnumber, req_id, params, self.prefer_long)
    }

    /// ROOT.getFeature — index of a feature id, 0 if absent.
    fn root_get_feature(&self, feature: u16) -> Result<u8> {
        let r = request(
            &self.dev,
            self.devnumber,
            FEAT_ROOT | SW_ID,
            &feature.to_be_bytes(),
            self.prefer_long,
        )?;
        Ok(r.first().copied().unwrap_or(0))
    }

    fn discover_features(&mut self) -> Result<()> {
        let fs_idx = self.root_get_feature(FEAT_FEATURE_SET)?;
        if fs_idx == 0 {
            return Err(Error::Protocol("no FEATURE_SET".into()));
        }
        // FEATURE_SET.getCount (function 0x00)
        let count = request(&self.dev, self.devnumber, ((fs_idx as u16) << 8) | SW_ID, &[], self.prefer_long)?;
        let n = count.first().copied().unwrap_or(0);
        // ROOT is index 0 and not included in the count.
        self.features.insert(FEAT_ROOT, (0, 0));
        self.feature_order.push(FEAT_ROOT);
        for i in 0..=n {
            // FEATURE_SET.getFeatureId (function 0x10)
            let req_id = (((fs_idx as u16) << 8) | 0x10) | SW_ID;
            match request(&self.dev, self.devnumber, req_id, &[i], self.prefer_long) {
                Ok(fd) if fd.len() >= 4 => {
                    let fid = u16::from_be_bytes([fd[0], fd[1]]);
                    let ver = fd[3];
                    if !self.features.contains_key(&fid) {
                        self.features.insert(fid, (i, ver));
                        self.feature_order.push(fid);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    // ---- convenience feature wrappers -------------------------------------

    /// DEVICE_NAME 0x0005: full UTF-8/ASCII device name.
    pub fn name(&self) -> Result<String> {
        let count = self.call(FEAT_DEVICE_NAME, 0x00, &[])?;
        let total = *count.first().ok_or(Error::Protocol("name count".into()))? as usize;
        let mut bytes = Vec::with_capacity(total);
        while bytes.len() < total {
            let chunk = self.call(FEAT_DEVICE_NAME, 0x10, &[bytes.len() as u8])?;
            if chunk.is_empty() {
                break;
            }
            let take = (total - bytes.len()).min(chunk.len());
            bytes.extend_from_slice(&chunk[..take]);
        }
        Ok(String::from_utf8_lossy(&bytes).trim_end_matches('\0').to_string())
    }

    /// DEVICE_NAME 0x0005 getDeviceType (function 0x20).
    pub fn kind(&self) -> Result<u8> {
        Ok(self.call(FEAT_DEVICE_NAME, 0x20, &[])?.first().copied().unwrap_or(0xFF))
    }

    /// UNIFIED_BATTERY 0x1004 getStatus (function 0x10) -> (charge%, status byte).
    pub fn battery(&self) -> Result<(Option<u8>, u8)> {
        let r = self.call(FEAT_UNIFIED_BATTERY, 0x10, &[])?;
        if r.len() < 3 {
            return Err(Error::Protocol("battery reply too short".into()));
        }
        let charge = if r[0] == 0 { None } else { Some(r[0]) };
        Ok((charge, r[2]))
    }
}

/// Build and send one HID++ request, wait for the matching reply.
/// `req_id` already encodes (feature_index << 8) | function | sw_id.
fn request(dev: &HidDevice, devnumber: u8, req_id: u16, params: &[u8], prefer_long: bool) -> Result<Vec<u8>> {
    let mut req_data = Vec::with_capacity(2 + params.len());
    req_data.extend_from_slice(&req_id.to_be_bytes());
    req_data.extend_from_slice(params);

    let long = prefer_long || req_data.len() > 5;
    let frame = if long {
        let mut f = vec![0u8; LONG_LEN];
        f[0] = REPORT_LONG;
        f[1] = devnumber;
        let n = req_data.len().min(LONG_LEN - 2);
        f[2..2 + n].copy_from_slice(&req_data[..n]);
        f
    } else {
        let mut f = vec![0u8; SHORT_LEN];
        f[0] = REPORT_SHORT;
        f[1] = devnumber;
        let n = req_data.len().min(SHORT_LEN - 2);
        f[2..2 + n].copy_from_slice(&req_data[..n]);
        f
    };
    dev.write(&frame)?;

    let want = req_id.to_be_bytes();
    let timeout = Duration::from_millis(if devnumber == 0xFF { 900 } else { 3000 });
    let start = Instant::now();
    let mut buf = [0u8; MAX_READ];
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        let nbytes = dev.read_timeout(&mut buf, remaining.as_millis() as i32)?;
        if nbytes < 4 {
            continue;
        }
        let report_id = buf[0];
        let rdn = buf[1];
        if rdn != devnumber && rdn != (devnumber ^ 0xFF) {
            continue;
        }
        let payload = &buf[2..nbytes];
        // HID++ 1.0 error: short report, sub_id 0x8F, echoes req header.
        if report_id == REPORT_SHORT && payload.len() >= 4 && payload[0] == 0x8F && payload[1..3] == want {
            return Err(Error::Hidpp10(payload[3]));
        }
        // HID++ 2.0 feature error: 0xFF, echoes req header.
        if payload.len() >= 4 && payload[0] == 0xFF && payload[1..3] == want {
            return Err(Error::Hidpp20(payload[3]));
        }
        // Success: reply echoes the (feature_idx, function|sw) header.
        if payload.len() >= 2 && payload[0..2] == want {
            return Ok(payload[2..].to_vec());
        }
        // else: a notification or an unrelated reply — keep waiting.
    }
    Err(Error::Timeout)
}

/// Ping a device number. Returns the HID++ protocol version, or None if no device.
pub fn ping(dev: &HidDevice, devnumber: u8, prefer_long: bool) -> Result<Option<f32>> {
    let req_id = FEAT_ROOT | 0x0010 | SW_ID; // ROOT.ping (function 0x10)
    let mark: u8 = 0x5A;
    let params = [0u8, 0u8, mark];

    let mut req_data = vec![(req_id >> 8) as u8, (req_id & 0xFF) as u8];
    req_data.extend_from_slice(&params);
    let frame = if prefer_long {
        let mut f = vec![0u8; LONG_LEN];
        f[0] = REPORT_LONG;
        f[1] = devnumber;
        f[2..2 + req_data.len()].copy_from_slice(&req_data);
        f
    } else {
        let mut f = vec![0u8; SHORT_LEN];
        f[0] = REPORT_SHORT;
        f[1] = devnumber;
        f[2..2 + req_data.len()].copy_from_slice(&req_data);
        f
    };
    dev.write(&frame)?;

    let want = req_id.to_be_bytes();
    let timeout = Duration::from_millis(2000);
    let start = Instant::now();
    let mut buf = [0u8; MAX_READ];
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        let nbytes = dev.read_timeout(&mut buf, remaining.as_millis() as i32)?;
        if nbytes < 4 {
            continue;
        }
        let report_id = buf[0];
        let rdn = buf[1];
        if rdn != devnumber && rdn != (devnumber ^ 0xFF) {
            continue;
        }
        let payload = &buf[2..nbytes];
        // Successful ping echoes header + version + our mark byte.
        if payload.len() >= 5 && payload[0..2] == want && payload[4] == mark {
            let version = payload[2] as f32 + payload[3] as f32 / 10.0;
            return Ok(Some(version));
        }
        // HID++ 1.0 device or error.
        if report_id == REPORT_SHORT && payload.len() >= 4 && payload[0] == 0x8F && payload[1..3] == want {
            match payload[3] {
                0x01 => return Ok(Some(1.0)),       // invalid sub-id => HID++ 1.0 device
                0x08 | 0x09 => return Ok(None),     // resource / connection failed => unreachable
                0x03 => return Ok(None),            // unknown device => empty slot
                _ => return Ok(None),
            }
        }
    }
    Ok(None)
}
