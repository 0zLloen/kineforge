#!/bin/sh
# Python reference reader parity (ROADMAP M4): tools/read_kflog.py --jsonl
# must equal the committed Rust dumps of the golden logs (tests/golden.rs
# proves those equal kf_telemetry::dump on every target), the recovered torn
# log must end complete with the sidecar's identity hash, and FORMAT.md must
# list the reader's SHA-256. Values are compared as parsed JSON (number
# formatting differs between serde_json and Python; ints and floats are
# compared exactly and never equal each other).
set -eu
cd "$(dirname "$0")/.."
exec python3 - <<'EOF'
import glob, hashlib, json, re, subprocess, sys

sys.path.insert(0, "tools")
import read_kflog

G = "crates/kf-telemetry/tests/golden"


def parse(line):
    return json.loads(line, parse_int=lambda s: ("int", int(s)),
                      parse_float=lambda s: ("float", repr(float(s))))


def dump(*args):
    r = subprocess.run([sys.executable, "tools/read_kflog.py", "--jsonl", *args],
                       stdout=subprocess.PIPE, check=True)
    return lines(r.stdout.decode("utf-8"))


def lines(text):
    # Not splitlines(): both dumps write U+2028 and friends unescaped.
    return text.split("\n")[:-1] if text.endswith("\n") else text.split("\n")


def fail(msg):
    print("python-parity: FAIL: " + msg)
    sys.exit(1)


logs = sorted(glob.glob(G + "/*.kflog"))
if not logs:
    fail("no golden logs in " + G)
for log in logs:
    want = lines(open(log[: -len(".kflog")] + ".jsonl", encoding="utf-8", newline="").read())
    got = dump(log)
    for i in range(max(len(want), len(got))):
        w = want[i] if i < len(want) else "<missing>"
        g = got[i] if i < len(got) else "<missing>"
        if "<missing>" in (w, g) or parse(w) != parse(g):
            fail("%s line %d\n  rust:   %s\n  python: %s" % (log, i + 1, w, g))
    print("ok %s (%d lines)" % (log, len(want)))

sidecar_path = G + "/torn.recovered.json"
sidecar = json.load(open(sidecar_path, encoding="utf-8"))
end = json.loads(dump("--recovered", sidecar_path, G + "/torn.kflog")[-1])
if not (end["kind"] == "ending" and end["complete"] and end["stop"] is None
        and end["uncommitted_chunks"] == 0 and end["after_end_chunks"] == 0):
    fail("recovered torn.kflog does not end complete: %s" % end)
with open(G + "/torn.kflog", "rb") as raw:
    r = read_kflog.recovered(raw, sidecar)
    stream = b"".join(iter(lambda: r.read(65536), b""))
if hashlib.sha256(stream).hexdigest() != sidecar["recovered_sha256"]:
    fail("recovered stream SHA-256 differs from torn.recovered.json")
print("ok recovered torn.kflog (complete, identity hash matches)")

# FORMAT.md §8: a sidecar next to a complete log is ignored, and one that
# does not match its log is refused (kf_store::session_stream does the same).
if dump("--recovered", sidecar_path, G + "/full.kflog") != dump(G + "/full.kflog"):
    fail("a stale sidecar changed the dump of the complete full.kflog")
print("ok stale sidecar ignored for complete full.kflog")
for k, d in (("valid_prefix_bytes", -8), ("tail_bytes_ignored", 8)):
    with open(G + "/torn.kflog", "rb") as raw:
        try:
            read_kflog.recovered(raw, dict(sidecar, **{k: sidecar[k] + d}))
        except ValueError:
            continue
    fail("a sidecar with a wrong %s was accepted" % k)
print("ok mismatched sidecars refused")

listed = re.findall(r"read_kflog\.py SHA-256: ([0-9a-f]{64})",
                    open("crates/kf-telemetry/FORMAT.md", encoding="utf-8").read())
actual = hashlib.sha256(open("tools/read_kflog.py", "rb").read()).hexdigest()
if listed != [actual]:
    fail("FORMAT.md lists read_kflog.py SHA-256 %s, actual %s" % (listed, actual))
print("ok FORMAT.md lists read_kflog.py SHA-256 " + actual)
EOF
