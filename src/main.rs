//! lowtech — a minimal Rust port of the useful parts of Solaar (no UI).
//!
//! Talks HID++ to Logitech devices over the receiver's vendor HID interface.
//! Cross-platform via the `hidapi` crate (statically linked): macOS (IOKit) and
//! Linux (hidraw). See the CLI definition below for subcommands.

mod features;
mod hidpp;
mod onboard;

use clap::{Parser, Subcommand};
use hidapi::HidApi;
use hidpp::{Device, HIDPP_USAGE_PAGE, VENDOR_LOGITECH};
use std::ffi::CString;

/// Minimal Logitech HID++ tool (Rust port of Solaar's core) — no UI.
#[derive(Parser)]
#[command(name = "lowtech", version, about, long_about = None)]
struct Cli {
    /// Device to act on: receiver slot 1-6, or 'ff' for a direct device.
    /// Defaults to the first reachable device.
    #[arg(short, long, global = true, value_name = "SLOT")]
    slot: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Enumerate receivers and reachable devices
    #[command(alias = "ls")]
    List,
    /// Show device info (name, protocol, features, battery, DPI)
    Show,
    /// Read DPI, or set it to <value>
    Dpi {
        /// New DPI value; omit to just read the current one
        value: Option<u16>,
    },
    /// Print live mouse button presses
    Watch {
        /// Dump every raw HID report (for debugging)
        #[arg(long)]
        raw: bool,
    },
    /// List the onboard-profile button assignments
    Buttons,
    /// Remap a button via onboard profiles
    Assign {
        /// Physical button number (1..N, see `buttons`)
        button: usize,
        /// Action: left right middle back forward button6-8, dpi-cycle dpi-up
        /// dpi-down dpi-default, ac-back ac-forward nav-back nav-forward,
        /// disable, consumer:XXXX, key:MM:KK, or a raw 8-hex spec
        target: String,
    },
    /// Restore the onboard profile sector from the local backup
    Restore,
}

