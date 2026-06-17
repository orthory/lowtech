// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 orthory

//! Onboard-profile (0x8100) button remapping.
//!
//! The PRO X SUPERLIGHT 2 (and most Logitech G mice) have no 0x1B04
//! Reprogrammable Controls; buttons live in the mouse's onboard profile memory.
//! Solaar only supports profileFormat <= 5; this mouse reports format 7, whose
//! profile sector has a 16-byte-larger header (button array at offset 48 instead
//! of 32). We located that empirically by dumping a sector and matching the
//! default button masks, then validating the CRC-16/CCITT-FALSE checksum.
//!
//! Profile sector layout (format 7, 255-byte sector) — relevant fields:
//!   [ 0]      report-rate index
//!   [ 4..29]  5 DPI stages: x(u16le) y(u16le) flag(1)  -> 800/1200/1600/2400/3200
//!   [48..]    button specs: N x 4 bytes (see `Button`)
//!   [253..255] CRC-16/CCITT-FALSE over [0..253]  (big-endian)
//!
//! Write protocol (per Solaar's write_sector):
//!   fn 0x60 startWrite(sector, offset=0, len)
//!   fn 0x70 writeData(<=16 bytes)  (repeated)
//!   fn 0x80 endWrite (commit)
//!
//! Safety: every write reads the sector back first and CRC-validates it, saves an
//! on-disk backup of the original bytes, then verifies the readback after writing.

use crate::hidpp::{Device, Error, Result, FEAT_ONBOARD_PROFILES};
use crate::{new_api, select_device};

const FN_GET_INFO: u8 = 0x00;
const FN_READ_SECTOR: u8 = 0x50;
const FN_START_WRITE: u8 = 0x60;
const FN_WRITE_DATA: u8 = 0x70;
const FN_END_WRITE: u8 = 0x80;

/// Button spec lives here in format-7 sectors; older formats use 32.
fn button_offset(profile_format: u8) -> usize {
    if profile_format >= 6 {
        48
    } else {
        32
    }
}

pub struct Info {
    pub profile_format: u8,
    pub button_count: usize,
    pub sector_size: usize,
}

fn get_info(d: &Device) -> Result<Info> {
    let r = d.call(FEAT_ONBOARD_PROFILES, FN_GET_INFO, &[])?;
    if r.len() < 10 {
        return Err(Error::Protocol("onboard getInfo too short".into()));
    }
    Ok(Info {
        profile_format: r[1],
        button_count: r[5] as usize,
        sector_size: u16::from_be_bytes([r[7], r[8]]) as usize,
    })
}

/// (sector, enabled) for each profile, from the control sector.
fn get_headers(d: &Device, sector_size: usize) -> Result<Vec<(u16, u8)>> {
    let ctrl = read_sector(d, 0, sector_size.max(64))?;
    let mut headers = Vec::new();
    let mut i = 0usize;
    loop {
        let rec = &ctrl[i * 4..i * 4 + 4.min(ctrl.len() - i * 4)];
        if rec.len() < 3 || (rec[0] == 0xFF && rec[1] == 0xFF) {
            break;
        }
        let sector = u16::from_be_bytes([rec[0], rec[1]]);
        let enabled = rec[2];
        if sector == 0 {
            break;
        }
        headers.push((sector, enabled));
        i += 1;
        if i * 4 + 3 > ctrl.len() {
            break;
        }
    }
    Ok(headers)
}

/// Read `size` bytes of a sector, 16 bytes per request (mirrors Solaar).
fn read_sector(d: &Device, sector: u16, size: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let (sh, sl) = ((sector >> 8) as u8, (sector & 0xFF) as u8);
    let mut o = 0usize;
    while o + 15 < size {
        let chunk = d.call(FEAT_ONBOARD_PROFILES, FN_READ_SECTOR, &[sh, sl, (o >> 8) as u8, (o & 0xFF) as u8])?;
        if chunk.len() < 16 {
            return Err(Error::Protocol("short sector read".into()));
        }
        out.extend_from_slice(&chunk[..16]);
        o += 16;
    }
    if out.len() < size {
        // Final, possibly-overlapping chunk read from (size-16).
        let off = size - 16;
        let chunk = d.call(FEAT_ONBOARD_PROFILES, FN_READ_SECTOR, &[sh, sl, (off >> 8) as u8, (off & 0xFF) as u8])?;
        let skip = 16 + o - size;
        out.extend_from_slice(&chunk[skip..16.min(chunk.len())]);
    }
    out.truncate(size);
    Ok(out)
}

