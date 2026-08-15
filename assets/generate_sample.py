"""Regenerate assets/sample.nmea.

A synthetic drive shaped like real receiver output. Kept in the repo so the
fixture is reproducible and its intent is legible: 222 KB of NMEA is otherwise
opaque, and every oddity below is deliberate.

The route is around Greenwich and crosses the prime meridian, so the longitude
hemisphere flips mid-track. That is a test property, and it makes plain that
nothing here is a real person's movements.

Shapes taken from real multi-constellation receiver output:

- timestamps that drift off the whole second (gaps of 0.97, 1.00 and 1.03 s).
  A fixture that ticks on whole seconds divides exactly by any replay rate and
  hides arithmetic that does not terminate, which is precisely how a replay
  freeze survived 40 tests
- five GSA sentences per epoch, one per constellation, all under the GN talker,
  several carrying no PRNs at all
- a QZSS GSV group reporting nothing in view
- satellites in view but untracked, reported with no SNR
- NMEA 4.11 signal-ID fields
- empty speed and course fields while stationary
- a dead-reckoned fix (GGA quality 6) at power-on and a DGPS stretch
- a 2D-only stretch where the constellation thins out
- GPTXT, GNZDA and GNGLL alongside the positioning sentences
- damaged sentences in sequence: a bad checksum, a corrupted RMC and an
  unsupported type, so the rejection paths have something to reject

Run from the repository root:  python3 assets/generate_sample.py
"""
import math, random
random.seed(20260815)

def cs(body):
    c = 0
    for ch in body: c ^= ord(ch)
    return f"${body}*{c:02X}"

def dm(val, is_lat):
    hemi = ('N' if val >= 0 else 'S') if is_lat else ('E' if val >= 0 else 'W')
    v = abs(val); deg = int(v); minutes = (v - deg) * 60
    return f"{deg:0{2 if is_lat else 3}d}{minutes:08.5f}", hemi

# Deliberately synthetic: a loop across the prime meridian at Greenwich, so the
# longitude sign flips mid-track and nothing here resembles a real person's route.
lat, lon, alt = 51.47790, 0.00420, 46.0
heading = 262.0

# Constellations: (talker, [(prn, elev, azim)], system_id)
GP = [(3,18,210),(4,80,276),(7,22,284),(8,17,171),(9,45,309),(16,71,59),(26,38,52),(27,36,137),(31,22,83)]
GL = [(70,43,170),(71,65,305),(73,65,290),(74,22,247),(80,31,118)]
GA = [(5,40,95),(13,62,201),(15,28,318)]
BD = [(14,56,274),(21,59,222),(22,21,170),(26,43,262),(29,14,106),(36,21,62)]

