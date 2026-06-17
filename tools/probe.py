#!/usr/bin/env python3
"""Minimal HID++ probe via libhidapi (mirrors Solaar's transport) to get ground truth."""
import ctypes, struct, sys, time

H = ctypes.cdll.LoadLibrary("/opt/homebrew/lib/libhidapi.dylib")

class DevInfo(ctypes.Structure): pass
DevInfo._fields_ = [
    ("path", ctypes.c_char_p), ("vendor_id", ctypes.c_ushort), ("product_id", ctypes.c_ushort),
    ("serial_number", ctypes.c_wchar_p), ("release_number", ctypes.c_ushort),
    ("manufacturer_string", ctypes.c_wchar_p), ("product_string", ctypes.c_wchar_p),
    ("usage_page", ctypes.c_ushort), ("usage", ctypes.c_ushort),
    ("interface_number", ctypes.c_int), ("next", ctypes.POINTER(DevInfo)), ("bus_type", ctypes.c_int)]

H.hid_enumerate.restype = ctypes.POINTER(DevInfo)
H.hid_enumerate.argtypes = [ctypes.c_ushort, ctypes.c_ushort]
H.hid_open_path.restype = ctypes.c_void_p
H.hid_open_path.argtypes = [ctypes.c_char_p]
H.hid_write.restype = ctypes.c_int
H.hid_write.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
H.hid_read_timeout.restype = ctypes.c_int
H.hid_read_timeout.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int]
H.hid_get_input_report.restype = ctypes.c_int
H.hid_get_input_report.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
H.hid_close.argtypes = [ctypes.c_void_p]
H.hid_init()
try:
    H.hid_darwin_set_open_exclusive(0)
except Exception: pass

def enumerate_logitech():
    out = []
    p = H.hid_enumerate(0x046D, 0)
    head = p
    while p:
        d = p.contents
        out.append({"path": d.path, "vid": d.vendor_id, "pid": d.product_id,
                    "usage_page": d.usage_page, "usage": d.usage, "iface": d.interface_number,
                    "product": d.product_string, "bus": d.bus_type})
        p = d.next
    if head: H.hid_free_enumeration(head)
    return out

def write(h, data):
    return H.hid_write(h, data, len(data))

def read(h, timeout_ms, n=32):
    buf = ctypes.create_string_buffer(n)
    r = H.hid_read_timeout(h, buf, n, timeout_ms)
    return buf.raw[:r] if r > 0 else b""

def has_hidpp(h):
    # check short (0x10) and long (0x11) input reports
    short = long = False
    buf = ctypes.create_string_buffer(32); buf[0] = bytes([0x10])
    if H.hid_get_input_report(h, buf, 32) == 7 and buf.raw[0] == 0x10: short = True
    buf = ctypes.create_string_buffer(32); buf[0] = bytes([0x11])
    if H.hid_get_input_report(h, buf, 32) == 20 and buf.raw[0] == 0x11: long = True
    return short, long

SW = 0x0B
def request(h, devnumber, req_id, params=b"", timeout=1.0, long_msg=False):
    if req_id < 0x8000:
        req_id = (req_id & 0xFFF0) | SW
    rd = struct.pack("!H", req_id) + params
    if long_msg or len(rd) > 5:
        frame = struct.pack("!BB18s", 0x11, devnumber, rd)
    else:
        frame = struct.pack("!BB5s", 0x10, devnumber, rd)
    write(h, frame)
    end = time.time() + timeout
    while time.time() < end:
        data = read(h, int(timeout*1000))
        if not data: continue
        rid = data[0]; rdn = data[1]
        if rdn != devnumber and rdn != (devnumber ^ 0xFF): continue
        payload = data[2:]
        if rid == 0x10 and payload[:1] == b"\x8f" and payload[1:3] == rd[:2]:
            return ("err10", payload[3])
        if payload[:1] == b"\xff" and payload[1:3] == rd[:2]:
            return ("err20", payload[3])
        if payload[:2] == rd[:2]:
            return ("ok", payload[2:])
    return ("timeout", None)

