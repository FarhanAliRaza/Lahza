"""Packs Natural Earth 1:50m (public domain, naturalearthdata.com) for the
`map` component: countries (code, name, label point, rings), lakes and
populated places, as crates/motion-core/data/ne50m.bin.

    python3 build_map_data.py COUNTRIES.geojson LAKES.geojson PLACES.geojson OUT.bin

Inputs: ne_50m_admin_0_countries, ne_50m_lakes and
ne_50m_populated_places_simple GeoJSON (github.com/nvkelso/natural-earth-vector).

Format (little-endian): b"LZNE", u32 version 1, then three sections.
- Countries: u32 count; each has a str code (ISO A3, else ADM0_A3), a str
  name, f32 label lon and lat, and rings.
- Lakes: u32 count; each has a str name and rings.
- Places: u32 count; each has a str name, a str country code, f32 lon and
  lat, and a u8 rank (Natural Earth scalerank, 0 = most important).

A str is a u16 length followed by UTF-8. Rings are a u32 count; each ring
is a u32 point count, i32 lon and lat of the first point in units of
1e-3 degrees, then i16 deltas. A delta of -32768 escapes to a full i32
pair.
"""
import json
import struct
import sys


def s(text):
    b = (text or "").encode()
    return struct.pack("<H", len(b)) + b


def rings(geom):
    polys = geom["coordinates"] if geom["type"] == "MultiPolygon" else [geom["coordinates"]]
    out = b""
    all_rings = [r for p in polys for r in p]
    out += struct.pack("<I", len(all_rings))
    for r in all_rings:
        pts = [(round(x * 1000), round(y * 1000)) for x, y in r]
        # Drop repeats after quantising.
        dedup = [pts[0]]
        for p in pts[1:]:
            if p != dedup[-1]:
                dedup.append(p)
        out += struct.pack("<I", len(dedup))
        out += struct.pack("<ii", *dedup[0])
        for (x0, y0), (x1, y1) in zip(dedup, dedup[1:]):
            dx, dy = x1 - x0, y1 - y0
            if -32767 <= dx <= 32767 and -32767 <= dy <= 32767:
                out += struct.pack("<hh", dx, dy)
            else:
                out += struct.pack("<hhii", -32768, 0, x1, y1)
    return out


def code(p):
    for k in ("ISO_A3_EH", "ISO_A3", "ADM0_A3"):
        v = p.get(k)
        if v and v != "-99":
            return v
    return p["ADM0_A3"]


def main():
    countries, lakes, places, out = sys.argv[1:5]
    c = json.load(open(countries))["features"]
    lk = json.load(open(lakes))["features"]
    pl = json.load(open(places))["features"]
    data = b"LZNE" + struct.pack("<I", 1)
    data += struct.pack("<I", len(c))
    for f in sorted(c, key=lambda f: code(f["properties"])):
        p = f["properties"]
        data += s(code(p)) + s(p["NAME"]) + struct.pack("<ff", p["LABEL_X"], p["LABEL_Y"]) + rings(f["geometry"])
    data += struct.pack("<I", len(lk))
    for f in lk:
        data += s(f["properties"].get("name") or "") + rings(f["geometry"])
    data += struct.pack("<I", len(pl))
    for f in sorted(pl, key=lambda f: f["properties"]["scalerank"]):
        p = f["properties"]
        data += s(p["name"]) + s(p.get("adm0_a3") or "") + struct.pack("<ffB", p["longitude"], p["latitude"], min(255, int(p["scalerank"])))
    open(out, "wb").write(data)
    print(f"{out}: {len(data) / 1e6:.2f} MB, {len(c)} countries, {len(lk)} lakes, {len(pl)} places")


if __name__ == "__main__":
    main()