/// Write a full sector (bytes must already include the trailing CRC).
fn write_sector(d: &Device, sector: u16, bytes: &[u8]) -> Result<()> {
    let len = bytes.len();
    let (sh, sl) = ((sector >> 8) as u8, (sector & 0xFF) as u8);
    d.call(
        FEAT_ONBOARD_PROFILES,
        FN_START_WRITE,
        &[sh, sl, 0, 0, (len >> 8) as u8, (len & 0xFF) as u8],
    )?;
    let mut o = 0usize;
    while o + 1 < len {
        let end = (o + 16).min(len);
        d.call(FEAT_ONBOARD_PROFILES, FN_WRITE_DATA, &bytes[o..end])?;
        o += 16;
    }
    d.call(FEAT_ONBOARD_PROFILES, FN_END_WRITE, &[])?;
    Ok(())
}

// ---- CRC-16/CCITT-FALSE (init 0xFFFF, poly 0x1021, no reflect/xorout) --------

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

fn put_crc(sector: &mut [u8]) {
    let n = sector.len();
    let c = crc16(&sector[..n - 2]);
    sector[n - 2] = (c >> 8) as u8;
    sector[n - 1] = (c & 0xFF) as u8;
}

fn crc_ok(sector: &[u8]) -> bool {
    let n = sector.len();
    let stored = u16::from_be_bytes([sector[n - 2], sector[n - 1]]);
    crc16(&sector[..n - 2]) == stored
}

// ---- Button spec encode/decode ----------------------------------------------

fn mouse_button_name(mask: u16) -> String {
    match mask {
        0x0001 => "Left click".into(),
        0x0002 => "Right click".into(),
        0x0004 => "Middle click".into(),
        0x0008 => "Back".into(),
        0x0010 => "Forward".into(),
        0x0020 => "Button 6".into(),
        0x0040 => "Button 7".into(),
        0x0080 => "Button 8".into(),
        _ => format!("mouse mask 0x{mask:04X}"),
    }
}

fn function_name(f: u8) -> &'static str {
    match f {
        0x0 => "no action",
        0x1 => "tilt left",
        0x2 => "tilt right",
        0x3 => "next DPI",
        0x4 => "previous DPI",
        0x5 => "cycle DPI",
        0x6 => "default DPI",
        0x7 => "shift DPI",
        0x8 => "next profile",
        0x9 => "previous profile",
        0xA => "cycle profile",
        0xB => "G-shift",
        0xC => "battery status",
        0xD => "profile select",
        0xE => "mode switch",
        0xF => "host button",
        0x10 => "scroll down",
        0x11 => "scroll up",
        _ => "function?",
    }
}

/// Human-readable description of a 4-byte button spec.
fn decode_button(spec: &[u8]) -> String {
    if spec == [0xFF, 0xFF, 0xFF, 0xFF] {
        return "disabled".into();
    }
    let behavior = spec[0] >> 4;
    match behavior {
        0x8 => match spec[1] {
            0x0 => "no action".into(),
            0x1 => mouse_button_name(u16::from_be_bytes([spec[2], spec[3]])),
            0x2 => {
                let (mods, code) = (spec[2], spec[3]);
                match (mods, code) {
                    (0x08, 0x2F) => "Cmd+[ (browser Back)".into(),
                    (0x08, 0x30) => "Cmd+] (browser Forward)".into(),
                    _ => format!("key 0x{code:02X} (modifiers 0x{mods:02X})"),
                }
            }
            0x3 => match u16::from_be_bytes([spec[2], spec[3]]) {
                0x0224 => "AC Back (consumer)".into(),
                0x0225 => "AC Forward (consumer)".into(),
                c => format!("consumer 0x{c:04X}"),
            },
            _ => format!("send? {:02X?}", spec),
        },
        0x9 => function_name(spec[1]).to_string(),
        0x0 | 0x1 | 0x2 => format!("macro (behavior {behavior})"),
        _ => format!("raw {:02X?}", spec),
    }
}

/// Returns whether a spec is a clean, recognized button mapping.
fn spec_valid(spec: &[u8]) -> bool {
    if spec == [0xFF, 0xFF, 0xFF, 0xFF] {
        return true;
    }
    match spec[0] >> 4 {
        0x8 => matches!(spec[1], 0x0..=0x3),
        0x9 => spec[1] <= 0x11,
        _ => false,
    }
}

// HID keyboard modifier bits (byte 2 of a MODIFIER_AND_KEY spec):
// 0x01 LCtrl, 0x02 LShift, 0x04 LAlt, 0x08 LGUI(Cmd), 0x10 RCtrl, ...
const MOD_LGUI: u8 = 0x08; // Command on macOS
// HID keyboard usage codes.
const KEY_LBRACKET: u8 = 0x2F; // [ {
const KEY_RBRACKET: u8 = 0x30; // ] }
// HID consumer-control usage codes.
const AC_BACK: u16 = 0x0224;
const AC_FORWARD: u16 = 0x0225;

