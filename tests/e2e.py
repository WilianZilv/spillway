#!/usr/bin/env python3
"""End-to-end scenarios for spillway. Runs as root on Linux.

Lab: three ext4 loop filesystems (d1 4 GB, d2 2 GB, d3 1 GB), a
master mounted at /root/spw/videos with its tmpfs spool, and — for the remote
scenarios — a standalone node process serving d3 over HTTP.

Every scenario asserts on what the filesystem shows (through the mount and on
the raw destination disks) and on the master's API. Usage:

    python3 tests/e2e.py              # all scenarios
    python3 tests/e2e.py basic thief  # selected scenarios
"""
import hashlib
import json
import os
import random
import shutil
import signal
import subprocess
import sys
import threading
import time
import traceback
import urllib.error
import urllib.request

LAB = "/root/spw"
HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("SPILLWAY_BIN", os.path.join(os.path.dirname(HERE), "target", "release", "spillway"))
MNT = f"{LAB}/videos"
SPOOL = f"{LAB}/ram"
API = "http://127.0.0.1:7070"
NODE_API = "http://127.0.0.1:7071"
TOKEN = "lab-token"
FIX = f"{LAB}/fixtures"
DESTS = {"d1": 4000, "d2": 2000, "d3": 1000}

master_proc = None
node_proc = None
hook_proc = None
results = []
HOOK = "http://127.0.0.1:7090"
OUTBOX = f"{LAB}/outbox"
FAULT_NAME = "spillway-fault-panic.mp4"  # a lookup of this name panics inside the master (fault injection)


# ----------------------------------------------------------------- helpers

def sh(cmd, check=True):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise RuntimeError(f"{cmd}: {r.stderr.strip()}")
    return r.stdout.strip()


