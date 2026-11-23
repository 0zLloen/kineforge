#!/usr/bin/env python3
"""Reference reader for KFLOG v1, the KineForge raw session log.

Standard library only, Python >= 3.8. The byte-level format is described in
FORMAT.md (crates/kf-telemetry/FORMAT.md). The Rust reader in kf-telemetry is
authoritative; this reader classifies every chunk exactly as it does.

Library:
    records(f)              yield the canonical dump objects of a binary stream
    recovered(raw, sidecar) the recovered byte stream (prefix + synthetic end),
                            checked against `raw`; `raw` itself if it is complete

CLI: read_kflog.py [--jsonl] [--recovered SIDECAR] FILE
    --jsonl      print the canonical dump as JSON lines (kf_telemetry::dump)
    --recovered  read FILE through its raw.recovered.json sidecar
Without --jsonl it prints the number of dump objects per kind and the ending.
"""

import argparse
import base64
import hashlib
import io
import json
import math
import struct
import sys
import zlib

MAGIC = b"KFLOG\r\n\x1a"
FILE_HEADER_LEN = 16
MAX_PAYLOAD = 1 << 20
HEADER = struct.Struct("<IBBHII")  # payload_len kind kind_version flags record_count reserved
MAX_JSON_DEPTH = 127  # serde_json's nesting limit

JSON_KINDS = {
    0x00: "session_header",
    0x04: "source_declare",
    0x05: "device_info",
    0x06: "render_state",
    0x30: "app_result",
    0x7F: "session_end",
}
COMMIT, ANNOTATION, SESSION_END = 0x7C, 0x7E, 0x7F

# Fixed layouts: struct format (one char per field) and field names.
# "_" zero padding, "?" sentinel (type min/max) means null, "!" bool byte
# (0 or 1), "name.i" array element.
FIXED = {
    0x01: ("input_motion", "qqiiIHH", "t_ns src_ts_ns? dx dy seq source flags"),
    0x02: ("input_button", "qqIHBB", "t_ns src_ts_ns? seq source button state"),
    0x03: ("input_wheel", "qqiiIHH", "t_ns src_ts_ns? dv dh seq source flags"),
    0x10: ("frame", "QqqQIHH", "frame t_start_ns t_input_cutoff_ns first_tick ticks flags _"),
    0x11: ("drain", "QHHI", "frame source _ last_seq"),
    0x12: ("frame_timing", "QqqII", "frame t_submit_ns t_present_ns? cpu_ns gpu_ns?"),
    0x20: ("sim_camera", "Qqqdd", "tick yaw_units pitch_units yaw_rad pitch_rad"),
    0x21: (
        "sim_target",
        "QIBBHddddddd",
        "tick target event shape _ pos_m.0 pos_m.1 pos_m.2 vel_m_s.0 vel_m_s.1 vel_m_s.2 radius_m",
    ),
    0x22: (
        "sim_shot",
        "QqIIdddBBHI",
        "tick t_ns target? seq ang_err_rad d_yaw_rad d_pitch_rad hit! _ _ _",
    ),
    0x23: ("sim_control", "QqQBBHHH", "tick t_ns frame action phase reason new_source? _"),
    0x7D: ("clock_sync", "qHHIqq", "t_ns domain _ _ src_ts_ns uncertainty_ns"),
}
SENTINEL = {"q": -(1 << 63), "Q": (1 << 64) - 1, "I": (1 << 32) - 1, "H": 0xFFFF}