def gsv(talker, sats, sig):
    """NMEA 4.11 GSV: four satellites per sentence, signal ID last."""
    out, total = [], max(1, (len(sats) + 3) // 4)
    if not sats:
        return [cs(f"{talker}GSV,1,1,00,{sig}")]
    for i in range(total):
        body = f"{talker}GSV,{total},{i+1},{len(sats):02d}"
        for prn, el, az in sats[i*4:(i+1)*4]:
            # A satellite low on the horizon is often in view but not tracked,
            # which a receiver reports as a present satellite with no SNR at all.
            tracked = el > 20 or random.random() > 0.5
            snr = max(0, min(50, int(22 + el * 0.35 + random.uniform(-5, 5))))
            body += f",{prn:02d},{el:02d},{az:03d}," + (f"{snr:02d}" if tracked else "")
        out.append(cs(f"{body},{sig}"))
    return out

lines = []
t = 0.0            # seconds since midnight
EPOCHS = 300
for e in range(EPOCHS):
    # A real receiver's epochs drift off the whole second and correct back; those
    # fractional gaps are what a whole-second fixture never exercises.
    if e and e % 5 == 0:
        t += 0.97
    elif e and e % 5 == 1:
        t += 1.03
    elif e:
        t += 1.0

    h, m, s = int(t // 3600), int(t // 60) % 60, t % 60
    stamp = f"{h:02d}{m:02d}{s:05.2f}"

    # Speed profile: stationary, pull away, cruise with a stop, resume.
    if e < 20:     speed = 0.0
    elif e < 70:   speed = (e - 20) * 0.55
    elif e < 170:  speed = 27.0 + 5.0 * math.sin(e / 25.0)
    elif e < 185:  speed = max(0.0, 27.0 - (e - 170) * 1.9)
    elif e < 200:  speed = 0.0
    else:          speed = min(31.0, (e - 200) * 0.9)

    moving = speed > 0.3
    if moving:
        heading = (heading + (0.8 if 90 < e < 150 else 0.06) + random.uniform(-0.3, 0.3)) % 360
        mps = speed * 0.514444
        lat += (mps * math.cos(math.radians(heading))) / 111_320.0
        lon += (mps * math.sin(math.radians(heading))) / (111_320.0 * math.cos(math.radians(lat)))
        alt += math.sin(e / 40.0) * 0.4 + random.uniform(-0.2, 0.2)

    hdop = round(0.6 + 0.6 * abs(math.sin(e / 55.0)) + random.uniform(0, 0.2), 1)
    pdop, vdop = round(hdop + 0.7, 1), round(hdop + 0.4, 1)

    # Fix quality: one dead-reckoned epoch at power-on, a DGPS stretch, else GPS.
    quality = 6 if e == 0 else (2 if 200 <= e < 260 else 1)
    # A brief 2D-only stretch, as happens when the constellation thins out.
    mode2 = 2 if 150 <= e < 165 else 3

    used_gp = GP[:4] if mode2 == 2 else GP[:7 + (e % 3)]
    used_gl = [] if mode2 == 2 else GL[:3]
    used_ga = GA[:2] if mode2 == 3 else []
    used_bd = BD[:4] if mode2 == 3 else BD[:2]
    sats_used = len(used_gp) + len(used_gl) + len(used_ga) + len(used_bd)

    lat_s, ns = dm(lat, True); lon_s, ew = dm(lon, False)
    date = "150826"

    # Stationary receivers routinely leave course, and sometimes speed, empty.
    sog = "" if not moving and e % 3 == 0 else f"{speed:.2f}"
    cog = "" if not moving else f"{heading:.2f}"

    lines.append(cs(f"GNRMC,{stamp},A,{lat_s},{ns},{lon_s},{ew},{sog},{cog},{date},,,A,V"))
    if moving:
        lines.append(cs(f"GNVTG,{heading:.2f},T,,M,{speed:.2f},N,{speed*1.852:.2f},K,A"))
    else:
        lines.append(cs(f"GNVTG,,,,,{speed:.2f},N,{speed*1.852:.2f},K,A"))
    lines.append(cs(f"GNGGA,{stamp},{lat_s},{ns},{lon_s},{ew},{quality},{sats_used:02d},{hdop},{alt:.2f},M,45.30,M,,"))

    # One GSA per constellation, all under the GN talker, several with no PRNs —
    # exactly the shape that broke used-satellite tracking once already.
    for sysid, used in ((1, used_gp), (2, used_gl), (3, used_ga), (4, used_bd), (5, [])):
        prns = [f"{p:02d}" for p, _, _ in used] + [""] * (12 - len(used))
        lines.append(cs(f"GNGSA,A,{mode2},{','.join(prns)},{pdop},{hdop},{vdop},{sysid}"))

    if e % 2 == 0:
        lines += gsv("GP", GP, 1)
        lines += gsv("GL", GL, 1)
        lines += gsv("GA", GA, 7)
        lines += gsv("BD", BD, 1)
        lines += gsv("GQ", [], 1)          # in view of nothing, but still reported

    # Damaged sentences, so the rejection paths have something to reject; a real
    # log's corruption sits in sequence rather than teleporting the clock.
    if e == 30:
        lines.append(f"$GNGGA,{stamp},{lat_s},{ns},{lon_s},{ew},1,09,0.9,46.0,M,45.3,M,,*00")
    if e == 90:
        good = cs(f"GNRMC,{stamp},A,{lat_s},{ns},{lon_s},{ew},{speed:.2f},{heading:.2f},{date},,,A,V")
        lines.append(good[:-2] + "7F")          # right sentence, wrong checksum
    if e == 140:
        lines.append(cs("GNZZZ,not,a,real,sentence"))

    lines.append(cs(f"GNZDA,{stamp},15,08,2026,00,00"))
    lines.append(cs(f"GNGLL,{lat_s},{ns},{lon_s},{ew},{stamp},A,A"))
    if e % 30 == 0:
        lines.append(cs("GPTXT,01,01,02,ANTENNA OK"))

open("assets/sample.nmea", "w").write("\r\n".join(lines) + "\r\n")
print(f"{len(lines)} sentences, {EPOCHS} epochs")
