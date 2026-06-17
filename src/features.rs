// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 orthory

//! Human-readable names + a few higher-level feature wrappers (DPI).

use crate::hidpp::{Device, Error, Result, FEAT_ADJUSTABLE_DPI, FEAT_EXTENDED_DPI};

/// HID++ 2.0 feature id -> short name (subset of Solaar's SupportedFeature).
pub fn feature_name(id: u16) -> &'static str {
    match id {
        0x0000 => "ROOT",
        0x0001 => "FEATURE_SET",
        0x0002 => "FEATURE_INFO",
        0x0003 => "DEVICE_FW_VERSION",
        0x0004 => "DEVICE_UNIT_ID",
        0x0005 => "DEVICE_NAME",
        0x0006 => "DEVICE_GROUPS",
        0x0007 => "DEVICE_FRIENDLY_NAME",
        0x0008 => "KEEP_ALIVE",
        0x0011 => "PROPERTY_ACCESS",
        0x0020 => "CONFIG_CHANGE",
        0x0021 => "CRYPTO_ID",
        0x0080 => "WIRELESS_SIGNAL_STRENGTH",
        0x00C2 => "DFUCONTROL_SIGNED",
        0x00C3 => "DFUCONTROL",
        0x1000 => "BATTERY_STATUS",
        0x1001 => "BATTERY_VOLTAGE",
        0x1004 => "UNIFIED_BATTERY",
        0x1010 => "CHARGING_CONTROL",
        0x1300 => "LED_CONTROL",
        0x1500 => "FORCE_PAIRING",
        0x1801 => "MANUFACTURING_MODE",
        0x1802 => "DEVICE_RESET",
        0x1803 => "GPIO_ACCESS",
        0x1805 => "OOBSTATE",
        0x1806 => "CONFIG_DEVICE_PROPS",
        0x1814 => "CHANGE_HOST",
        0x1815 => "HOSTS_INFO",
        0x1817 => "LIGHTSPEED_PREPAIRING",
        0x1830 => "POWER_MODES",
        0x1861 => "BATTERY_VOLTAGE",
        0x1875 => "ANALYTICS_EVENTS",
        0x1890 => "REPORT_RATE_PRIVATE",
        0x18A1 => "LED_TEST",
        0x18B1 => "MLX903xx_MONITORING",
        0x1981 => "BACKLIGHT",
        0x1982 => "BACKLIGHT2",
        0x1983 => "BACKLIGHT3",
        0x1B00 => "REPROG_CONTROLS",
        0x1B04 => "REPROG_CONTROLS_V4",
        0x1B05 => "FULL_KEY_CUSTOMIZATION",
        0x1B10 => "CONTROL_LIST",
        0x1BC0 => "REPORT_HID_USAGE",
        0x1C00 => "PERSISTENT_REMAPPABLE_ACTION",
        0x1D4B => "WIRELESS_DEVICE_STATUS",
        0x1DF3 => "ENABLE_HIDDEN_FEATURES",
        0x1E00 => "ENABLE_HIDDEN_FEATURES",
        0x1E02 => "MANAGE_DEACTIVATABLE_FEATURES",
        0x1E22 => "SPI_DIRECT_ACCESS",
        0x1EB0 => "TDE_ACCESS",
        0x1F03 => "FIRMWARE_PROPERTIES",
        0x2001 => "LEFT_RIGHT_SWAP",
        0x2100 => "VERTICAL_SCROLLING",
        0x2110 => "SMART_SHIFT",
        0x2111 => "SMART_SHIFT_ENHANCED",
        0x2121 => "HIRES_WHEEL",
        0x2130 => "LOWRES_WHEEL",
        0x2150 => "THUMB_WHEEL",
        0x2200 => "MOUSE_POINTER",
        0x2201 => "ADJUSTABLE_DPI",
        0x2202 => "EXTENDED_ADJUSTABLE_DPI",
        0x2205 => "POINTER_SPEED",
        0x2230 => "ANGLE_SNAPPING",
        0x2240 => "SURFACE_TUNING",
        0x2250 => "XY_STATS",
        0x2251 => "WHEEL_STATS",
        0x8010 => "GKEY",
        0x8020 => "MKEYS",
        0x8030 => "MR",
        0x8040 => "BRIGHTNESS_CONTROL",
        0x8060 => "REPORT_RATE",
        0x8061 => "EXTENDED_ADJUSTABLE_REPORT_RATE",
        0x8070 => "COLOR_LED_EFFECTS",
        0x8071 => "RGB_EFFECTS",
        0x8090 => "MODE_STATUS",
        0x80E0 => "BUNNY_HOPPING",
        0x8100 => "ONBOARD_PROFILES",
        0x8110 => "MOUSE_BUTTON_SPY",
        0x8111 => "LATENCY_MONITORING",
        0x1602 => "PASSWORD",
        _ => "",
    }
}