def _fixed(code, n, payload):
    name, fmt, fields = FIXED[code]
    st = struct.Struct("<" + fmt)
    if n * st.size != len(payload):
        raise ValueError("payload_len != record_count x size")
    out = []
    for vals in st.iter_unpack(payload):
        rec = {"kind": name}
        for f, c, v in zip(fields.split(), fmt, vals):
            if f == "_":
                if v:
                    raise ValueError("non-zero padding")
                continue
            if f.endswith("?"):
                f, v = f[:-1], (None if v == SENTINEL[c] else v)
            elif f.endswith("!"):
                if v > 1:
                    raise ValueError("bool byte is neither 0 nor 1")
                f, v = f[:-1], bool(v)
            if c == "d" and not math.isfinite(v):
                v = None  # serde_json writes non-finite floats as null
            if "." in f:
                rec.setdefault(f.partition(".")[0], []).append(v)
            else:
                rec[f] = v
        out.append(rec)
    return out


def _no_constant(s):
    raise ValueError("%s is not JSON" % s)


def _float(s):
    f = float(s)
    if math.isinf(f):
        raise ValueError("number out of range")
    return f


def _int(s):
    # As serde_json: -0 and integers outside i64/u64 become floats.
    if s != "-0" and len(s) <= 21 and -(1 << 63) <= int(s) < 1 << 64:
        return int(s)
    return _float(s)


def _depth(v):
    deepest, stack = 0, [(v, 1)]
    while stack:
        x, d = stack.pop()
        if isinstance(x, (dict, list)):
            deepest = max(deepest, d)
            stack.extend((y, d + 1) for y in (x.values() if isinstance(x, dict) else x))
    return deepest


def _json(n, payload):
    if n != 1:
        raise ValueError("record_count != 1")
    v = json.loads(
        payload.decode("utf-8"),
        parse_constant=_no_constant,
        parse_int=_int,
        parse_float=_float,
    )
    if not isinstance(v, dict) or _depth(v) > MAX_JSON_DEPTH:
        raise ValueError("not an object, or nested too deeply")
    json.dumps(v, ensure_ascii=False).encode("utf-8")  # lone surrogates raise
    return v


def _commit(n, p):
    if n != 1 or len(p) < 32:
        raise ValueError("commit needs record_count 1 and >= 32 bytes")
    t_ns, tick, frame, count, pad = struct.unpack_from("<qQQII", p)
    if pad or len(p) != 32 + 8 * count:
        raise ValueError("bad commit")
    sources = []
    for source, pad, last_seq in struct.iter_unpack("<HHI", p[32:]):
        if pad:
            raise ValueError("non-zero padding")
        sources.append([source, last_seq])
    none = SENTINEL["Q"]
    return {
        "kind": "commit",
        "t_ns": t_ns,
        "tick": None if tick == none else tick,
        "frame": None if frame == none else frame,
        "sources": sources,
    }


def _annotations(n, p):
    out, i = [], 0
    while i < len(p):
        if len(p) - i < 24:
            raise ValueError("truncated annotation header")
        t_ns, code, source, first_seq, lost, size = struct.unpack_from("<qHHIII", p, i)
        i += 24
        padded = (size + 7) // 8 * 8
        if padded > len(p) - i:
            raise ValueError("annotation text runs past the payload")
        text = p[i : i + size].decode("utf-8")
        if any(p[i + size : i + padded]):
            raise ValueError("non-zero padding")
        i += padded
        out.append(
            {
                "kind": "annotation",
                "t_ns": t_ns,
                "code": code,
                "source": None if source == 0xFFFF else source,
                "first_seq": first_seq,
                "lost": None if lost == SENTINEL["I"] else lost,
                "text": text,
            }
        )
    if len(out) != n:
        raise ValueError("annotation count != record_count")
    return out


def _decode(offset, kind, version, flags, n, payload):
    """(decoded, dump objects) for one CRC-valid chunk."""
    known = kind in JSON_KINDS or kind in FIXED or kind in (COMMIT, ANNOTATION)
    if not known or version != 1 or flags != 0:
        return False, [
            {
                "kind": "skipped",
                "offset": offset,
                "kind_code": kind,
                "kind_version": version,
                "flags": flags,
                "record_count": n,
                "payload_len": len(payload),
            }
        ]
    try:
        if kind in JSON_KINDS:
            return True, [{"kind": JSON_KINDS[kind], "value": _json(n, payload)}]
        if kind == COMMIT:
            return True, [_commit(n, payload)]
        if kind == ANNOTATION:
            return True, _annotations(n, payload)
        return True, _fixed(kind, n, payload)
    except (ValueError, RecursionError):
        return False, [{"kind": "undecodable", "offset": offset, "kind_code": kind}]