/// Parse a CLI target string into a 4-byte button spec.
fn parse_target(s: &str) -> std::result::Result<[u8; 4], String> {
    let t = s.to_ascii_lowercase();
    let mouse = |mask: u16| [0x80, 0x01, (mask >> 8) as u8, (mask & 0xFF) as u8];
    let func = |f: u8| [0x90, f, 0xFF, 0x00];
    let key = |mods: u8, code: u8| [0x80, 0x02, mods, code]; // SEND modifier+key
    let consumer = |code: u16| [0x80, 0x03, (code >> 8) as u8, (code & 0xFF) as u8]; // SEND consumer
    Ok(match t.as_str() {
        "left" | "leftclick" | "left-click" => mouse(0x0001),
        "right" | "rightclick" | "right-click" => mouse(0x0002),
        "middle" | "middleclick" | "middle-click" => mouse(0x0004),
        "back" => mouse(0x0008),
        "forward" => mouse(0x0010),
        "button6" => mouse(0x0020),
        "button7" => mouse(0x0040),
        "button8" => mouse(0x0080),
        "dpi-cycle" | "cycle-dpi" => func(0x5),
        "dpi-up" | "dpi-next" | "next-dpi" => func(0x3),
        "dpi-down" | "dpi-prev" | "previous-dpi" => func(0x4),
        "dpi-default" => func(0x6),
        // macOS-native browser navigation (what makes side buttons "just work"):
        "nav-back" | "cmd-[" | "cmd-lbracket" => key(MOD_LGUI, KEY_LBRACKET), // Cmd+[
        "nav-forward" | "cmd-]" | "cmd-rbracket" => key(MOD_LGUI, KEY_RBRACKET), // Cmd+]
        "ac-back" => consumer(AC_BACK),     // HID consumer "AC Back"
        "ac-forward" => consumer(AC_FORWARD), // HID consumer "AC Forward"
        "disable" | "none" | "off" => [0xFF, 0xFF, 0xFF, 0xFF],
        _ => {
            // consumer:XXXX  -> SEND consumer-control code (hex)
            if let Some(hex) = t.strip_prefix("consumer:") {
                let code = u16::from_str_radix(hex, 16).map_err(|_| format!("bad consumer code '{hex}'"))?;
                return Ok(consumer(code));
            }
            // key:MM:KK  -> SEND modifier(MM)+key(KK), both hex
            if let Some(rest) = t.strip_prefix("key:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    let mods = u8::from_str_radix(parts[0], 16).map_err(|_| "bad modifier hex".to_string())?;
                    let code = u8::from_str_radix(parts[1], 16).map_err(|_| "bad keycode hex".to_string())?;
                    return Ok(key(mods, code));
                }
            }
            // raw 8-hex-digit spec, e.g. 80010004
            let hex: String = t.chars().filter(|c| !c.is_whitespace()).collect();
            if hex.len() == 8 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                let mut b = [0u8; 4];
                for i in 0..4 {
                    b[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
                }
                return Ok(b);
            }
            return Err(format!(
                "unknown target '{s}'. Use: left right middle back forward button6-8, \
                 dpi-cycle dpi-up dpi-down dpi-default, nav-back nav-forward ac-back ac-forward, \
                 disable, consumer:XXXX, key:MM:KK, or a raw 8-hex-digit spec."
            ));
        }
    })
}

// ---- profile resolution ------------------------------------------------------

struct ActiveProfile {
    sector: u16,
    size: usize,
    button_count: usize,
    button_off: usize,
    data: Vec<u8>,
}

fn load_active_profile(d: &Device) -> Result<ActiveProfile> {
    if !d.has(FEAT_ONBOARD_PROFILES) {
        return Err(Error::NotSupported("device has no ONBOARD_PROFILES (0x8100)"));
    }
    let info = get_info(d)?;
    let headers = get_headers(d, info.sector_size)?;
    // First enabled profile, else the first one.
    let (sector, _) = headers
        .iter()
        .find(|(_, en)| *en != 0)
        .copied()
        .or_else(|| headers.first().copied())
        .ok_or(Error::Protocol("no onboard profiles found".into()))?;
    let data = read_sector(d, sector, info.sector_size)?;
    if !crc_ok(&data) {
        return Err(Error::Protocol(format!(
            "profile sector {sector} CRC mismatch — refusing to touch it (format may differ)"
        )));
    }
    let button_off = button_offset(info.profile_format);
    // Sanity: most button specs at this offset should parse cleanly.
    let valid = (0..info.button_count)
        .filter(|i| {
            let o = button_off + i * 4;
            data.get(o..o + 4).map(spec_valid).unwrap_or(false)
        })
        .count();
    if valid + 1 < info.button_count {
        return Err(Error::Protocol(format!(
            "button array not recognized at offset {button_off} ({valid}/{} specs valid)",
            info.button_count
        )));
    }
    Ok(ActiveProfile {
        sector,
        size: info.sector_size,
        button_count: info.button_count,
        button_off,
        data,
    })
}