def ping(h, devnumber, long_msg=False):
    req_id = 0x0010 | SW
    mark = 0x55
    rd = struct.pack("!HBBB", req_id, 0, 0, mark)
    frame = struct.pack("!BB18s", 0x11, devnumber, rd) if long_msg else struct.pack("!BB5s", 0x10, devnumber, rd)
    write(h, frame)
    end = time.time() + 2.0
    while time.time() < end:
        data = read(h, 2000)
        if not data: continue
        rid, rdn, payload = data[0], data[1], data[2:]
        if rdn != devnumber and rdn != (devnumber ^ 0xFF): continue
        if payload[:2] == rd[:2] and (len(payload) > 4 and payload[4] == mark):
            return payload[2] + payload[3] / 10.0
        if rid == 0x10 and payload[:1] == b"\x8f" and payload[1:3] == rd[:2]:
            err = payload[3]
            if err == 0x01: return 1.0   # invalid subid -> HID++1.0
            if err in (0x09, 0x08): return None
            if err == 0x03: return "no-device"
    return None

print("=== Logitech HID interfaces ===")
devs = enumerate_logitech()
seen = {}
for d in devs:
    if d["path"] not in seen: seen[d["path"]] = d
for d in seen.values():
    print(f"  path={d['path'].decode()} pid={d['pid']:04X} usage_page={d['usage_page']:04X} "
          f"usage={d['usage']:02X} iface={d['iface']} bus={d['bus']} product={d['product']!r}")

print("\n=== Probing HID++ on each interface ===")
for d in seen.values():
    h = H.hid_open_path(d["path"])
    if not h:
        print(f"  {d['path'].decode()}: OPEN FAILED"); continue
    s, l = has_hidpp(h)
    print(f"  pid={d['pid']:04X} up={d['usage_page']:04X} short={s} long={l}")
    if s or l:
        # try direct device ping (0xFF) and receiver slots 1..6
        for dn in [0xFF, 1, 2, 3, 4, 5, 6]:
            pv = ping(h, dn, long_msg=l and not s)
            if pv and pv != "no-device":
                print(f"     devnumber {dn}: protocol {pv}")
                # root: get FEATURE_SET index
                st, fs = request(h, dn, 0x0000, struct.pack("!H", 0x0001), long_msg=l and not s)
                if st == "ok":
                    fsi = fs[0]
                    print(f"       FEATURE_SET idx={fsi}")
                    st2, cnt = request(h, dn, fsi << 8, long_msg=l and not s)
                    if st2 == "ok":
                        n = cnt[0]
                        print(f"       feature count={n}")
                        feats = []
                        for i in range(n+1):
                            st3, fd = request(h, dn, (fsi<<8)|0x10, struct.pack("!B", i), long_msg=l and not s)
                            if st3 == "ok":
                                fid = struct.unpack("!H", fd[:2])[0]
                                feats.append((i, fid, fd[2], fd[3]))
                        for i, fid, ftype, fver in feats:
                            mark = "  <-- REPROG_CONTROLS_V4" if fid == 0x1B04 else ("  <-- DEVICE_NAME" if fid==0x0005 else "")
                            print(f"         [{i:2}] 0x{fid:04X} type=0x{ftype:02X} v{fver}{mark}")
    H.hid_close(h)

print("\n=== Device name + onboard profile probe (devnumber 1) ===")
for d in seen.values():
    if d["usage_page"] != 0xFF00: continue
    h = H.hid_open_path(d["path"])
    if not h: continue
    dn = 1
    # DEVICE_NAME 0x0005 is at feature index 3
    st, c = request(h, dn, 0x0300, b"")  # getDeviceNameCount
    if st == "ok":
        total = c[0]; name = b""
        off = 0
        while len(name) < total:
            st2, chunk = request(h, dn, 0x0310, struct.pack("!B", off))
            if st2 != "ok": break
            name += chunk[:min(16, total-len(name))]; off = len(name)
        print(f"  device name: {name.decode('ascii','replace')!r} (len {total})")
    st, t = request(h, dn, 0x0320, b"")  # getDeviceType
    if st == "ok": print(f"  device type id: {t[0]}")
    # ONBOARD_PROFILES 0x8100 at index 13: fn 0x00 getInfo
    st, info = request(h, dn, 0x0D00, b"")
    if st == "ok":
        print(f"  onboard(0x8100) getInfo: {info.hex()}")
    # getMode fn 0x20
    st, m = request(h, dn, 0x0D20, b"")
    if st == "ok": print(f"  onboard getMode: {m.hex()}  (0x01=onboard,0x02=host)")
    # 0x8110 at index 14 - probe getInfo
    st, x = request(h, dn, 0x0E00, b"")
    if st == "ok": print(f"  feature 0x8110 fn0: {x.hex()}")
    # battery 0x1004 idx6
    st, b = request(h, dn, 0x0600, b"")
    if st == "ok": print(f"  battery(0x1004) fn0: {b.hex()}")
    H.hid_close(h)
