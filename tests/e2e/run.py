"""Runs TranslateProbe E2E scenarios: make_scenario.js, fakewow, the Helper, then check.py.

The Helper is Translate Companion, WoWTranslateCompanion.exe (TranslateProbeHelper.exe until
2026-10-19); --helper takes any build of it, for example one from another CARGO_TARGET_DIR.

    python -I tests/e2e/run.py --scenario basic [--capture wgc|gdi] [--translator echo|llama]
    python -I tests/e2e/run.py --all [--capture wgc|gdi] [--translator echo]
  options: [--helper EXE] [--fakewow EXE] [--out DIR] [--fakewow-selfcheck] [--done-within-s 5]
           [--helper-arg ARG]... (extra Helper arguments, for example --helper-arg=--backend=cpu)
           [--fakewow-arg ARG]... (extra fakewow arguments, for example --fakewow-arg=--gamma=0.45)

The Helper runs with --no-overlay (cards are rendered in memory and reported in --diag-json; no
window is ever shown), --no-clipboard (it never touches the clipboard; --no-overlay implies it, the
flag is given anyway) and --wow-class TPFakeWow. fakewow is placed off every monitor (WGC mode),
so nothing covers the game. GDI mode reads the screen, so fakewow must be visible: it runs only
when no WoW client is running and is reported as deferred otherwise (amendment A1).

Every process started here is waited for or killed before the script returns, and a final
tasklist check reports any of them still alive, by process id (a Translate Companion the user is
running, under either exe name, is not ours and is never reported or touched). Exit code: 0 all
passed, 1 a check failed, 2 a run could not start, 3 deferred (nothing failed).
"""
import argparse
import ctypes
import json
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SCENARIOS = ["basic", "scroll", "fade", "cover", "tabs", "altz", "dropped", "late_start", "gamma22", "conv",
             "presence", "presence_late"]
GAME_EXES = ["WowB.exe", "Wow.exe", "WowClassic.exe", "WowT.exe"]
CREATE_NO_WINDOW = 0x08000000
# (pid, image name) of every process this script started
STARTED = []


def running(image):
    out = subprocess.run(["tasklist", "/FI", f"IMAGENAME eq {image}", "/NH", "/FO", "CSV"],
                         capture_output=True, text=True, creationflags=CREATE_NO_WINDOW).stdout
    return f'"{image.lower()}"' in out.lower()


def pid_running(pid, image):
    out = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/NH", "/FO", "CSV"],
                         capture_output=True, text=True, creationflags=CREATE_NO_WINDOW).stdout
    return f'"{image.lower()}"' in out.lower()


def game_running():
    return any(running(x) for x in GAME_EXES)


class FILETIME(ctypes.Structure):
    _fields_ = [("lo", ctypes.c_uint32), ("hi", ctypes.c_uint32)]


def process_cpu_s(proc):
    """User + kernel CPU seconds of a child process (by its own handle; works after it exited)."""
    k32 = ctypes.windll.kernel32
    c, e, k, u = FILETIME(), FILETIME(), FILETIME(), FILETIME()
    if not k32.GetProcessTimes(ctypes.c_void_p(int(proc._handle)), ctypes.byref(c), ctypes.byref(e),
                               ctypes.byref(k), ctypes.byref(u)):
        return None
    return ((k.hi << 32 | k.lo) + (u.hi << 32 | u.lo)) / 1e7


def stop(proc, name, notes):
    if proc is not None and proc.poll() is None:
        proc.kill()
        proc.wait(10)
        notes.append(f"{name} killed")


def wait_header(truth, timeout_s):
    t0 = time.time()
    while time.time() - t0 < timeout_s:
        if truth.exists():
            with open(truth, encoding="utf-8") as f:
                line = f.readline()
            if line.strip():
                return json.loads(line)
        time.sleep(0.05)
    return None