def _fill(f, n):
    """Read until n bytes or end of input."""
    buf = b""
    while len(buf) < n:
        b = f.read(n - len(buf))
        if not b:
            break
        buf += b
    return buf


def _header_stop(h):
    """Why the 16-byte file header is unusable, or None."""
    if len(h) < FILE_HEADER_LEN:
        h += bytes(FILE_HEADER_LEN - len(h))
        if h[:8] == MAGIC or not any(h):
            return {"reason": "truncated", "offset": 0}
        return {"reason": "bad_magic", "offset": None}
    if h[:8] != MAGIC:
        return {"reason": "bad_magic", "offset": None}
    if h[8:12] != b"\x01\0\0\0":
        return {"reason": "unsupported_version", "offset": None}
    if any(h[12:16]):
        return {"reason": "non_zero_reserved", "offset": 12}
    return None


def _chunks(f):
    """Yield (offset, kind, kind_version, flags, record_count, payload) for
    each CRC-valid chunk; a final dict says why reading stopped early."""
    offset = FILE_HEADER_LEN
    try:
        while True:
            h = _fill(f, 16)
            if not h:
                return
            stop = "truncated"
            if len(h) == 16:
                size, kind, version, flags, n, reserved = HEADER.unpack(h)
                if size > MAX_PAYLOAD:  # bounded before reading
                    stop = "payload_too_large"
                elif size % 8:
                    stop = "payload_misaligned"
                else:
                    payload = _fill(f, size)
                    trailer = _fill(f, 8)
                    if len(payload) < size or len(trailer) < 8:
                        stop = "truncated"
                    elif zlib.crc32(h + payload) != int.from_bytes(trailer[:4], "little"):
                        stop = "crc_mismatch"
                    elif any(trailer[4:]):
                        stop = "non_zero_trailer"
                    elif reserved:
                        stop = "non_zero_reserved"
                    else:
                        stop = None
            if stop:
                yield {"reason": stop, "offset": offset}
                return
            yield offset, kind, version, flags, n, payload
            offset += 24 + size
    except OSError:
        yield {"reason": "io", "offset": None}


def records(f):
    """Yield the canonical dump objects of a KFLOG v1 stream `f` (anything
    with read(n) -> bytes): the committed records in file order, with
    "skipped" and "undecodable" chunk markers, each group's "commit", the
    terminal "session_end", then one "ending" object. Records after the last
    commit are never yielded; the ending counts them."""
    try:
        stop = _header_stop(_fill(f, FILE_HEADER_LEN))
    except OSError:
        stop = {"reason": "io", "offset": None}
    if stop:
        yield {"kind": "ending", "complete": False, "stop": stop}
        return
    committed_end = valid_end = FILE_HEADER_LEN
    pending, session_end, after_end, stop = [], None, 0, None
    for c in _chunks(f):
        if isinstance(c, dict):
            stop = c
            break
        offset, kind, version, flags, n, payload = c
        valid_end = offset + 24 + len(payload)
        if session_end is not None:
            after_end += 1
            continue
        # Pending chunks stay raw (1x their size) until their commit arrives.
        if kind in (COMMIT, SESSION_END):
            decoded, objs = _decode(*c)
            if decoded and kind == COMMIT:
                for p in pending:
                    yield from _decode(*p)[1]
                yield objs[0]
                pending, committed_end = [], valid_end
                continue
            if decoded and not pending and committed_end > FILE_HEADER_LEN:
                session_end = objs[0]["value"]
                continue
        pending.append(c)
    if session_end is not None:
        yield {"kind": "session_end", "value": session_end}
    yield {
        "kind": "ending",
        "complete": session_end is not None,
        "committed_end": committed_end,
        "valid_end": valid_end,
        "uncommitted_chunks": len(pending),
        "after_end_chunks": after_end,
        "stop": stop,
    }