pub fn device_type_name(t: u8) -> &'static str {
    match t {
        0 => "Keyboard",
        1 => "Remote Control",
        2 => "Numpad",
        3 => "Mouse",
        4 => "Touchpad",
        5 => "Trackball",
        6 => "Presenter",
        7 => "Receiver",
        8 => "Headset",
        9 => "Webcam",
        10 => "Steering Wheel",
        11 => "Joystick",
        12 => "Gamepad",
        _ => "?",
    }
}

pub fn battery_status_name(s: u8) -> &'static str {
    match s {
        0 => "discharging",
        1 => "charging",
        2 => "almost full",
        3 => "full",
        4 => "slow recharge",
        5 => "invalid battery",
        6 => "thermal error",
        _ => "?",
    }
}

/// Resolved DPI state for a sensor.
pub struct Dpi {
    pub x: u16,
    pub y: u16,
    pub has_y: bool,
    pub has_lod: bool,
    pub lod: u8,
}

/// Read DPI via EXTENDED_ADJUSTABLE_DPI (0x2202).
pub fn dpi_read(d: &Device) -> Result<Dpi> {
    if !d.has(FEAT_EXTENDED_DPI) {
        if d.has(FEAT_ADJUSTABLE_DPI) {
            return Err(Error::NotSupported("device uses ADJUSTABLE_DPI (0x2201), not yet implemented"));
        }
        return Err(Error::NotSupported("no adjustable-DPI feature"));
    }
    // getInfo (fn 0x10) for sensor 0: which axes/LOD are configurable.
    let info = d.call(FEAT_EXTENDED_DPI, 0x10, &[0x00])?;
    let has_y = info.get(2).map(|b| b & 0x01 != 0).unwrap_or(false);
    let has_lod = info.get(2).map(|b| b & 0x02 != 0).unwrap_or(false);

    // getSensorDpi (fn 0x50) for sensor 0.
    let r = d.call(FEAT_EXTENDED_DPI, 0x50, &[0x00])?;
    if r.len() < 5 {
        return Err(Error::Protocol("dpi reply too short".into()));
    }
    // current X at [1..3]; if zero, fall back to default at [3..5].
    let x = be16(&r[1..3]);
    let x = if x == 0 { be16(&r[3..5]) } else { x };
    let y = if has_y && r.len() >= 7 {
        let y = be16(&r[5..7]);
        if y == 0 && r.len() >= 9 { be16(&r[7..9]) } else { y }
    } else {
        x
    };
    let lod = if has_lod { r.get(9).copied().unwrap_or(0) } else { 0 };
    Ok(Dpi { x, y, has_y, has_lod, lod })
}

/// Set DPI (both axes) via EXTENDED_ADJUSTABLE_DPI (0x2202), preserving LOD.
pub fn dpi_set(d: &Device, value: u16) -> Result<Dpi> {
    let cur = dpi_read(d)?;
    let y = if cur.has_y { value } else { 0 };
    // setSensorDpi (fn 0x60): [sensor, x_hi, x_lo, y_hi, y_lo, lod]
    let params = [
        0x00,
        (value >> 8) as u8,
        (value & 0xFF) as u8,
        (y >> 8) as u8,
        (y & 0xFF) as u8,
        cur.lod,
    ];
    d.call(FEAT_EXTENDED_DPI, 0x60, &params)?;
    dpi_read(d)
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
