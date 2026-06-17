# lowtech

A minimal Rust port of the useful core of [Solaar](https://github.com/pwr-Solaar/Solaar)
for Logitech HID++ devices — **no UI**, just the things you actually want from a
terminal: see your devices, read DPI/battery, watch button presses, and
**reassign mouse buttons**. Single self-contained binary, system libraries
statically linked.

Built and verified against a **Logitech PRO X SUPERLIGHT 2** on a Logitech Bolt
receiver, on macOS (Apple Silicon). Also builds on **Linux** (hidraw backend) —
see [Build](#build).

## A bit of lore

Logitech G HUB just didn't work for me, so I figured I'd port
[Solaar][solaar] — the excellent Linux tool for Logitech devices — into
something I could actually run. Then it turned out Solaar didn't really support
the mouse I had at the time, the LIGHTSPEED "2c" (PRO X SUPERLIGHT 2): its
onboard profiles use a format newer than Solaar handles, so it couldn't remap
the buttons. So this became its own thing — **deliberately minimal**, just the
handful of features I actually use, talking HID++ straight to the device.

It runs on **macOS and Linux**, but feature parity with Solaar or G HUB isn't the goal — it's built for setting up a minimal onboard mapping once and forgetting about it: the config lives on the mouse, so nothing has to keep running.

Huge thanks to **Solaar**: the HID++ feature list, control IDs, and the initial
hardware/button mappings used here were derived from its work.

[solaar]: https://github.com/pwr-Solaar/Solaar

## Why not just use Solaar?

Solaar is a large Python + GTK application. On macOS:

- `make install_udev` **can't work** — it `sudo cp`s rules into `/etc/udev/rules.d/`
  and runs `udevadm`, but **udev is Linux-only**. Those rules only grant device
  permissions on Linux; macOS uses a different model entirely (TCC).
- It needs PyGObject/GTK, dbus, pyyaml, etc. installed via Homebrew.
- For newer gaming mice it doesn't do what you want anyway: the PRO X
  SUPERLIGHT 2 reports **onboard-profile format 7**, and Solaar only supports
  formats ≤ 5 (it silently bails), so it **cannot remap this mouse's buttons**.

`lowtech` talks HID++ directly over the receiver's vendor HID interface
(usage page `0xFF00`), which needs **no special permission** on macOS.

## What it does

```
lowtech list                      enumerate receivers and reachable devices
lowtech show                      name, type, HID++ version, battery, DPI, feature list
lowtech dpi [VALUE]               read DPI (or set it to VALUE — see note)
lowtech buttons                   list the onboard-profile button assignments
lowtech assign <BUTTON> <TARGET>  remap a button (writes the mouse's flash)
lowtech restore                   restore the profile sector from the local backup
lowtech watch [--raw]             print live mouse button presses
```

The CLI uses [clap](https://crates.io/crates/clap): `lowtech --help`,
`lowtech <cmd> --help`, and `lowtech --version` all work.

Use `-s`/`--slot <SLOT>` (global) to pick a device — a receiver device number
`1`..`6`, or `ff` for a directly-connected device. Omitted, it uses the first
reachable device (e.g. `lowtech show`, `lowtech --slot 2 buttons`).

`watch` reads the Logitech mouse's HID input report and prints button presses
(move the mouse briefly so it can classify the motion bytes, then press buttons).
`--raw` dumps the raw HID reports for debugging. Buttons decode with the standard
HID order (bit0=Left, 1=Right, 2=Middle, 3=Back, 4=Forward, ...).

### Examples

```console
$ lowtech list
Logitech HID++ interfaces: 1
  [receiver] Logitech receiver  (HID++ 1.0)
  [slot 1] PRO X SUPERLIGHT 2c  (HID++ 4.2, 33 features)

$ lowtech show
Device [slot 1]
  Name:     PRO X SUPERLIGHT 2c
  Type:     Mouse (3)
  HID++:    4.2
  Battery:  49% (discharging)
  DPI:      800  (LOD=2)
  Features: 33
    ...

$ lowtech buttons
Onboard profile (sector 1, 5 buttons), CRC OK:
  button 1: Left click   [80 01 00 01]
  button 2: Right click  [80 01 00 02]
  button 3: Middle click [80 01 00 04]
  button 4: Back         [80 01 00 08]
  button 5: Forward      [80 01 00 10]

# remap the "forward" button to middle-click
$ lowtech assign 5 middle
backed up original sector to ./lowtech-backup-sector1.bin
button 5: Forward -> Middle click
verified: readback matches (Middle click)

# put it back
$ lowtech assign 5 forward
```

`assign` targets:

- mouse buttons: `left right middle back forward button6 button7 button8`
- DPI: `dpi-cycle dpi-up dpi-down dpi-default`
- browser navigation: `ac-back` / `ac-forward` (HID consumer codes, OS-agnostic),
  or `nav-back` / `nav-forward` (the macOS Cmd+[ / Cmd+] shortcut)
- `disable`
- advanced: `consumer:XXXX` (hex consumer code), `key:MM:KK` (hex modifier+keycode),
  or a raw 8-hex-digit spec (e.g. `80010004`)

Because `lowtech` writes the raw HID++ button spec, you're **not limited to the
presets a vendor app exposes**. G HUB only lets you pick from its own list;
here a button can be any standard HID action — any mouse button, any keyboard
key + modifiers (`key:MM:KK`), or any consumer/media usage (`consumer:XXXX`) —
written straight into the onboard profile.

### Optional: make the side buttons send browser Back/Forward

By default the side buttons send raw HID mouse buttons 4 and 5. Most browsers
treat those as Back/Forward, but if yours doesn't you can remap them in the
onboard profile to send explicit navigation events — no driver needed:

```console
# button 4 = back side button, button 5 = forward side button
$ lowtech assign 4 ac-back       # HID consumer "AC Back"  (macOS/Windows/Linux)
$ lowtech assign 5 ac-forward    # HID consumer "AC Forward"
# or, for the macOS browser keyboard shortcut instead:
$ lowtech assign 4 nav-back      # Cmd+[
$ lowtech assign 5 nav-forward   # Cmd+]
```

`ac-back`/`ac-forward` are the HID-standard, OS-agnostic codes for browser
navigation. The change is written to onboard memory and persists with no driver
running; `lowtech restore` reverts it.

## How button assignment works (and why it's safe)

This mouse has no `0x1B04` (Reprogrammable Controls). Buttons live in the
**onboard profile** (`0x8100`), as 4-byte specs inside a 255-byte profile sector
that ends with a CRC-16/CCITT-FALSE checksum. In format 7 the button array sits
at **offset 48** (format ≤5 used offset 32; that 16-byte shift is why Solaar's
parser misreads this mouse).

`assign` is conservative:

1. Reads the active profile sector and **validates its CRC** — refuses to touch
   it if the format isn't understood.
2. Writes a **backup** of the original bytes to `./lowtech-backup-sector<N>.bin`.
3. Patches only the 4 bytes of the target button, recomputes the CRC, writes the
   sector (`startWrite` / `writeData` / `endWrite`).
4. **Reads back** and verifies both the CRC and the changed button.

If anything looks wrong it tells you to run `lowtech restore`.

> Note: `dpi <value>` (the host-side `0x2202` write) is rejected while the mouse
> is in onboard mode (`0x05 logitech internal`) — on this mouse DPI is governed
> by the onboard profile, same as buttons.

## Build

Same command on either OS — the `hidapi` backend is selected per platform in
`Cargo.toml` (`macos-shared-device` on macOS, `linux-static-hidraw` on Linux):

```
cargo build --release
./target/release/lowtech list
```

### macOS

The bundled C hidapi is **statically compiled in** — no `libhidapi.dylib`
runtime dependency. `otool -L` shows only Apple's own system frameworks
(IOKit / CoreFoundation / AppKit) and libSystem, which **cannot** be statically
linked on macOS by design:

```
$ otool -L target/release/lowtech
    /System/Library/Frameworks/IOKit.framework/...
    /System/Library/Frameworks/CoreFoundation.framework/...
    /System/Library/Frameworks/AppKit.framework/...
    /usr/lib/libiconv.2.dylib
    /usr/lib/libSystem.B.dylib
```

`watch` (reading the mouse input report) may require **Input Monitoring**
permission: System Settings → Privacy & Security → Input Monitoring → enable
your terminal, then quit & reopen it. The HID++ commands don't need it.

### Linux

hidapi is compiled in statically using the **hidraw** backend (the same one
Solaar uses). `libudev` remains a dynamic dependency for device enumeration:

```
$ ldd target/release/lowtech
    libudev.so.1 => ...
    libc.so.6 => ...
```

For a fully self-contained binary, build against musl with a static
`libudev`/`libusb` (e.g. `cargo build --release --target x86_64-unknown-linux-musl`).

**Permissions:** by default `hidraw` nodes are root-only. Install the bundled
udev rule so your user can talk to Logitech devices without `sudo` (this is the
piece Solaar's broken `make install_udev` was trying to do):

```
sudo cp linux/42-logitech-hidpp.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then replug the receiver.

## Layout

- `src/hidpp.rs` — HID++ transport: framing, request/reply, ping, feature discovery.
- `src/features.rs` — id→name tables and DPI (0x2202) read/set.
- `src/onboard.rs` — onboard-profile (0x8100) read + safe button remap.
- `src/main.rs` — CLI.
- `tools/probe.py`, `tools/onboard_dump.py` — the read-only Python scripts used to
  reverse-engineer the protocol and the format-7 sector layout against the real
  device (run via the system `libhidapi`). Kept for reference.

## Status / limitations

- Tested on PRO X SUPERLIGHT 2c via a Bolt receiver, macOS arm64. The Linux
  build (hidraw) shares all the code but has had less hands-on testing.
- `watch` decodes the button bitfield from the mouse input report (byte 1). The
  bit→name mapping follows the standard HID layout; press each button to confirm.
- `dpi` set, persistent (`0x1C00`) remapping, profiles/macros, RGB, and the
  generic `0x1B04` remap path are not implemented (not needed for this mouse).
- Talking to paired devices through the receiver and to direct USB/BT devices is
  supported; only single-receiver setups have been exercised.

## Credits

Built on the shoulders of [Solaar](https://github.com/pwr-Solaar/Solaar)
(GPL-2.0-or-later). The HID++ feature ids, control ids, and onboard-profile /
button-spec encodings used here were derived from Solaar's implementation — this
project would not exist without it. `lowtech` is an independent, deliberately
minimal reimplementation in Rust; any mistakes are mine.