fn backup_path(sector: u16) -> String {
    format!("./lowtech-backup-sector{sector}.bin")
}

// ---- commands ----------------------------------------------------------------

pub fn cmd_buttons(slot: Option<&str>) -> Result<()> {
    let api = new_api()?;
    let d = select_device(&api, slot)?;
    let p = load_active_profile(&d)?;
    println!(
        "Onboard profile (sector {}, {} buttons), CRC OK:",
        p.sector, p.button_count
    );
    for i in 0..p.button_count {
        let o = p.button_off + i * 4;
        let spec = &p.data[o..o + 4];
        println!(
            "  button {}: {}  [{:02X} {:02X} {:02X} {:02X}]",
            i + 1,
            decode_button(spec),
            spec[0],
            spec[1],
            spec[2],
            spec[3]
        );
    }
    println!("\nRemap with:  lowtech assign <button#> <target>");
    println!("Targets: left right middle back forward button6-8");
    println!("         dpi-cycle dpi-up dpi-down dpi-default disable");
    println!("         nav-back nav-forward (Cmd+[ / Cmd+]), ac-back ac-forward (consumer codes)");
    println!("         consumer:XXXX, key:MM:KK (hex modifier+keycode), or a raw 8-hex spec");
    Ok(())
}

pub fn cmd_assign(slot: Option<&str>, button: usize, target: &str) -> Result<()> {
    let spec = parse_target(target).map_err(Error::Protocol)?;

    let api = new_api()?;
    let d = select_device(&api, slot)?;
    let p = load_active_profile(&d)?;
    if button < 1 || button > p.button_count {
        return Err(Error::Protocol(format!(
            "button {button} out of range 1..{}",
            p.button_count
        )));
    }

    let o = p.button_off + (button - 1) * 4;
    let old = &p.data[o..o + 4];
    if old == spec {
        println!(
            "button {button} already set to '{}' — nothing to do",
            decode_button(&spec)
        );
        return Ok(());
    }

    // Backup the original sector before any write.
    let bpath = backup_path(p.sector);
    std::fs::write(&bpath, &p.data).map_err(|e| Error::Protocol(format!("backup write failed: {e}")))?;
    println!("backed up original sector to {bpath}");

    // Build the new sector: copy, patch the one button spec, recompute CRC.
    let mut new = p.data.clone();
    new[o..o + 4].copy_from_slice(&spec);
    put_crc(&mut new);

    println!(
        "button {button}: {} -> {}",
        decode_button(old),
        decode_button(&spec)
    );
    write_sector(&d, p.sector, &new)?;

    // Verify by reading back.
    let check = read_sector(&d, p.sector, p.size)?;
    if !crc_ok(&check) {
        return Err(Error::Protocol(
            "readback CRC bad after write — restore from backup with `lowtech restore`".into(),
        ));
    }
    let rb = &check[o..o + 4];
    if rb == spec {
        println!("verified: readback matches ({})", decode_button(rb));
    } else {
        return Err(Error::Protocol(format!(
            "readback mismatch (got {:02X?}) — restore with `lowtech restore`",
            rb
        )));
    }
    Ok(())
}

pub fn cmd_restore(slot: Option<&str>) -> Result<()> {
    let api = new_api()?;
    let d = select_device(&api, slot)?;
    let info = get_info(&d)?;
    let headers = get_headers(&d, info.sector_size)?;
    let (sector, _) = headers
        .iter()
        .find(|(_, en)| *en != 0)
        .copied()
        .or_else(|| headers.first().copied())
        .ok_or(Error::Protocol("no onboard profiles found".into()))?;
    let bpath = backup_path(sector);
    let data = std::fs::read(&bpath)
        .map_err(|e| Error::Protocol(format!("no backup at {bpath}: {e}")))?;
    if data.len() != info.sector_size || !crc_ok(&data) {
        return Err(Error::Protocol("backup file invalid (size/CRC)".into()));
    }
    write_sector(&d, sector, &data)?;
    let check = read_sector(&d, sector, info.sector_size)?;
    if crc_ok(&check) && check == data {
        println!("restored sector {sector} from {bpath}");
        Ok(())
    } else {
        Err(Error::Protocol("restore verification failed".into()))
    }
}
