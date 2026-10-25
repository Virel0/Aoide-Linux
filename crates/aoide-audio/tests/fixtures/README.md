# Audio fixtures

Small encoded files for `tests/formats.rs`, made once and committed so the
tests need no encoder. Every file is a generated signal (a ramp or clicks),
48 kHz, stereo, 16-bit before encoding. Nothing recorded.

Made on macOS 26.2 with `afconvert` (part of macOS), ffmpeg 9.0.2 (Homebrew,
libmp3lame from LAME 4.0) and `oggenc` from vorbis-tools 1.4.3 (libVorbis
1.3.7).

## The source WAVs

`gen.py`, run with `python3 gen.py`, writes the four sources:

```python
import struct, wave

RATE, A, B = 48000, 48123, 30007

def ramp(start, frames):
    # tests/common: ramp(start, frames, 2)
    out = []
    for k in range(frames):
        v = (start + k) % 20000 - 10000
        out += [v, -v]
    return out

def clicks(frames):
    # One-sample clicks at half scale every 4001 frames from frame 1000, and
    # one 500 frames before the end; left positive, right negative.
    at = set(range(1000, frames - 600, 4001)) | {frames - 500}
    out = []
    for k in range(frames):
        v = 16384 if k in at else 0
        out += [v, -v]
    return out

def write(name, samples):
    with wave.open(name, "wb") as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(RATE)
        w.writeframes(struct.pack("<%dh" % len(samples), *samples))

write("ramp_a.wav", ramp(0, A))
write("ramp_b.wav", ramp(A, B))
write("clicks_a.wav", clicks(A))
write("clicks_b.wav", clicks(B))
```

`b` carries on the ramp exactly where `a` stops, so the two ALAC files join
into one unbroken ramp.

## The encodes

For `x` in `a` and `b`:

| File | Command |
|---|---|
| `alac_x.m4a` | `afconvert -f m4af -d alac ramp_x.wav alac_x.m4a` |
| `aac_x.m4a` | `afconvert -f m4af -d aac -b 128000 clicks_x.wav aac_x.m4a` |
| `adts_x.aac` | `ffmpeg -i clicks_x.wav -c:a aac -b:a 128k -f adts adts_x.aac` |
| `mp3_x.mp3` | `ffmpeg -i clicks_x.wav -c:a libmp3lame -b:a 128k mp3_x.mp3` |
| `vorbis_x.ogg` | `oggenc -Q -q 4 -o vorbis_x.ogg clicks_x.wav` |

`adts_x.aac` uses ffmpeg's own AAC encoder because that is what Jellyfin
transcodes with.

`opus.opus`, the first quarter second of the clicks, is only there to fail
(symphonia has no Opus decoder); libopus 1.6.1:

```
ffmpeg -t 0.25 -i clicks_a.wav -c:a libopus -b:a 64k opus.opus
```

## ReplayGain fixtures

For `tests/replaygain.rs`: half a second of a 1 kHz tone at half scale
(−6 dBFS, peak exactly 0.5), 48 kHz stereo, tagged. Made on macOS 26.2 with
ffmpeg 9.0.2, metaflac 1.5.0 and `oggenc` from vorbis-tools 1.4.3.

`gen_rg.py` writes the source, `tone.wav`:

```python
import math, struct, wave

RATE, FRAMES = 48000, 24000

def tone():
    # 1 kHz at half scale (-6 dBFS), both channels, half a second.
    out = []
    for k in range(FRAMES):
        v = round(16384 * math.sin(2 * math.pi * 1000 * k / RATE))
        out += [v, v]
    return out

with wave.open("tone.wav", "wb") as w:
    w.setnchannels(2); w.setsampwidth(2); w.setframerate(RATE)
    s = tone()
    w.writeframes(struct.pack("<%dh" % len(s), *s))
```