def run_one(name, a, out):
    lua = HERE / "scenarios" / f"{name}.lua"
    scen = out / f"{name}.json"
    tag = f"{name}.{a.capture}"
    truth = out / f"{tag}.truth.jsonl"
    diag = out / f"{tag}.diag.jsonl"
    report = out / f"{tag}.report.json"
    logs = out / "logs" / tag
    result = {"scenario": name, "capture": a.capture, "translator": a.translator}
    if a.capture == "gdi" and game_running():
        result.update(status="deferred", why="GDI mode needs fakewow on screen; a WoW client is running")
        return result
    r = subprocess.run(["node", str(HERE / "make_scenario.js"), str(lua), str(scen)], capture_output=True, text=True,
                       cwd=REPO, creationflags=CREATE_NO_WINDOW)
    if r.returncode != 0:
        result.update(status="error", why="make_scenario failed", detail=r.stderr.strip()[-400:])
        return result
    for p in (truth, diag, report):
        if p.exists():
            p.unlink()
    logs.mkdir(parents=True, exist_ok=True)
    fw_cmd = [a.fakewow, "--scenario", str(scen), "--truth", str(truth),
              "--place", "onscreen" if a.capture == "gdi" else "offscreen"]
    if a.fakewow_selfcheck:
        fw_cmd += ["--selfcheck", "--wgc-check"]
    fw_cmd += a.fakewow_arg
    notes = []
    fw = helper = None
    fw_out = open(out / f"{tag}.fakewow.json", "w", encoding="utf-8")
    try:
        fw = subprocess.Popen(fw_cmd, stdout=fw_out, stderr=subprocess.PIPE, creationflags=CREATE_NO_WINDOW)
        STARTED.append((fw.pid, Path(a.fakewow).name))
        header = wait_header(truth, 15)
        if header is None:
            result.update(status="error", why="fakewow wrote no truth header", detail=fw.stderr.read().decode()[-400:]
                          if fw.poll() is not None else "")
            return result
        play_start = header["playStart"] / 1000.0
        end = play_start + (header["durationMs"] + header["tailMs"]) / 1000.0
        start_at = play_start + header.get("helperStartMs", 0) / 1000.0 - (0.0 if header.get("helperStartMs") else 2.0)
        if start_at > time.time():
            time.sleep(start_at - time.time())
        exit_after = max(5.0, end - time.time() + 1.0)
        h_cmd = [a.helper, "--wow-class", header.get("class", "TPFakeWow"), "--translator", a.translator,
                 "--capture", a.capture, "--no-overlay", "--no-clipboard", "--no-tray", "--diag-json", str(diag),
                 "--log-dir", str(logs), "--exit-after", f"{exit_after:.1f}"] + a.helper_arg
        t_h = time.time()
        helper = subprocess.Popen(h_cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                  creationflags=CREATE_NO_WINDOW)
        STARTED.append((helper.pid, Path(a.helper).name))
        try:
            helper.wait(exit_after + 20)
        except subprocess.TimeoutExpired:
            notes.append("helper did not exit by itself")
        wall = time.time() - t_h
        cpu = process_cpu_s(helper)
        stop(helper, "helper", notes)
        result["helper_exit"] = helper.returncode
        try:
            fw.wait(max(5.0, end - time.time() + 15.0))
        except subprocess.TimeoutExpired:
            notes.append("fakewow did not exit by itself")
        stop(fw, "fakewow", notes)
        result["fakewow_exit"] = fw.returncode
        cpu_pct = None if cpu is None else round(cpu / max(wall, 0.001) * 100.0, 2)
        result["helper_cpu_pct"] = cpu_pct
        if not diag.exists():
            result.update(status="error", why="the Helper wrote no diag file", notes=notes)
            return result
        cmd = [sys.executable, "-I", str(HERE / "check.py"), "--truth", str(truth), "--diag", str(diag),
               "--report", str(report), "--done-within-s", str(a.done_within_s)]
        if cpu_pct is not None:
            cmd += ["--helper-cpu-pct", str(cpu_pct)]
        c = subprocess.run(cmd, capture_output=True, text=True, creationflags=CREATE_NO_WINDOW)
        try:
            result["checks"] = json.loads(c.stdout.strip().splitlines()[-1])["checks"]
        except (ValueError, IndexError, KeyError):
            result["check_output"] = (c.stdout + c.stderr)[-400:]
        result["status"] = "pass" if c.returncode == 0 else "fail"
        result["notes"] = notes
        return result
    finally:
        stop(helper, "helper", notes)
        stop(fw, "fakewow", notes)
        fw_out.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--scenario", choices=SCENARIOS)
    g.add_argument("--all", action="store_true")
    ap.add_argument("--capture", choices=["wgc", "gdi"], default="wgc")
    ap.add_argument("--translator", choices=["echo", "llama"], default="echo")
    ap.add_argument("--helper", default=str(REPO / "helper" / "app" / "target" / "release" / "WoWTranslateCompanion.exe"))
    ap.add_argument("--fakewow", default=str(REPO / "helper" / "testkit" / "target" / "release" / "fakewow.exe"))
    ap.add_argument("--out", default=str(HERE / "out"))
    ap.add_argument("--fakewow-selfcheck", action="store_true")
    ap.add_argument("--done-within-s", type=float, default=5.0)
    ap.add_argument("--helper-arg", action="append", default=[])
    ap.add_argument("--fakewow-arg", action="append", default=[])
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    for exe in (a.helper, a.fakewow):
        if not Path(exe).exists():
            print(json.dumps({"status": "error", "why": f"missing {exe}"}))
            return 2
    results = [run_one(n, a, out) for n in (SCENARIOS if a.all else [a.scenario])]
    for r in results:
        print(json.dumps(r))
    leftovers = [f"{image} (pid {pid})" for pid, image in STARTED if pid_running(pid, image)]
    statuses = {r["status"] for r in results}
    print(json.dumps({"summary": {s: sum(1 for r in results if r["status"] == s) for s in sorted(statuses)},
                      "leftover_processes": leftovers}))
    if leftovers or "error" in statuses:
        return 2 if "fail" not in statuses else 1
    if "fail" in statuses:
        return 1
    return 3 if statuses == {"deferred"} else 0


if __name__ == "__main__":
    sys.exit(main())
