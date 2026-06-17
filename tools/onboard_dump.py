#!/usr/bin/env python3
"""Read-only dump + decode of onboard profile sectors (0x8100) for validation."""
import ctypes, struct, time

H = ctypes.cdll.LoadLibrary("/opt/homebrew/lib/libhidapi.dylib")
class DI(ctypes.Structure): pass
DI._fields_ = [("path", ctypes.c_char_p), ("vendor_id", ctypes.c_ushort), ("product_id", ctypes.c_ushort),
    ("serial_number", ctypes.c_wchar_p), ("release_number", ctypes.c_ushort), ("manufacturer_string", ctypes.c_wchar_p),
    ("product_string", ctypes.c_wchar_p), ("usage_page", ctypes.c_ushort), ("usage", ctypes.c_ushort),
    ("interface_number", ctypes.c_int), ("next", ctypes.POINTER(DI)), ("bus_type", ctypes.c_int)]
H.hid_enumerate.restype = ctypes.POINTER(DI); H.hid_enumerate.argtypes = [ctypes.c_ushort, ctypes.c_ushort]
H.hid_open_path.restype = ctypes.c_void_p; H.hid_open_path.argtypes = [ctypes.c_char_p]
H.hid_write.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
H.hid_read_timeout.restype = ctypes.c_int; H.hid_read_timeout.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int]
H.hid_init()
try: H.hid_darwin_set_open_exclusive(0)
except Exception: pass

SW = 0x0B
def open_vendor():
    p = H.hid_enumerate(0x046D, 0); head = p
    path = None
    while p:
        d = p.contents
        if d.usage_page == 0xFF00 and path is None:
            path = d.path
        p = d.next
    return H.hid_open_path(path) if path else None

def request(h, dn, req_id, params=b"", timeout=1.0):
    if req_id < 0x8000: req_id = (req_id & 0xFFF0) | SW
    rd = struct.pack("!H", req_id) + params
    frame = struct.pack("!BB18s", 0x11, dn, rd) if len(rd) > 5 else struct.pack("!BB5s", 0x10, dn, rd)
    H.hid_write(h, frame, len(frame))
    end = time.time() + timeout
    while time.time() < end:
        buf = ctypes.create_string_buffer(32)
        n = H.hid_read_timeout(h, buf, 32, int(timeout*1000))
        if n <= 0: continue
        data = buf.raw[:n]
        if data[1] != dn and data[1] != (dn ^ 0xFF): continue
        pl = data[2:]
        if pl[:1] == b"\xff" and pl[1:3] == rd[:2]: return ("err", pl[3])
        if pl[:2] == rd[:2]: return ("ok", pl[2:])
    return ("timeout", None)

def fr(h, dn, fn, *params):
    p = bytes(params)
    st, r = request(h, dn, (0x0D << 8) | (fn & 0xFF), p)  # 0x8100 is feature index 13 (0x0D)
    return r if st == "ok" else None

CRC_POLY = 0x1021
def crc16(data):
    crc = 0xFFFF
    for b in data:
        crc = ((crc << 8) ^ _tbl[((crc >> 8) ^ b) & 0xFF]) & 0xFFFF
    return crc
_tbl = []
for i in range(256):
    c = i << 8
    for _ in range(8):
        c = ((c << 1) ^ CRC_POLY) & 0xFFFF if (c & 0x8000) else (c << 1) & 0xFFFF
    _tbl.append(c)

h = open_vendor()
dn = 1
info = fr(h, dn, 0x00)
print("getInfo:", info.hex())
memory, pformat, macro = info[0], info[1], info[2]
count, oob, buttons, sectors = info[3], info[4], info[5], info[6]
size = struct.unpack("!H", info[7:9])[0]
shift = info[9]
print(f"  memory=0x{memory:02X} profileFormat=0x{pformat:02X} macroFormat=0x{macro:02X}")
print(f"  count={count} oob={oob} buttons={buttons} sectors={sectors} size={size} shift=0x{shift:02X}")

