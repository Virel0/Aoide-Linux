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