class _Recovered:
    """The first `left` bytes of `raw`, then `end`."""

    def __init__(self, raw, left, end):
        self.raw, self.left, self.end = raw, left, end

    def read(self, n):
        if self.left:
            b = self.raw.read(min(n, self.left))
            self.left = self.left - len(b) if b else 0
            if b:
                return b
        b, self.end = self.end[:n], self.end[n:]
        return b


def recovered(raw, sidecar):
    """The recovered byte stream of a seekable raw log given its parsed
    raw.recovered.json: the first valid_prefix_bytes, then the synthetic end
    chunk. A complete `raw` is returned as is (FORMAT.md §8: the sidecar is
    ignored). ValueError if the sidecar is invalid or does not match `raw`."""
    ok = (
        isinstance(sidecar, dict)
        and all(
            type(sidecar.get(k)) is int and 0 <= sidecar[k] < 1 << 64
            for k in ("valid_prefix_bytes", "tail_bytes_ignored")
        )
        and all(isinstance(sidecar.get(k), str) for k in ("synthetic_end_b64", "recovered_sha256"))
    )
    if not ok:
        raise ValueError("invalid raw.recovered.json")
    b64 = sidecar["synthetic_end_b64"]
    end = base64.b64decode(b64, validate=True)
    if base64.b64encode(end).decode("ascii") != b64:
        raise ValueError("synthetic_end_b64 is not canonical base64")
    *_, ending = records(raw)
    size = raw.seek(0, io.SEEK_END)
    raw.seek(0)
    if ending["complete"]:
        return raw
    prefix = sidecar["valid_prefix_bytes"]
    h, r = hashlib.sha256(), _Recovered(raw, prefix, end)
    for b in iter(lambda: r.read(65536), b""):
        h.update(b)
    raw.seek(0)
    if (
        ending.get("committed_end", 0) <= FILE_HEADER_LEN
        or prefix != ending["committed_end"]
        or prefix + sidecar["tail_bytes_ignored"] != size
        or h.hexdigest() != sidecar["recovered_sha256"]
    ):
        raise ValueError("raw.recovered.json does not match the raw log (run recovery again)")
    return _Recovered(raw, prefix, end)


def dumps(obj):
    """One canonical dump line (serde_json's key order and separators)."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)


def main(argv=None):
    p = argparse.ArgumentParser(description="Read a KFLOG v1 raw session log (FORMAT.md).")
    p.add_argument("--jsonl", action="store_true", help="print the canonical dump as JSON lines")
    p.add_argument("--recovered", metavar="SIDECAR", help="read FILE through its raw.recovered.json")
    p.add_argument("file", metavar="FILE")
    a = p.parse_args(argv)
    out = sys.stdout.buffer
    try:
        with open(a.file, "rb") as f:
            if a.recovered:
                with open(a.recovered, "rb") as s:
                    r = recovered(f, json.loads(s.read().decode("utf-8")))
                if r is f:
                    print("read_kflog.py: %s is complete; sidecar ignored" % a.file, file=sys.stderr)
                f = r
            counts = {}
            for obj in records(f):
                if a.jsonl:
                    out.write((dumps(obj) + "\n").encode("utf-8"))
                elif obj["kind"] == "ending":
                    for kind in sorted(counts):
                        out.write(("%s: %d\n" % (kind, counts[kind])).encode())
                    out.write((dumps(obj) + "\n").encode("utf-8"))
                else:
                    counts[obj["kind"]] = counts.get(obj["kind"], 0) + 1
    except (OSError, ValueError, RecursionError) as e:
        sys.exit("read_kflog.py: %s" % e)


if __name__ == "__main__":
    main()