# mode
mode = fr(h, dn, 0x20)
print("getMode:", mode.hex() if mode else None, "(01=onboard 02=host)")
cur = fr(h, dn, 0x40)  # getCurrentProfile? try
print("fn0x40:", cur.hex() if cur else None)

def read_sector(h, dn, sector, s):
    out = b""; o = 0
    while o < s - 15:
        chunk = fr(h, dn, 0x50, sector >> 8, sector & 0xFF, o >> 8, o & 0xFF)
        if chunk is None: break
        out += chunk[:16]; o += 16
    if len(out) < s:
        chunk = fr(h, dn, 0x50, sector >> 8, sector & 0xFF, (s - 16) >> 8, (s - 16) & 0xFF)
        if chunk: out += chunk[16 + o - s:]
    return out[:s]

# profile headers from control sector 0
print("\n--- control sector 0 (first 32 bytes) ---")
hdr = read_sector(h, dn, 0, 64)
print(hdr[:32].hex())
headers = []
i = 0
while True:
    rec = hdr[i*4:i*4+4]
    if len(rec) < 3 or rec[0:2] == b"\xff\xff": break
    sector, enabled = struct.unpack("!HB", rec[0:3])
    if sector == 0: break
    headers.append((sector, enabled)); i += 1
print("profile headers (sector,enabled):", headers)

for size_try in (size, 256, 512):
    if not headers: break
    sec = headers[0][0]
    data = read_sector(h, dn, sec, size_try)
    if len(data) < size_try: continue
    stored = struct.unpack("!H", data[size_try-2:size_try])[0]
    calc = crc16(data[:size_try-2])
    print(f"\nsector {sec} size_try={size_try}: stored_crc=0x{stored:04X} calc_crc=0x{calc:04X} {'MATCH' if stored==calc else 'no'}")
    if stored == calc:
        print("  *** CRC VALIDATED — format understood at this size ***")
        report_rate = data[0]; rdi = data[1]; rsi = data[2]
        resolutions = [struct.unpack('<H', data[3+i*2:5+i*2])[0] for i in range(5)]
        name = data[160:208].decode('utf-16le', 'replace').rstrip('\x00').rstrip('￿')
        print(f"  report_rate={report_rate} default_dpi_idx={rdi} shift_dpi_idx={rsi} resolutions={resolutions}")
        print(f"  name={name!r}")
        print("  buttons:")
        for b in range(buttons):
            spec = data[32+b*4:32+b*4+4]
            print(f"    btn{b+1}: {spec.hex()}  behavior=0x{spec[0]>>4:X}")
        break

print("\n=== FULL RAW SECTOR 1 (offset: bytes) ===")
sec = headers[0][0]
data = read_sector(h, dn, sec, 255)
for off in range(0, len(data), 16):
    row = data[off:off+16]
    print(f"  {off:3} (0x{off:02X}): {row.hex(' ')}")

print("\n=== scan for SEND/BUTTON (8x 01 ..) and FUNCTION (9x ..) 4-byte specs ===")
for off in range(0, len(data)-3):
    b0 = data[off]
    beh = b0 >> 4
    if beh == 0x8 and data[off+1] in (0x00,0x01,0x02,0x03):
        print(f"  @off {off:3}: {data[off:off+4].hex(' ')}  SEND type={data[off+1]}")
    elif beh == 0x9 and data[off+1] <= 0x11:
        print(f"  @off {off:3}: {data[off:off+4].hex(' ')}  FUNCTION fn={data[off+1]}")

# also dump profiles 2..5 (disabled defaults) sector starts for comparison
print("\n=== compare: first 64 bytes of each profile sector ===")
for (s, en) in headers:
    d = read_sector(h, dn, s, 64)
    print(f"  sector {s} (enabled={en}): {d[:64].hex(' ')}")