def api(path, method="GET", body=None, base=API, token=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    if data is not None:
        req.add_header("content-type", "application/json")
    if token:
        req.add_header("authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            t = r.read()
            if not t:
                return None
            try:
                return json.loads(t)
            except ValueError:
                return t.decode()
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"{method} {path} -> {e.code}: {e.read().decode()}") from None


def ov():
    return api("/api/overview")


def dest(ovr, did):
    return next(d for d in ovr["dests"] if d["id"] == did)


def wait(cond, timeout, what, every=0.2):
    t0 = time.time()
    last = None
    while time.time() - t0 < timeout:
        try:
            last = cond()
            if last:
                return last
        except Exception as e:  # noqa: BLE001
            last = e
        time.sleep(every)
    raise AssertionError(f"timeout ({timeout}s) waiting for: {what} (last={last!r})")


def check(cond, msg):
    if not cond:
        raise AssertionError(msg)


def md5(b):
    return hashlib.md5(b).hexdigest()


FIXTURES = {}


def fixture(name):
    if name not in FIXTURES:
        FIXTURES[name] = open(f"{FIX}/{name}", "rb").read()
    return FIXTURES[name]


def rec_path(cam, i, sub=False):
    t = time.gmtime(1792600000 + i * 10)
    return f"{cam}{'-sub' if sub else ''}/{time.strftime('%Y-%m-%d', t)}/{time.strftime('%H-%M-%S', t)}-{i:06d}.mp4"


def write(rel, data, chunk=65536, delay=0.0):
    p = os.path.join(MNT, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        for off in range(0, len(data), chunk):
            f.write(data[off:off + chunk])
            if delay:
                f.flush()
                time.sleep(delay)


def where(rel):
    """Destinations (raw disks) holding rel."""
    return [d for d in DESTS if os.path.exists(f"{LAB}/{d}/{rel}")]


def all_moved(n_files=None):
    o = ov()
    r = o["ram"]
    ok = r["files"] == 0
    if n_files is not None:
        ok = ok and o["totals"]["moved_files"] >= n_files
    return ok


def cam():
    return "%08x-%04x-4%03x-8%03x-%012x" % (random.getrandbits(32), random.getrandbits(16), random.getrandbits(12), random.getrandbits(12), random.getrandbits(48))


# ----------------------------------------------------------------- lab control

CONFIG = """
[mount]
path = "{mnt}"
threads = 16

[ram]
spool_dir = "{spool}"
thief_max_gb = {thief}
thief_resume_pct = 70.0
hard_limit_pct = 120.0
settle_secs = 1.0

[mover]
fail_streak = 3
avoid_secs = 10.0
shutdown_flush_secs = 20.0

[web]
listen = "127.0.0.1:7070"

[hooks.moved]
outbox_dir = "{lab}/outbox"

[cluster]
token = "{token}"
node_timeout_secs = 4.0

[[destination]]
id = "d1"
path = "{lab}/d1"
expect_mount = true

[[destination]]
id = "d2"
path = "{lab}/d2"
expect_mount = true
"""


def mount_dests():
    for d, size in DESTS.items():
        os.makedirs(f"{LAB}/{d}", exist_ok=True)
        if sh(f"mountpoint -q {LAB}/{d} && echo y || echo n") == "n":
            img = f"{LAB}/{d}.img"
            if not os.path.exists(img):
                sh(f"truncate -s {size}M {img} && mkfs.ext4 -q -F {img}")
            sh(f"mount -o loop {img} {LAB}/{d}")
        sh(f"mount -o remount,rw {LAB}/{d}", check=False)


def wipe_dests():
    for d in DESTS:
        for e in os.listdir(f"{LAB}/{d}"):
            if e != "lost+found":
                p = f"{LAB}/{d}/{e}"
                shutil.rmtree(p) if os.path.isdir(p) else os.remove(p)


def start_master(thief_gb=1.0, wait_healthy=True):
    global master_proc
    log = open(f"{LAB}/e2e-master.log", "ab")
    env = {**os.environ, "SPILLWAY_FAULT_PANIC_NAME": FAULT_NAME}
    master_proc = subprocess.Popen([BIN, "master", "-c", f"{LAB}/e2e.toml"], stdout=log, stderr=log, start_new_session=True, env=env)
    wait(lambda: sh(f"mountpoint -q {MNT} && echo y || echo n") == "y", 15, "mount up")
    wait(lambda: api("/healthz") == "ok", 10, "api up")
    if wait_healthy:
        wait(lambda: all(d["status"] == "healthy" for d in ov()["dests"]), 10, "destinations healthy")


def stop_master(sig=signal.SIGINT, timeout=40):
    global master_proc
    if master_proc and master_proc.poll() is None:
        master_proc.send_signal(sig)
        try:
            master_proc.wait(timeout)
        except subprocess.TimeoutExpired:
            master_proc.kill()
            master_proc.wait()
    master_proc = None
    sh(f"umount -l {MNT}", check=False)


def fresh_lab(thief_gb=1.0):
    stop_master()
    stop_hook()
    sh("pkill -f '[t]ests/hookserver.py'", check=False)  # strays from demos
    shutil.rmtree(OUTBOX, ignore_errors=True)
    stop_node()
    sh(f"pkill -INT -f '{BIN}'", check=False)
    for _ in range(100):  # a master may still be flushing RAM on shutdown
        if sh(f"pgrep -f '{BIN}' || true") == "":
            break
        time.sleep(0.2)
    sh(f"pkill -9 -f '{BIN}'", check=False)
    time.sleep(0.3)
    sh(f"umount -l {MNT}", check=False)
    sh(f"umount -l {SPOOL}", check=False)
    mount_dests()
    wipe_dests()
    open(f"{LAB}/e2e.toml", "w").write(CONFIG.format(mnt=MNT, spool=SPOOL, lab=LAB, token=TOKEN, thief=thief_gb))
    start_master(thief_gb)


def start_node(dest_dir="d3"):
    global node_proc
    log = open(f"{LAB}/e2e-node.log", "ab")
    node_proc = subprocess.Popen(
        [BIN, "node", "--id", "node-b", "--master", API, "--token", TOKEN, "--listen", "127.0.0.1:7071", "--advertise", NODE_API],
        stdout=log, stderr=log, start_new_session=True,
    )
    wait(lambda: any(n["id"] == "node-b" and n["online"] for n in ov()["nodes"]), 10, "node-b online")


def start_hook():
    global hook_proc
    if hook_proc and hook_proc.poll() is None:
        return
    hook_proc = subprocess.Popen([sys.executable, os.path.join(HERE, "hookserver.py"), "7090"], start_new_session=True)
    wait(lambda: hstate() is not None, 5, "hook server up")
    time.sleep(0.2)
    check(hook_proc.poll() is None, "our hook server owns port 7090 (another one is running)")


def stop_hook():
    global hook_proc
    if hook_proc and hook_proc.poll() is None:
        hook_proc.kill()
        hook_proc.wait()
    hook_proc = None


def hctl(**kw):
    api("/control", "POST", kw, base=HOOK)


def hstate():
    try:
        return api("/state", base=HOOK)
    except Exception:  # noqa: BLE001
        return None


def set_hooks(validate=None, moved=None, **vopts):
    """Points spillway at the test hook server (None = leave as is, "" = disable)."""
    patch = {"hooks": {}}
    if validate is not None:
        patch["hooks"]["validate"] = {"url": validate, "token": "hook-token", "batch_max": 64, "batch_wait_ms": 50, "inflight": 4, "timeout_ms": 1000, "on_error": "move", "hold_retry_ms": 1000, **vopts}
    if moved is not None:
        patch["hooks"]["moved"] = {"url": moved, "token": "hook-token", "batch_max": 50, "batch_wait_ms": 300, "timeout_ms": 1000}
    api("/api/settings", "PUT", patch)


def hooks_off():
    set_hooks(validate="", moved="")


def stop_node():
    global node_proc
    if node_proc and node_proc.poll() is None:
        node_proc.send_signal(signal.SIGCONT)
        node_proc.kill()
        node_proc.wait()
    node_proc = None


# ----------------------------------------------------------------- scenarios

def s_basic():
    """Writes land in RAM, get parsed and moved; every file reads back identical through the mount."""
    files = {}
    c1, c2 = cam(), cam()
    for i in range(60):
        name = ["frag.mp4", "progressive.mp4", "faststart.mp4"][i % 3]
        rel = rec_path(c1 if i % 2 else c2, i, sub=(i % 7 == 0))
        write(rel, fixture(name), chunk=32768)
        files[rel] = md5(fixture(name))
    # immediately readable (from RAM)
    for rel, h in list(files.items())[:5]:
        check(md5(open(f"{MNT}/{rel}", "rb").read()) == h, f"RAM read mismatch {rel}")
    wait(lambda: all_moved(60), 30, "all 60 files moved")
    o = ov()
    check(o["mp4"]["ok"] >= 60 and o["mp4"]["failed"] == 0, f"mp4 parse: {o['mp4']['ok']} ok {o['mp4']['failed']} failed")
    check(o["mp4"]["fragmented"] >= 20, "fMP4 detected")
    for rel, h in files.items():
        w = where(rel)
        check(len(w) == 1, f"{rel} on exactly one destination, found {w}")
        check(md5(open(f"{MNT}/{rel}", "rb").read()) == h, f"post-move read mismatch {rel}")
    check(not os.listdir(SPOOL) or all(not files for _, _, files in os.walk(SPOOL)), "spool empty")
    # path survives moves: ls through the mount shows everything
    listed = sum(len(fs) for _, _, fs in os.walk(MNT))
    check(listed >= 60, f"union listing shows {listed} files")
    return f"60 files moved, md5 ok, {o['mp4']['fragmented']} fMP4"


def s_balance():
    """Strict mode honors free-space proportions (d1 is ~2x d2)."""
    before = {d["id"]: d["moved_files"] for d in ov()["dests"]}
    o = ov()
    free = {d["id"]: d["free_bytes"] for d in o["dests"]}
    data = os.urandom(256 * 1024)
    c = cam()
    for i in range(300):
        write(rec_path(c, 1000 + i), data)
        if i % 30 == 29:
            time.sleep(0.4)  # paced: below the RAM pressure threshold
    wait(lambda: all_moved(), 60, "balance batch moved")
    o = ov()
    moved = {d["id"]: d["moved_files"] - before[d["id"]] for d in o["dests"]}
    expect = free["d1"] / (free["d1"] + free["d2"])
    got = moved["d1"] / max(1, moved["d1"] + moved["d2"])
    check(abs(got - expect) < 0.08, f"d1 share {got:.2f} vs free-space share {expect:.2f} (moved {moved})")
    return f"split {moved} ≈ free-space share {expect:.2f}"


def s_modify():
    """Files are modifiable: reopen during settle, reopen after the move (copy-up), truncate-rewrite."""
    c = cam()
    a = rec_path(c, 1)
    # 1. write half, close, reopen within settle and append
    data = fixture("progressive.mp4")
    write(a, data[: len(data) // 2])
    time.sleep(0.3)
    with open(f"{MNT}/{a}", "r+b") as f:
        f.seek(len(data) // 2)
        f.write(data[len(data) // 2:])
    wait(lambda: where(a), 15, "appended file moved")
    check(md5(open(f"{LAB}/{where(a)[0]}/{a}", "rb").read()) == md5(data), "appended file intact on disk")
    # 2. modify after it moved: copy-up, then re-move
    first = where(a)[0]
    with open(f"{MNT}/{a}", "r+b") as f:
        f.seek(0, 2)
        f.write(b"TRAILER")
    wait(lambda: ov()["ram"]["files"] == 0, 15, "copied-up file moved again")
    w = where(a)
    check(len(w) == 1, f"exactly one copy after re-move (old copy cleaned): {w} (first was {first})")
    check(open(f"{LAB}/{w[0]}/{a}", "rb").read() == data + b"TRAILER", "modification persisted")
    check(open(f"{MNT}/{a}", "rb").read() == data + b"TRAILER", "mount shows modification")
    # 3. O_TRUNC rewrite of a moved file
    with open(f"{MNT}/{a}", "wb") as f:
        f.write(b"short")
    wait(lambda: ov()["ram"]["files"] == 0, 15, "rewritten file moved")
    w = where(a)
    check(len(w) == 1 and open(f"{LAB}/{w[0]}/{a}", "rb").read() == b"short", f"truncate-rewrite persisted ({w})")
    # 4. rename inside RAM before sealing
    b = rec_path(c, 2)
    tmp = b + ".mp4"
    write(tmp, data)
    os.rename(f"{MNT}/{tmp}", f"{MNT}/{b}")
    wait(lambda: where(b), 15, "renamed file moved")
    check(not where(tmp), "temp name never reached disk")
    # 5. unlink before the move
    d = rec_path(c, 3)
    write(d, data)
    os.remove(f"{MNT}/{d}")
    time.sleep(2.5)
    check(not where(d) and not os.path.exists(f"{MNT}/{d}"), "unlinked file never reached disk")
    return "append, copy-up, truncate, rename, unlink ok"


def s_rules():
    """Extension allowlist, read-only moved files without allow_delete, empty files dropped."""
    c = cam()
    try:
        open(f"{MNT}/{c}.txt", "wb").write(b"x")
        raise AssertionError("non-mp4 create should fail")
    except PermissionError:
        pass
    rel = rec_path(c, 1)
    write(rel, fixture("frag.mp4"))
    wait(lambda: where(rel), 15, "moved")
    try:
        os.remove(f"{MNT}/{rel}")
        raise AssertionError("delete of moved file should be EROFS")
    except OSError as e:
        check(e.errno == 30, f"EROFS expected, got {e}")
    empty = rec_path(c, 2)
    open(f"{MNT}/{empty}", "wb").close()
    time.sleep(2.5)
    check(not where(empty) and not os.path.exists(f"{MNT}/{empty}"), "empty file dropped")
    st = os.statvfs(MNT)
    check(st.f_blocks > 0, "statfs works")
    return "EACCES on .txt, EROFS on moved delete, empty dropped"


def traffic(stop, files, errors, cams=8, delay=0.05):
    """Background writer used by failure scenarios."""
    cs = [cam() for _ in range(cams)]
    i = 0
    data = fixture("frag.mp4")
    while not stop.is_set():
        rel = rec_path(cs[i % cams], 50000 + i)
        try:
            write(rel, data, chunk=65536)
            files.append(rel)
        except OSError as e:
            errors.append((rel, str(e)))
        i += 1
        time.sleep(delay)


def run_with_traffic(fn, **kw):
    stop, files, errors = threading.Event(), [], []
    t = threading.Thread(target=traffic, args=(stop, files, errors), kwargs=kw, daemon=True)
    t.start()
    try:
        fn(files)
    finally:
        stop.set()
        t.join()
    return files, errors


def verify_all(files, what):
    wait(lambda: all_moved(), 60, f"{what}: RAM drained")
    h = md5(fixture("frag.mp4"))
    missing = [f for f in files if not where(f)]
    check(not missing, f"{what}: {len(missing)} files never reached a destination, e.g. {missing[:3]}")
    dup = [f for f in files if len(where(f)) > 1]
    check(not dup, f"{what}: duplicated files {dup[:3]}")
    bad = [f for f in files[:: max(1, len(files) // 40)] if md5(open(f"{MNT}/{f}", "rb").read()) != h]
    check(not bad, f"{what}: content mismatch {bad[:3]}")


def s_dest_readonly():
    """A destination turning read-only (I/O errors) is detected, traffic fails over, then it recovers."""
    def body(files):
        time.sleep(2)
        # remount fails with EBUSY while a copy is writing; retry like an admin would
        wait(lambda: sh(f"mount -o remount,ro {LAB}/d2", check=False) == "" and sh(f"findmnt -no OPTIONS {LAB}/d2").startswith("ro"), 10, "d2 remounted read-only", every=0.1)
        wait(lambda: dest(ov(), "d2")["status"] in ("error", "suspended"), 10, "d2 marked failing")
        n0 = dest(ov(), "d2")["moved_files"]
        time.sleep(4)
        check(dest(ov(), "d2")["moved_files"] == n0, "nothing moved to read-only d2")
        sh(f"mount -o remount,rw {LAB}/d2")
        wait(lambda: dest(ov(), "d2")["status"] == "healthy", 10, "d2 recovered")
        wait(lambda: dest(ov(), "d2")["moved_files"] > n0, 15, "d2 receives again")
    files, errors = run_with_traffic(body)
    check(not errors, f"writers saw errors: {errors[:3]}")
    verify_all(files, "readonly")
    evs = [e["code"] for e in api("/api/events")]
    check("dest_error" in evs and "dest_healthy" in evs, "events emitted")
    return f"{len(files)} files written through the outage, none lost"


def s_dest_unmounted():
    """NFS-style disappearance: the mount vanishes (expect_mount) — nothing may be written to the parent disk."""
    # start from a clean directory *under* the mountpoint (leftovers of older runs hide there)
    sh(f"umount -l {LAB}/d2")
    for e in os.listdir(f"{LAB}/d2"):
        p = f"{LAB}/d2/{e}"
        shutil.rmtree(p) if os.path.isdir(p) else os.remove(p)
    sh(f"mount -o loop {LAB}/d2.img {LAB}/d2")
    wait(lambda: dest(ov(), "d2")["status"] == "healthy", 10, "d2 healthy before the test")

    def body(files):
        time.sleep(2)
        sh(f"umount -l {LAB}/d2")
        wait(lambda: dest(ov(), "d2")["status"] == "error", 10, "d2 error (not a mountpoint)")
        reason = dest(ov(), "d2")["reason"] or ""
        check("montagem" in reason, f"reason mentions the mountpoint: {reason}")
        time.sleep(3)
        stray = [p for p in os.listdir(f"{LAB}/d2") if p != ".spillway"]
        check(not stray, f"files leaked into the parent disk: {stray[:5]}")
        sh(f"mount -o loop {LAB}/d2.img {LAB}/d2")
        wait(lambda: dest(ov(), "d2")["status"] == "healthy", 10, "d2 healthy after remount")
    files, errors = run_with_traffic(body)
    check(not errors, f"writers saw errors: {errors[:3]}")
    verify_all(files, "unmounted")
    return f"{len(files)} files, no writes to the root disk while unmounted"


def s_dest_full():
    """A filling destination stops receiving before max_use; nothing fails with ENOSPC."""
    wipe_dests()
    time.sleep(2.5)  # next probe sees the freed space
    api("/api/destinations/d1", "PATCH", {"max_use_pct": 1 + dest(ov(), "d1")["use_pct"] + 6})
    lim = dest(ov(), "d1")["max_use_pct"]
    fails0 = ov()["totals"]["move_failures"]

    def body(files):
        wait(lambda: dest(ov(), "d1")["status"] == "full", 60, "d1 full")
    files, errors = run_with_traffic(body, delay=0.01)
    verify_all(files, "full")
    o = ov()
    d1 = dest(o, "d1")
    check(d1["use_pct"] < lim + 0.5, f"d1 stayed under its limit: {d1['use_pct']:.1f}% vs {lim:.1f}%")
    check(o["totals"]["move_failures"] == fails0, f"no ENOSPC failures ({o['totals']['move_failures'] - fails0})")
    api("/api/destinations/d1", "PATCH", {"max_use_pct": 95})
    wipe_dests()
    return f"d1 stopped at {d1['use_pct']:.1f}% (limit {lim:.1f}%), zero failures"


def s_thief():
    """All destinations down: RAM fills, the thief discards the oldest closed files, writers keep going."""
    api("/api/settings", "PUT", {"ram": {"thief_max_gb": 0.25, "thief_resume_pct": 60}})
    for d in ("d1", "d2"):
        api(f"/api/destinations/{d}", "PATCH", {"enabled": False})
    wait(lambda: not any(d["status"] == "healthy" for d in ov()["dests"]), 5, "no writable dests")
    data = fixture("frag.mp4")
    c = cam()
    written = []
    for i in range(140):  # ~440 MB into a 256 MiB ceiling, at ~100 MB/s
        rel = rec_path(c, 70000 + i)
        write(rel, data)
        written.append(rel)
        time.sleep(0.03)
    o = wait(lambda: ov() if ov()["totals"]["thief_files"] > 0 else None, 10, "thief discarded files")
    check(o["ram"]["used_bytes"] <= o["ram"]["hard_limit_bytes"], "RAM under hard limit")
    stolen = o["totals"]["thief_files"]
    oldest_gone = not os.path.exists(f"{MNT}/{written[0]}")
    newest_kept = os.path.exists(f"{MNT}/{written[-1]}")
    check(oldest_gone and newest_kept, "thief takes the oldest, keeps the newest")
    for d in ("d1", "d2"):
        api(f"/api/destinations/{d}", "PATCH", {"enabled": True})
    wait(lambda: all_moved(), 40, "backlog drained after destinations return")
    kept = [w for w in written if where(w)]
    check(len(kept) + stolen >= len(written), f"every file either moved or stolen ({len(kept)} + {stolen} / {len(written)})")
    api("/api/settings", "PUT", {"ram": {"thief_max_gb": 1.0, "thief_resume_pct": 70}})
    evs = [e["code"] for e in api("/api/events")]
    check("thief_active" in evs and "thief_standby" in evs, "thief events")
    wipe_dests()
    return f"{stolen} oldest files discarded, {len(kept)} newest moved after recovery"


def s_hard_limit():
    """Above the hard limit writes fail with ENOSPC instead of exhausting host memory."""
    api("/api/settings", "PUT", {"ram": {"thief_max_gb": 0.1, "hard_limit_pct": 110}})
    for d in ("d1", "d2"):
        api(f"/api/destinations/{d}", "PATCH", {"enabled": False})
    # one huge open file cannot be stolen: it hits the hard limit
    p = f"{MNT}/{cam()}/2026-01-01/00-00-00-1.mp4"
    os.makedirs(os.path.dirname(p), exist_ok=True)
    got_enospc = False
    try:
        # with the kernel write-back cache the error may surface on write, flush or close
        with open(p, "wb") as f:
            for _ in range(200):
                f.write(os.urandom(1 << 20))
                f.flush()
    except OSError as e:
        got_enospc = e.errno == 28
    check(got_enospc, "ENOSPC at hard limit")
    o = ov()
    check(o["ram"]["used_bytes"] <= o["ram"]["hard_limit_bytes"], "used <= hard limit")
    check(o["thief"]["stalled"] or o["totals"]["write_rejected"] > 0, "rejection visible in stats")
    os.remove(p)
    for d in ("d1", "d2"):
        api(f"/api/destinations/{d}", "PATCH", {"enabled": True})
    api("/api/settings", "PUT", {"ram": {"thief_max_gb": 1.0, "hard_limit_pct": 120}})
    return "ENOSPC raised at the hard limit; host memory protected"


def s_remote_node():
    """A node on 'another server' serves d3: files flow to it, reads go over HTTP, hang/kill fail over."""
    start_node()
    api("/api/destinations", "POST", {"id": "d3", "node": "node-b", "path": f"{LAB}/d3", "expect_mount": True, "weight": 50})
    wait(lambda: dest(ov(), "d3")["status"] == "healthy", 10, "remote d3 healthy")
    c = cam()
    data = fixture("progressive.mp4")
    rels = [rec_path(c, 90000 + i) for i in range(30)]
    for r in rels:
        write(r, data)
    wait(lambda: all_moved(), 30, "moved incl. remote")
    on_d3 = [r for r in rels if "d3" in where(r)]
    check(on_d3, "some files went to the remote node")
    for r in on_d3[:5]:
        check(md5(open(f"{MNT}/{r}", "rb").read()) == md5(data), "read through remote node")

    def hang(files):
        time.sleep(1.5)
        node_proc.send_signal(signal.SIGSTOP)  # TCP accepts, nothing answers
        wait(lambda: dest(ov(), "d3")["status"] == "offline", 12, "hung node offline")
        t0 = time.time()
        os.listdir(MNT)  # union listing must not hang on the dead node
        check(time.time() - t0 < 3, "listing does not wait on a hung node")
        time.sleep(2)
        node_proc.send_signal(signal.SIGCONT)
        wait(lambda: dest(ov(), "d3")["status"] == "healthy", 12, "node back")
    files, errors = run_with_traffic(hang, delay=0.1)
    check(not errors, f"writers saw errors: {errors[:3]}")
    verify_all(files, "node hang")

    def crash(files):
        time.sleep(1.5)
        node_proc.kill()
        wait(lambda: dest(ov(), "d3")["status"] == "offline", 12, "killed node offline")
        time.sleep(2)
        start_node()
        wait(lambda: dest(ov(), "d3")["status"] == "healthy", 12, "restarted node healthy")
    files2, errors2 = run_with_traffic(crash, delay=0.1)
    check(not errors2, f"writers saw errors: {errors2[:3]}")
    verify_all(files2, "node crash")
    leftovers = os.listdir(f"{LAB}/d3/.spillway/tmp") if os.path.isdir(f"{LAB}/d3/.spillway/tmp") else []
    check(not leftovers, f"partial transfers cleaned: {leftovers[:3]}")
    api("/api/destinations/d3", "DELETE")
    stop_node()
    wipe_dests()
    return f"{len(on_d3)} files on remote node; survived SIGSTOP and SIGKILL ({len(files) + len(files2)} files during faults)"


def s_remote_reenable():
    """Disable + quick re-enable of a remote destination: nothing is sent before the node confirms it, no failures."""
    start_node()
    api("/api/destinations", "POST", {"id": "d3", "node": "node-b", "path": f"{LAB}/d3", "expect_mount": True})
    wait(lambda: dest(ov(), "d3")["status"] == "healthy", 10, "remote d3 healthy")
    api("/api/destinations/d1", "PATCH", {"enabled": False})
    api("/api/destinations/d2", "PATCH", {"enabled": False})
    c = cam()
    data = fixture("progressive.mp4")
    rels, seen = [], set()
    # re-enable at different points of the node's 2 s heartbeat cycle; the
    # window that used to fail is "before the heartbeat that drops it"
    for k, pause in enumerate((0.1, 0.4, 0.8, 1.2, 1.6, 2.2)):
        api("/api/destinations/d3", "PATCH", {"enabled": False})
        batch = [rec_path(c, 97000 + k * 100 + i) for i in range(15)]
        for r in batch:
            write(r, data)
        time.sleep(pause)
        api("/api/destinations/d3", "PATCH", {"enabled": True})
        t0 = time.time()
        while ov()["ram"]["files"] > 0 and time.time() - t0 < 20:
            seen.add(dest(ov(), "d3")["status"])
            time.sleep(0.05)
        rels += batch
    wait(lambda: all_moved(), 20, "everything moved to the remote node")
    d3 = dest(ov(), "d3")
    check(d3["failures"] == 0, f"no transfer failures on re-enable (got {d3['failures']}: {d3['last_error']})")
    check("suspended" not in seen and "error" not in seen, f"never suspended/erroring: {sorted(seen)}")
    check(all("d3" in where(r) for r in rels), "all files on d3")
    api("/api/destinations/d1", "PATCH", {"enabled": True})
    api("/api/destinations/d2", "PATCH", {"enabled": True})
    api("/api/destinations/d3", "DELETE")
    stop_node()
    wipe_dests()
    return f"6 re-enables, {len(rels)} files, 0 failures; states seen: {', '.join(sorted(seen))}"


def s_burst_moved():
    """Paced burst of new files with a remote node and the moved hook on: the master never stalls, no destination drops out."""
    start_hook()
    hctl(reset=True)
    set_hooks(validate="", moved=f"{HOOK}/spillway/moved")
    start_node()
    api("/api/destinations", "POST", {"id": "d3", "node": "node-b", "path": f"{LAB}/d3", "expect_mount": True})
    wait(lambda: dest(ov(), "d3")["status"] == "healthy", 10, "remote d3 healthy")
    seq0 = max((e["seq"] for e in api("/api/events")), default=0)
    data = os.urandom(200 * 1024)
    n, threads, secs = 3000, 24, 8.0
    cams_ = [cam() for _ in range(60)]
    slowest, errs = [0.0], []

    def worker(k):
        t0 = time.time()
        per = n // threads
        for j in range(per):
            wait_s = t0 + secs * j / per - time.time()
            if wait_s > 0:
                time.sleep(wait_s)
            t1 = time.time()
            try:
                write(rec_path(cams_[(k * 7 + j) % len(cams_)], 300000 + k * 1000 + j), data, chunk=1 << 20)
            except OSError as e:
                errs.append(str(e))
            slowest[0] = max(slowest[0], time.time() - t1)
    t0 = time.time()
    ths = [threading.Thread(target=worker, args=(k,)) for k in range(threads)]
    [t.start() for t in ths]
    [t.join() for t in ths]
    dt = time.time() - t0
    check(not errs, f"write errors: {errs[:3]}")
    check(dt < secs + 2.5, f"burst paced at {n / secs:.0f}/s finished in {dt:.1f}s (writers were held up)")
    check(slowest[0] < 1.5, f"slowest single file write {slowest[0]:.2f}s")
    wait(lambda: all_moved(), 60, "burst moved")
    trips = [e["message"] for e in api("/api/events") if e["seq"] > seq0 and e["code"] == "dest_unresponsive"]
    check(not trips, f"no destination dropped out: {trips[:2]}")
    st = wait(lambda: hstate() if hstate()["moved_unique"] >= n else None, 30, "every moved event delivered")
    api("/api/destinations/d3", "DELETE")
    stop_node()
    hooks_off()
    wipe_dests()
    return f"{n} files in {dt:.1f}s ({n / dt:.0f}/s), slowest write {slowest[0] * 1000:.0f} ms, 0 breaker trips, {st['moved_unique']} moved events"


def s_external_changes():
    """Files deleted or moved directly on the disks (behind spillway's back) are noticed: no stale opens, reads follow the file."""
    start_node()
    api("/api/destinations", "POST", {"id": "d3", "node": "node-b", "path": f"{LAB}/d3", "expect_mount": True, "weight": 3})
    wait(lambda: dest(ov(), "d3")["status"] == "healthy", 10, "remote d3 healthy")
    data = fixture("frag.mp4")
    c = cam()
    rels = [rec_path(c, 400000 + i) for i in range(18)]
    for r in rels:
        write(r, data)
    wait(lambda: all_moved(), 30, "files moved")
    for r in rels:  # warm the location cache
        check(md5(open(f"{MNT}/{r}", "rb").read()) == md5(data), "read before changes")
    on = {d: [r for r in rels if d in where(r)] for d in DESTS}
    check(on["d1"] and on["d3"] and len(on["d1"]) >= 3, f"files on local and remote disks: { {d: len(v) for d, v in on.items()} }")
    gone_local, gone_remote = on["d1"][0], on["d3"][0]
    moved_closed, moved_open = on["d1"][1], on["d1"][2]
    # an open handle that started reading before the file moves away
    fh = open(f"{MNT}/{moved_open}", "rb")
    head = fh.read(4096)

    def move(rel, src, dst):
        os.makedirs(os.path.dirname(f"{LAB}/{dst}/{rel}"), exist_ok=True)
        shutil.copy2(f"{LAB}/{src}/{rel}", f"{LAB}/{dst}/{rel}")
        os.remove(f"{LAB}/{src}/{rel}")
    os.remove(f"{LAB}/d1/{gone_local}")
    os.remove(f"{LAB}/d3/{gone_remote}")
    move(moved_closed, "d1", "d2")
    move(moved_open, "d1", "d2")
    time.sleep(2.5)
    for r, what in ((gone_local, "local"), (gone_remote, "remote")):
        try:
            open(f"{MNT}/{r}", "rb").read()
            check(False, f"deleted {what} file still opens from the cache")
        except FileNotFoundError:
            pass
    check(md5(open(f"{MNT}/{moved_closed}", "rb").read()) == md5(data), "moved file reads from its new disk")
    fh.seek(len(data) - 300 * 1024)
    tail = fh.read()
    fh.close()
    check(tail == data[-300 * 1024:] and head == data[:4096], "an open handle keeps reading after the file moved")
    api("/api/destinations/d3", "DELETE")
    stop_node()
    wipe_dests()
    return "deleted files (local and remote) stop opening within 2.5 s; moved files read from the new disk, even mid-read"


def s_fs_panic():
    """An internal panic in a filesystem operation answers EIO for that call only: the mount stays up for every other writer."""
    files, errors = [], []
    stop = threading.Event()
    c = cam()

    def writer():
        i = 0
        while not stop.is_set():
            r = rec_path(c, 500000 + i)
            try:
                write(r, fixture("frag.mp4"))
                files.append(r)
            except OSError as e:
                errors.append(str(e))
            i += 1
            time.sleep(0.02)
    th = threading.Thread(target=writer)
    th.start()
    time.sleep(0.5)
    hits = 0
    for _ in range(5):
        try:
            os.stat(f"{MNT}/{c}/{FAULT_NAME}")
        except OSError as e:
            hits += e.errno == 5
    time.sleep(0.5)
    stop.set()
    th.join()
    check(hits == 5, f"injected panics answered EIO ({hits}/5)")
    check(sh(f"mountpoint -q {MNT} && echo y || echo n") == "y", "mount still up after panics")
    check(not errors, f"other writers unaffected: {errors[:3]}")
    wait(lambda: all_moved(), 20, "files moved")
    verify_all(files, "after panics")
    wait(lambda: any(e["code"] == "fs_panic" for e in api("/api/events")), 5, "fs_panic event in the panel")
    return f"5 injected panics answered EIO; mount stayed up; {len(files)} files written meanwhile, all intact"


def s_underlay_rescue():
    """Mount down, the recorder keeps writing into the plain directory under it: after restart those files are pulled in and moved."""
    data = fixture("frag.mp4")
    c = cam()
    stop_master()
    check(sh(f"mountpoint -q {MNT} && echo y || echo n") == "n", "mount is gone")
    # the recorder doesn't notice: it keeps writing segments into the local directory
    local = [rec_path(c, 600000 + i) for i in range(20)]
    for r in local:
        os.makedirs(os.path.dirname(f"{MNT}/{r}"), exist_ok=True)
        with open(f"{MNT}/{r}", "wb") as f:
            f.write(data)
    # ...and one segment is still open while spillway comes back
    slow = rec_path(c, 600100)
    fh = open(f"{MNT}/{slow}", "wb")
    fh.write(data[: len(data) // 2])
    under = os.open(MNT, os.O_RDONLY | os.O_DIRECTORY)  # our own view of the directory under the mount
    start_master()
    fh.write(data[len(data) // 2:])  # still lands on the local disk, through the old handle
    fh.close()

    def underlay_files():
        out = []
        for root, _, names in os.walk(f"/proc/self/fd/{under}"):
            out += [os.path.join(root, n) for n in names]
        return out
    wait(lambda: not underlay_files(), 45, "local directory under the mount emptied")
    wait(lambda: all_moved() and all(where(r) for r in local + [slow]), 30, "rescued files on the destinations")
    os.close(under)
    bad = [r for r in local + [slow] if md5(open(f"{MNT}/{r}", "rb").read()) != md5(data)]
    check(not bad, f"rescued content intact: {bad[:3]}")
    check(any(e["code"] == "underlay_rescued" for e in api("/api/events")), "underlay_rescued event in the panel")
    wipe_dests()
    return f"{len(local) + 1} segments written to the local disk during the downtime (one still open at restart) were rescued and moved"


def s_master_crash():
    """kill -9 the master with files in RAM: on restart the spool is recovered and flushed."""
    api("/api/destinations/d1", "PATCH", {"enabled": False})
    api("/api/destinations/d2", "PATCH", {"enabled": False})
    c = cam()
    rels = [rec_path(c, 95000 + i) for i in range(20)]
    for r in rels:
        write(r, fixture("faststart.mp4"))
    time.sleep(1.5)
    check(ov()["ram"]["files"] >= 20, "files held in RAM")
    stop_master(signal.SIGKILL)
    check(sh(f"ls {SPOOL} | wc -l") != "0", "spool survived the crash (tmpfs)")
    # destinations come back in config before restart
    cfg = open(f"{LAB}/e2e.toml").read().replace("enabled = false", "enabled = true")
    open(f"{LAB}/e2e.toml", "w").write(cfg)
    start_master()
    wait(lambda: all_moved(), 30, "recovered files moved")
    missing = [r for r in rels if not where(r)]
    check(not missing, f"lost after crash: {missing[:3]}")
    evs = [e["code"] for e in api("/api/events")]
    check("spool_recovered" in evs, "recovery event")
    return "20 files survived kill -9 and were flushed after restart"


def s_graceful_stop():
    """SIGTERM flushes RAM to destinations before exiting."""
    c = cam()
    rels = [rec_path(c, 97000 + i) for i in range(15)]
    for r in rels:
        write(r, fixture("frag.mp4"))
    stop_master(signal.SIGTERM)
    missing = [r for r in rels if not where(r)]
    check(not missing, f"not flushed on stop: {missing[:3]}")
    start_master()
    return "graceful stop flushed everything"


def s_throughput():
    """Thousands of files per minute: many small segments from many cameras at once."""
    wipe_dests()
    data = os.urandom(150 * 1024)
    n_cams, per = 200, 25
    cams_ = [cam() for _ in range(n_cams)]
    t0 = time.time()
    errs = []

    def worker(k):
        for i in range(per):
            try:
                write(rec_path(cams_[k], 200000 + i), data, chunk=16384)
            except OSError as e:
                errs.append(str(e))
    ths = [threading.Thread(target=worker, args=(k,)) for k in range(n_cams)]
    [t.start() for t in ths]
    [t.join() for t in ths]
    dt = time.time() - t0
    n = n_cams * per
    wait(lambda: all_moved(), 120, "throughput batch moved")
    dt_all = time.time() - t0
    check(not errs, f"write errors: {errs[:3]}")
    rate = n / dt * 60
    check(rate > 5000, f"ingest rate {rate:.0f} files/min")
    for d in ("d1", "d2"):
        st = os.statvfs(f"{LAB}/{d}")
        used = 100 * (1 - st.f_bavail / st.f_blocks)
        check(used <= 95.5, f"{d} real usage {used:.1f}% must stay under max_use")
    wipe_dests()
    return f"{n} files (16 KiB writes) in {dt:.1f}s = {rate:,.0f} files/min, {n * len(data) / dt / 1048576:.0f} MiB/s; all on disk after {dt_all:.1f}s"


def hook_files(prefix, n, drops=0):
    """Writes n files under <prefix>/, the first `drops` of them marked for the hook to drop."""
    rels = []
    data = fixture("faststart.mp4")
    for i in range(n):
        name = f"drop-{i:03d}.mp4" if i < drops else f"seg-{i:03d}.mp4"
        rel = f"{prefix}/2026-10-22/{name}"
        write(rel, data)
        rels.append(rel)
    return rels


def s_hook_validate():
    """validate hook: drops what it refuses, labels what it keeps, batches under a burst; moved hook gets every landed file."""
    start_hook()
    hctl(reset=True)
    set_hooks(validate=f"{HOOK}/spillway/validate", moved=f"{HOOK}/spillway/moved")
    g = cam()
    rels = hook_files(g, 60, drops=15)
    wait(lambda: all_moved(), 30, "decided and moved")
    dropped = [r for r in rels if "/drop-" in r]
    kept = [r for r in rels if "/drop-" not in r]
    check(not any(where(r) for r in dropped), "dropped files never reach a disk")
    check(not any(os.path.exists(f"{MNT}/{r}") for r in dropped), "dropped files are gone from the mount")
    check(all(len(where(r)) == 1 for r in kept), "kept files moved")
    st = wait(lambda: hstate() if hstate()["moved_unique"] >= len(kept) else None, 15, "moved events delivered")
    check(set(st["moved_ids"]) == set(kept), f"moved hook got exactly the kept files ({len(st['moved_ids'])} of {len(kept)})")
    check(max(st["batch_sizes"]) > 1, f"burst is batched (max batch {max(st['batch_sizes'])})")
    check(max(st["batch_sizes"]) <= 64, "batch_max respected")
    check(st["unauthorized"] == 0, "token sent")
    groups = api(f"/api/groups?key=group")["groups"]
    mine = next(x for x in groups if x["value"] == g)
    check(mine["files"] == len(kept) and mine["dropped_files"] == len(dropped), f"labels drive the dashboard groups ({mine['files']} moved, {mine['dropped_files']} dropped)")
    o = ov()
    check(o["hooks"]["validate"]["dropped"] >= 15 and o["hooks"]["moved"]["pending"] == 0, "hook stats in overview")
    recent = api("/api/recent")
    check(any(r["outcome"] == "dropped" and r["reason"] == "marked drop" for r in recent), "drop reason in recent files")
    hooks_off()
    return f"{len(dropped)} dropped, {len(kept)} moved with labels, batches up to {max(st['batch_sizes'])}, {st['validate_calls']} calls"


def s_hook_down():
    """validate hook unreachable (fail-open): files still move, the failure is visible."""
    stop_hook()
    set_hooks(validate="http://127.0.0.1:7099/spillway/validate")
    t0 = time.time()
    rels = hook_files(cam(), 30)
    wait(lambda: all_moved(), 20, "moved despite the hook being down")
    took = time.time() - t0
    check(all(where(r) for r in rels), "all files on disk")
    o = ov()
    check(o["hooks"]["validate"]["failing"] and o["hooks"]["validate"]["errors"] > 0, "hook marked failing")
    check(any(e["code"] == "hook_validate_error" for e in api("/api/events")), "event emitted")
    hooks_off()
    return f"30 files moved in {took:.1f}s with the hook down"


def s_hook_slow():
    """validate hook slower than its timeout: batches time out and fail open without stalling."""
    start_hook()
    hctl(reset=True, validate="slow", delay=5)
    set_hooks(validate=f"{HOOK}/spillway/validate", timeout_ms=800)
    t0 = time.time()
    rels = hook_files(cam(), 40)
    wait(lambda: all_moved(), 15, "moved after hook timeouts")
    took = time.time() - t0
    check(all(where(r) for r in rels), "all files on disk")
    check(took < 10, f"not stalled by the slow hook ({took:.1f}s)")
    check(ov()["hooks"]["validate"]["fallback"] >= 40, "fallback decisions counted")
    hctl(validate="ok")
    hooks_off()
    return f"40 files moved in {took:.1f}s (hook sleeps 5 s, timeout 0.8 s)"


def s_hook_hold():
    """hold: the hook asks for more time once, files wait in RAM, then move."""
    start_hook()
    hctl(reset=True, validate="hold")
    set_hooks(validate=f"{HOOK}/spillway/validate")
    rels = hook_files(cam(), 20)
    time.sleep(1.6)  # settle 1 s + first answer
    o = ov()
    check(o["ram"]["deciding"] > 0 or o["hooks"]["validate"]["held"] > 0, "files held for a decision")
    wait(lambda: all_moved(), 15, "moved after hold")
    check(all(where(r) for r in rels), "all files on disk")
    check(hstate()["decisions"]["hold"] == 20, "every file held once")
    hctl(validate="ok")
    hooks_off()
    return "20 files held once, then moved"


def s_hook_fail_closed():
    """on_error=hold with the hook down: nothing leaves RAM until the hook is back."""
    stop_hook()
    set_hooks(validate=f"{HOOK}/spillway/validate", on_error="hold", hold_retry_ms=500)
    rels = hook_files(cam(), 15, drops=5)
    time.sleep(4)
    o = ov()
    check(o["ram"]["deciding"] == 15 and not any(where(r) for r in rels), f"held in RAM while the hook is down ({o['ram']['deciding']} deciding)")
    start_hook()
    hctl(reset=True)
    wait(lambda: all_moved(), 15, "decided once the hook is back")
    check(sum(1 for r in rels if where(r)) == 10, "kept files moved, refused ones dropped")
    hooks_off()
    return "15 files waited 4 s for the hook, then 10 moved and 5 dropped"


def s_hook_outbox_crash():
    """moved hook down + master kill -9: the outbox replays every event after restart."""
    start_hook()
    hctl(reset=True, moved="error")
    set_hooks(validate="", moved=f"{HOOK}/spillway/moved")
    rels = hook_files(cam(), 25)
    wait(lambda: all_moved(), 20, "files moved while the receiver is down")
    wait(lambda: ov()["hooks"]["moved"]["pending"] >= 25, 5, "events queued in the outbox")
    wait(lambda: ov()["hooks"]["moved"]["failing"], 5, "moved hook marked failing")
    stop_master(signal.SIGKILL)
    check(any(f.endswith(".jsonl") for f in os.listdir(OUTBOX)), "outbox segments on disk")
    start_master()
    check(ov()["hooks"]["moved"]["pending"] >= 25, "outbox reloaded after restart")
    hctl(moved="ok")
    st = wait(lambda: hstate() if hstate()["moved_unique"] >= 25 else None, 40, "replayed after the receiver is back")
    check(set(rels) <= set(st["moved_ids"]), "every moved file delivered")
    wait(lambda: ov()["hooks"]["moved"]["pending"] == 0, 10, "outbox drained")
    check(not [f for f in os.listdir(OUTBOX) if f.endswith(".jsonl")], "delivered segments deleted")
    hooks_off()
    return f"25 events survived kill -9; delivered {st['moved_events']} times total ({st['moved_dupes']} duplicates, receiver must be idempotent)"


SCENARIOS = [
    ("basic", s_basic),
    ("balance", s_balance),
    ("modify", s_modify),
    ("rules", s_rules),
    ("readonly", s_dest_readonly),
    ("unmounted", s_dest_unmounted),
    ("full", s_dest_full),
    ("thief", s_thief),
    ("hardlimit", s_hard_limit),
    ("remote", s_remote_node),
    ("reenable", s_remote_reenable),
    ("burstmoved", s_burst_moved),
    ("extchanges", s_external_changes),
    ("fspanic", s_fs_panic),
    ("underlay", s_underlay_rescue),
    ("crash", s_master_crash),
    ("stop", s_graceful_stop),
    ("hookvalidate", s_hook_validate),
    ("hookdown", s_hook_down),
    ("hookslow", s_hook_slow),
    ("hookhold", s_hook_hold),
    ("hookclosed", s_hook_fail_closed),
    ("outbox", s_hook_outbox_crash),
    ("throughput", s_throughput),
]


def main():
    want = sys.argv[1:]
    sel = [s for s in SCENARIOS if not want or s[0] in want]
    sh(f"bash {HERE}/mkfixtures.sh {FIX}")
    open(f"{LAB}/e2e-master.log", "w").close()
    open(f"{LAB}/e2e-node.log", "w").close()
    fresh_lab()
    for name, fn in sel:
        t0 = time.time()
        try:
            msg = fn()
            results.append((name, True, msg, time.time() - t0))
            print(f"PASS  {name:<11} {time.time() - t0:5.1f}s  {msg}", flush=True)
        except Exception as e:  # noqa: BLE001
            results.append((name, False, str(e), time.time() - t0))
            print(f"FAIL  {name:<11} {time.time() - t0:5.1f}s  {e}", flush=True)
            traceback.print_exc()
            fresh_lab()  # isolate the next scenario
    stop_master()
    stop_node()
    stop_hook()
    passed = sum(1 for r in results if r[1])
    print(f"\n{passed}/{len(results)} scenarios passed")
    json.dump([{"name": n, "ok": ok, "detail": m, "secs": round(s, 1)} for n, ok, m, s in results], open(f"{LAB}/e2e-results.json", "w"), indent=1)
    sys.exit(0 if passed == len(results) else 1)


if __name__ == "__main__":
    main()