| File | Tags | Command |
|---|---|---|
| `rg_album.flac` | track −4 dB / 0.5, album −7 dB / 0.6 | `ffmpeg -i tone.wav -c:a flac rg_album.flac`, then `metaflac --remove-all-tags --set-tag="REPLAYGAIN_TRACK_GAIN=-4.00 dB" --set-tag="REPLAYGAIN_TRACK_PEAK=0.500000" --set-tag="REPLAYGAIN_ALBUM_GAIN=-7.00 dB" --set-tag="REPLAYGAIN_ALBUM_PEAK=0.600000" rg_album.flac` |
| `rg_boost.flac` | track +9 dB / 0.5 | as above, with `--set-tag="REPLAYGAIN_TRACK_GAIN=+9.00 dB" --set-tag="REPLAYGAIN_TRACK_PEAK=0.500000"` |
| `rg_track.ogg` | track −3 dB, no peak | `oggenc -Q -q 4 -c "REPLAYGAIN_TRACK_GAIN=-3.00 dB" -o rg_track.ogg tone.wav` |
| `rg_album.mp3` | as `rg_album.flac`, ID3v2.4 `TXXX` | `ffmpeg -i tone.wav -c:a libmp3lame -b:a 128k -id3v2_version 4 -metadata "REPLAYGAIN_TRACK_GAIN=-4.00 dB" -metadata "REPLAYGAIN_TRACK_PEAK=0.500000" -metadata "REPLAYGAIN_ALBUM_GAIN=-7.00 dB" -metadata "REPLAYGAIN_ALBUM_PEAK=0.600000" rg_album.mp3` |
| `rg_album.m4a` | track −4 dB, album −7 dB / 0.6, iTunes `----` atoms | `ffmpeg -i tone.wav -c:a alac rg_album.m4a`, then `python3 itunes_rg.py rg_album.m4a "replaygain_track_gain=-4.00 dB" "replaygain_album_gain=-7.00 dB" "replaygain_album_peak=0.600000"` |

ffmpeg's MP4 muxer drops tags it does not know, so `itunes_rg.py` adds the
freeform atoms itself, inside the `ilst` that ffmpeg writes. It relies on
`moov` being the last atom (ffmpeg's default without `+faststart`), so no
chunk offset moves:

```python
# Adds iTunes freeform (----) ReplayGain atoms to an M4A whose moov is the
# last top-level atom (what ffmpeg writes), so no chunk offset moves.
import struct, sys

def atom(kind, payload):
    return struct.pack(">I4s", 8 + len(payload), kind) + payload

def freeform(name, value):
    return atom(b"----",
        atom(b"mean", b"\0\0\0\0" + b"com.apple.iTunes")
        + atom(b"name", b"\0\0\0\0" + name.encode())
        + atom(b"data", struct.pack(">II", 1, 0) + value.encode()))

def children(data, start, end):
    at = start
    while at < end:
        size, kind = struct.unpack(">I4s", data[at:at + 8])
        yield at, size, kind
        at += size

path, pairs = sys.argv[1], sys.argv[2:]
data = bytearray(open(path, "rb").read())
moov = [(a, s) for a, s, k in children(data, 0, len(data)) if k == b"moov"][0]
assert moov[0] + moov[1] == len(data), "moov is not last"
udta = [(a, s) for a, s, k in children(data, moov[0] + 8, moov[0] + moov[1]) if k == b"udta"][0]
meta = [(a, s) for a, s, k in children(data, udta[0] + 8, udta[0] + udta[1]) if k == b"meta"][0]
ilst = [(a, s) for a, s, k in children(data, meta[0] + 12, meta[0] + meta[1]) if k == b"ilst"][0]
extra = b"".join(freeform(n, v) for n, v in (p.split("=", 1) for p in pairs))
end = ilst[0] + ilst[1]
data[end:end] = extra
for at, size in (moov, udta, meta, ilst):
    struct.pack_into(">I", data, at, size + len(extra))
open(path, "wb").write(data)
```