fn main() {
    let Cli { slot, command } = Cli::parse();
    let slot = slot.as_deref();

    let result = match command.unwrap_or(Commands::List) {
        Commands::List => cmd_list(),
        Commands::Show => cmd_show(slot),
        Commands::Dpi { value } => cmd_dpi(slot, value),
        Commands::Watch { raw } => cmd_watch(raw),
        Commands::Buttons => onboard::cmd_buttons(slot),
        Commands::Assign { button, target } => onboard::cmd_assign(slot, button, &target),
        Commands::Restore => onboard::cmd_restore(slot),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// A discovered HID++ vendor interface path.
fn vendor_paths(api: &HidApi) -> Vec<CString> {
    let mut paths = Vec::new();
    for info in api.device_list() {
        if info.vendor_id() == VENDOR_LOGITECH && info.usage_page() == HIDPP_USAGE_PAGE {
            let p = info.path().to_owned();
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
    }
    paths
}

/// Probe every reachable device across all vendor interfaces.
fn discover(api: &HidApi) -> Vec<Device> {
    let mut out = Vec::new();
    for path in vendor_paths(api) {
        // 0xFF = direct device; 1..=6 = receiver slots.
        for dn in [0xFFu8, 1, 2, 3, 4, 5, 6] {
            if let Ok(Some(dev)) = Device::probe(api, &path, dn) {
                out.push(dev);
            }
        }
    }
    out
}

/// A reachable HID++ 1.0 responder at 0xFF is the receiver itself, not a device.
fn is_receiver(d: &Device) -> bool {
    d.devnumber == 0xFF && d.protocol < 2.0
}

/// Pick a device by slot arg ("ff" or 1..6), else the first reachable real device.
pub(crate) fn select_device(api: &HidApi, slot: Option<&str>) -> hidpp::Result<Device> {
    let want: Option<u8> = match slot {
        Some(s) if s.eq_ignore_ascii_case("ff") => Some(0xFF),
        Some(s) => s.parse::<u8>().ok(),
        None => None,
    };
    for path in vendor_paths(api) {
        match want {
            // Explicit slot: honor exactly what was asked for.
            Some(dn) => {
                if let Ok(Some(dev)) = Device::probe(api, &path, dn) {
                    return Ok(dev);
                }
            }
            // Default: prefer a real device (HID++ >= 2.0) over the receiver.
            None => {
                for dn in [1, 2, 3, 4, 5, 6, 0xFF] {
                    if let Ok(Some(dev)) = Device::probe(api, &path, dn) {
                        if !is_receiver(&dev) {
                            return Ok(dev);
                        }
                    }
                }
            }
        }
    }
    Err(hidpp::Error::Protocol(match want {
        Some(dn) => format!("no device reachable at slot {dn}"),
        None => "no reachable Logitech HID++ device found".into(),
    }))
}

pub(crate) fn new_api() -> hidpp::Result<HidApi> {
    HidApi::new().map_err(hidpp::Error::from)
}

fn cmd_list() -> hidpp::Result<()> {
    let api = new_api()?;
    let paths = vendor_paths(&api);
    if paths.is_empty() {
        println!("No Logitech HID++ vendor interface found.");
        return Ok(());
    }
    println!("Logitech HID++ interfaces: {}", paths.len());
    let devices = discover(&api);
    if devices.is_empty() {
        println!("  (no reachable devices — receiver present but slots empty/asleep?)");
        return Ok(());
    }
    for d in &devices {
        if is_receiver(d) {
            println!("  [receiver] Logitech receiver  (HID++ {:.1})", d.protocol);
            continue;
        }
        let slot = if d.devnumber == 0xFF {
            "direct".to_string()
        } else {
            format!("slot {}", d.devnumber)
        };
        let name = d.name().unwrap_or_else(|_| "<name unavailable>".into());
        println!("  [{slot}] {name}  (HID++ {:.1}, {} features)", d.protocol, d.features.len());
    }
    Ok(())
}

fn cmd_show(slot: Option<&str>) -> hidpp::Result<()> {
    let api = new_api()?;
    let d = select_device(&api, slot)?;

    let slot = if d.devnumber == 0xFF { "direct".into() } else { format!("slot {}", d.devnumber) };
    println!("Device [{slot}]");
    match d.name() {
        Ok(n) => println!("  Name:     {n}"),
        Err(e) => println!("  Name:     <unavailable: {e}>"),
    }
    if let Ok(t) = d.kind() {
        println!("  Type:     {} ({t})", features::device_type_name(t));
    }
    println!("  HID++:    {:.1}", d.protocol);

    match d.battery() {
        Ok((Some(pct), st)) => println!("  Battery:  {pct}% ({})", features::battery_status_name(st)),
        Ok((None, st)) => println!("  Battery:  ? ({})", features::battery_status_name(st)),
        Err(_) => {}
    }

    if let Ok(dpi) = features::dpi_read(&d) {
        if dpi.has_y && dpi.x != dpi.y {
            print!("  DPI:      X={} Y={}", dpi.x, dpi.y);
        } else {
            print!("  DPI:      {}", dpi.x);
        }
        if dpi.has_lod {
            print!("  (LOD={})", dpi.lod);
        }
        println!();
    }

    println!("  Features: {}", d.features.len());
    for fid in &d.feature_order {
        let (idx, ver) = d.features[fid];
        let name = features::feature_name(*fid);
        let label = if name.is_empty() { format!("unknown:{fid:04X}") } else { name.to_string() };
        println!("    [{idx:2}] 0x{fid:04X} v{ver}  {label}");
    }
    Ok(())
}

fn cmd_dpi(slot: Option<&str>, value: Option<u16>) -> hidpp::Result<()> {
    let api = new_api()?;
    let d = select_device(&api, slot)?;
    match value {
        None => {
            let dpi = features::dpi_read(&d)?;
            if dpi.has_y && dpi.x != dpi.y {
                println!("DPI: X={} Y={}", dpi.x, dpi.y);
            } else {
                println!("DPI: {}", dpi.x);
            }
        }
        Some(v) => {
            let before = features::dpi_read(&d)?;
            let after = features::dpi_set(&d, v)?;
            println!("DPI: {} -> {}", before.x, after.x);
        }
    }
    Ok(())
}

fn cmd_watch(raw: bool) -> hidpp::Result<()> {
    let api = new_api()?;

    // A Logitech mouse input interface (Generic Desktop / Mouse). Button presses
    // arrive here as a standard HID button bitfield.
    struct Iface {
        label: String,
        dev: hidapi::HidDevice,
        prev_sig: Vec<u8>,
        // distinct values seen at each byte index, to classify motion bytes.
        seen: Vec<std::collections::HashSet<u8>>,
        count: u32,
    }
    let mut ifaces: Vec<Iface> = Vec::new();
    let mut open_err: Option<hidapi::HidError> = None;

    for info in api.device_list() {
        if info.vendor_id() != VENDOR_LOGITECH || info.usage_page() != 0x0001 || info.usage() != 0x02 {
            continue;
        }
        match api.open_path(info.path()) {
            Ok(dev) => {
                let label = info
                    .product_string()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Logitech mouse")
                    .to_string();
                ifaces.push(Iface {
                    label,
                    dev,
                    prev_sig: Vec::new(),
                    seen: Vec::new(),
                    count: 0,
                });
            }
            Err(e) => open_err = Some(e),
        }
    }

    if ifaces.is_empty() {
        eprintln!(
            "No Logitech mouse interface opened{}.\n\
             On macOS reading input may need Input Monitoring permission:\n  \
             System Settings -> Privacy & Security -> Input Monitoring -> enable your terminal, then quit & reopen it.",
            open_err.map(|e| format!(" ({e})")).unwrap_or_default()
        );
        return Err(hidpp::Error::Protocol("no mouse interface opened".into()));
    }

    println!(
        "Watching {} Logitech mouse interface(s). Move briefly to calibrate, then press buttons. (Ctrl-C to stop.)",
        ifaces.len()
    );

    // Motion bytes vary continuously (many distinct values, and signed deltas dip
    // to 0xFF/0xFE); button bytes take a small set (rest + a few small masks).
    const MOTION_DISTINCT: usize = 12;
    // Reports to observe per device before decoding, so motion bytes self-classify.
    const WARMUP: u32 = 24;

    // A byte is motion if it varies a lot, or has shown a high value (negative
    // delta high-byte) that a small button mask never would.
    let is_motion = |s: &std::collections::HashSet<u8>| -> bool {
        s.len() >= MOTION_DISTINCT || s.iter().any(|&v| v >= 0xF0)
    };

    // Non-blocking drain keeps us current with high-rate mice so button events
    // aren't delayed behind a backlog of motion reports.
    let mut buf = [0u8; 64];
    loop {
        let mut got_any = false;
        for iface in ifaces.iter_mut() {
            // Drain every report pending on this interface right now.
            while let Ok(n) = iface.dev.read_timeout(&mut buf, 0) {
                if n == 0 {
                    break;
                }
                got_any = true;
                let report = &buf[..n];
                iface.count = iface.count.saturating_add(1);
                if iface.seen.len() < n {
                    iface.seen.resize(n, std::collections::HashSet::new());
                }
                for (i, &b) in report.iter().enumerate() {
                    if iface.seen[i].len() < MOTION_DISTINCT {
                        iface.seen[i].insert(b);
                    }
                }
                // Change key: keep only non-motion bytes, so it changes on
                // buttons, not movement. (Constant bytes like the report id
                // never change it.)
                let sig: Vec<u8> = report
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| if is_motion(&iface.seen[i]) { 0 } else { b })
                    .collect();
                if sig == iface.prev_sig {
                    continue; // motion-only frame
                }
                iface.prev_sig = sig.clone();

                if raw {
                    println!("[{}] raw[{n:2}]: {}", iface.label, hex(report));
                }
                if iface.count < WARMUP {
                    continue; // still learning which bytes are motion
                }

                // The button byte is the lowest-index byte that varies within a
                // small range (rest + masks) — not the report id, not motion.
                let btn = sig.iter().enumerate().find(|(i, _)| {
                    let d = iface.seen[*i].len();
                    (2..MOTION_DISTINCT).contains(&d) && !is_motion(&iface.seen[*i])
                });
                if let Some((_, &b)) = btn {
                    let pressed = decode_buttons(b);
                    if pressed.is_empty() {
                        println!("[{}] (released)", iface.label);
                    } else {
                        println!("[{}] pressed: {}  [0x{b:02X}]", iface.label, pressed.join(", "));
                    }
                }
            }
        }
        if !got_any {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

fn decode_buttons(mask: u8) -> Vec<String> {
    const NAMES: [&str; 8] = [
        "Left", "Right", "Middle", "Back", "Forward", "Button6", "Button7", "Button8",
    ];
    (0..8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| NAMES[bit].to_string())
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}
