"""Exercise the packaged native app with a loopback Dr.COM fixture and real Keychain.

Only ephemeral credentials are used. Each run has its own data directory and
Keychain service. GUI evidence comes from the running AppKit view, not a mockup.
"""
import contextlib
import http.server
import json
import os
import pathlib
import platform
import plistlib
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import uuid
import zlib

APP = pathlib.Path(sys.argv[1]).resolve()
EXE = APP / "Contents/MacOS/snnulogin"
OUT = pathlib.Path("dist/macos-validation").resolve()
OUT.mkdir(parents=True, exist_ok=True)
bundle_info = plistlib.loads((APP / "Contents/Info.plist").read_bytes())
app_version = subprocess.check_output([str(EXE), "--version"], text=True).strip()
assert app_version == f"SNNU-AutoLogin v{bundle_info['CFBundleShortVersionString']}"
assert bundle_info["CFBundleVersion"] == bundle_info["CFBundleShortVersionString"]
(OUT / "version.json").write_text(json.dumps({
    "cli": app_version,
    "bundle_version": bundle_info["CFBundleVersion"],
    "short_version": bundle_info["CFBundleShortVersionString"],
}), encoding="utf-8")
LOCK = threading.RLock()
STATE = {"mode": "offline", "posts": 0, "logoffs": 0, "status_requests": 0}


def mode(value):
    with LOCK:
        STATE.update(mode=value, posts=0, logoffs=0, status_requests=0)


class Portal(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, text, code=200):
        payload = text.encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        with contextlib.suppress(BrokenPipeError):
            self.wfile.write(payload)

    def do_GET(self):
        with LOCK:
            current = STATE["mode"]
            if self.path == "/__fixture":
                self.reply(json.dumps(STATE))
            elif self.path == "/generate_204":
                self.reply("", 204)
            elif current == "unreachable":
                self.reply("temporarily unavailable", 503)
            elif self.path.startswith("/snnuportal/logoff"):
                STATE.update(mode="offline", logoffs=STATE["logoffs"] + 1)
                self.reply('<input name="account"><input name="password">')
            elif self.path.startswith("/snnuportal/userstatus.jsp"):
                STATE["status_requests"] += 1
                if current in ("online", "online-other"):
                    account = "20249999" if current == "online-other" else "20241234"
                    self.reply(f"当前登录账号：<span>{account}@mobile</span><br>当前IP：10.0.0.2<br>登录时间：2026-10-27 08:00:00<br>")
                else:
                    self.reply('<input name="account"><input name="password">')
            else:
                self.reply('<input name="account"><input name="password">')

    def do_POST(self):
        payload = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        with LOCK:
            if self.path == "/__fixture":
                mode(payload.decode())
                self.reply("ok")
                return
            # Intentionally never log the form body.
            STATE["posts"] += 1
            if STATE["mode"] == "credential":
                self.reply("<p>账号或密码错误</p>")
            elif STATE["mode"] == "account-state":
                self.reply("<p>您的账号已欠费</p>")
            else:
                STATE["mode"] = "online"
                self.reply("当前登录账号：<span>20241234@mobile</span><br>当前IP：10.0.0.2<br>")


def wait_until(check, seconds=20, process=None):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"Native app exited early: {process.returncode}")
        if check():
            return
        time.sleep(0.1)
    raise TimeoutError("Native validation timed out")


def command(env, *args):
    subprocess.run([str(EXE), *args], env=env, check=True, timeout=90)


def run_app(env, data, *args):
    ready = data / "ready"
    ready.unlink(missing_ok=True)
    log = (data / "native-output.txt").open("wb")
    start = time.monotonic()
    process = subprocess.Popen([str(EXE), *args, "--ready-file", str(ready)], env=env, stdout=log, stderr=log)
    try:
        wait_until(ready.exists, process=process)
    except Exception:
        process.kill()
        process.wait()
        log.close()
        print((data / "native-output.txt").read_text(errors="replace"))
        raise
    return process, log, (time.monotonic() - start) * 1000


def finish(process, log, seconds):
    try:
        code = process.wait(timeout=seconds)
        if code:
            raise RuntimeError(f"Native app failed: {code}")
    except Exception:
        log.flush()
        print(pathlib.Path(log.name).read_text(errors="replace"), flush=True)
        raise
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        log.close()


def state_file(data):
    try:
        return json.loads((data / "state.json").read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def read_png(path):
    """Read native 8-bit RGB/RGBA screenshots without an imaging dependency."""
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    position, compressed = 8, bytearray()
    while position < len(data):
        length = struct.unpack_from(">I", data, position)[0]
        kind = data[position + 4:position + 8]
        payload = data[position + 8:position + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", payload)
            assert depth == 8 and color in (2, 6) and interlace == 0
        elif kind == b"IDAT":
            compressed.extend(payload)
        position += length + 12
    channels = 4 if color == 6 else 3
    stride, raw, rows = width * channels, zlib.decompress(compressed), []
    previous = bytearray(stride)
    position = 0
    for _ in range(height):
        filtering = raw[position]
        row = bytearray(raw[position + 1:position + stride + 1])
        position += stride + 1
        for x in range(stride):
            left = row[x - channels] if x >= channels else 0
            up = previous[x]
            upper_left = previous[x - channels] if x >= channels else 0
            if filtering == 0:
                prediction = 0
            elif filtering == 1:
                prediction = left
            elif filtering == 2:
                prediction = up
            elif filtering == 3:
                prediction = (left + up) // 2
            elif filtering == 4:
                value = left + up - upper_left
                distances = (abs(value - left), abs(value - up), abs(value - upper_left))
                prediction = (left, up, upper_left)[distances.index(min(distances))]
            else:
                raise AssertionError(f"Unsupported PNG filter: {filtering}")
            row[x] = (row[x] + prediction) & 255
        rows.append(row)
        previous = row
    return width, height, channels, rows


def check_desktop_appearance(screenshots):
    """Golden design assertions from the shipped Windows UI, not Cocoa defaults."""
    expected_heights = {"home": 472, "settings": 448, "account": 592,
                        "network": 596, "features": 724, "help": 596, "about": 424}
    checks = {
        "home": [(4, 4, 0xF6F7F9), (142, 140, 0xE8F4E5), (32, 338, 0xFFFFFF)],
        "settings": [(32, 84, 0xFFFFFF), (312, 203, 0x2563EB)],
        "account": [(32, 148, 0xFFFFFF), (30, 318, 0xEBEFF3), (143, 326, 0xFFFFFF), (40, 446, 0x2563EB)],
        "network": [(32, 80, 0xFFFFFF), (46, 102, 0x167A4A), (32, 200, 0xFFFFFF), (40, 440, 0x2563EB)],
        "features": [(32, 108, 0xFFFFFF), (32, 320, 0xFFFFFF), (32, 532, 0xFFFFFF)],
        "help": [(32, 80, 0xFFFFFF)],
        "about": [],
    }
    results = {}
    frames = {
        "home": {1: (292, 16, 64, 36), 10: (122, 72, 136, 136)},
        "settings": {2: (24, 76, 332, 76), 11: (24, 172, 332, 62),
                     12: (24, 246, 332, 62), 4: (24, 328, 332, 42), 3: (24, 382, 332, 42)},
        "account": {201: (38, 151, 304, 24), 202: (38, 237, 304, 24),
                    16: (286, 194, 70, 32), 30: (28, 320, 106, 34),
                    31: (137, 320, 106, 34), 32: (246, 320, 106, 34),
                    13: (24, 438, 332, 44), 14: (24, 536, 332, 32)},
        "network": {15: (24, 432, 332, 44)},
    }
    for page, expected_height in expected_heights.items():
        layout = json.loads((screenshots / f"{page}.layout.json").read_text())
        assert layout["width"] == 380 and layout["height"] == expected_height, (page, layout)
        controls = {control["id"]: control for control in layout["controls"]}
        for identifier, expected in frames.get(page, {}).items():
            actual = tuple(controls[identifier][key] for key in ("x", "y", "width", "height"))
            assert actual == expected, (page, identifier, actual, expected)
        if page != "home":
            assert tuple(controls[40][key] for key in ("x", "y", "width", "height")) == (16, 16, 64, 36)
        width, height, channels, rows = read_png(screenshots / f"{page}.png")
        for x, y, expected in checks[page]:
            px, py = int(x * width / 380), int(y * height / expected_height)
            actual = tuple(rows[py][px * channels:px * channels + 3])
            rgb = ((expected >> 16) & 255, (expected >> 8) & 255, expected & 255)
            assert all(abs(a - b) <= 2 for a, b in zip(actual, rgb)), (page, x, y, actual, rgb)
        # Validate rendered text as well as cards. AppKit's center/right enum
        # values differ by architecture; geometry alone cannot catch that bug.
        if page in ("home", "settings", "account"):
            left, top, right, bottom = ((24, 228, 356, 266) if page == "home"
                                        else (90, 20, 290, 48))
            ink_x = []
            for py in range(int(top * height / expected_height), int(bottom * height / expected_height)):
                for px in range(int(left * width / 380), int(right * width / 380)):
                    rgb = rows[py][px * channels:px * channels + 3]
                    if max(rgb) < 90:
                        ink_x.append(px * 380 / width)
            assert ink_x, (page, "missing title")
            center = (min(ink_x) + max(ink_x)) / 2
            assert abs(center - 190) <= 4, (page, "title must be centered", center)
        results[page] = {"width": 380, "height": expected_height, "windows_palette_and_cards": True}
    (screenshots.parent / "appearance.json").write_text(json.dumps(results, indent=2))


server = http.server.ThreadingHTTPServer(("127.0.0.1", 8601), Portal)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix="snnu-macos-validation-") as temp:
        base = pathlib.Path(temp)
        service = "io.github.shixiang-liu.SNNU-AutoLogin.test." + uuid.uuid4().hex
        env = os.environ.copy()
        env.update(SNNU_PORTAL_HOST="127.0.0.1", SNNU_KEYCHAIN_SERVICE=service)

        def profile(name):
            data = base / name
            data.mkdir()
            env["SNNU_DATA_DIR"] = str(data)
            return data

        try:
            data = profile("integration")
            command(env, "--fixture-check")
            (OUT / "integration.json").write_bytes((data / "integration.json").read_bytes())

            data = profile("ui")
            mode("online")
            command(env, "--fixture-setup")
            screenshots = OUT / "screenshots"
            process, log, startup = run_app(env, data, "--smoke-test", str(screenshots))
            # A second executable instance must notify the running app and exit.
            command(env, "--daemon")
            finish(process, log, 35)
            assert (screenshots / "smoke.json").exists()
            for name in ("home", "settings", "account", "network", "features", "help", "about"):
                assert (screenshots / f"{name}.png").stat().st_size > 1000
            check_desktop_appearance(screenshots)

            # Run the actual worker, drop its session, and confirm it reconnects.
            data = profile("reconnect")
            mode("offline")
            command(env, "--fixture-setup")
            process, log, _ = run_app(env, data, "--daemon")
            try:
                wait_until(lambda: state_file(data).get("self_login_ok"), process=process)
                first = state_file(data)["last_ok_ts"]
                mode("offline")
                wait_until(lambda: STATE["posts"] > 0 and state_file(data).get("last_ok_ts", 0) > first, seconds=25, process=process)
                # Stop via the shipped CLI. Exiting must not send portal logoff.
                logoffs = STATE["logoffs"]
                command(env, "--stop")
                finish(process, log, 15)
                assert STATE["logoffs"] == logoffs
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                log.close()
            (OUT / "reconnect.json").write_text(json.dumps({"automatic_login": True, "dropped_session_reconnected": True, "quit_preserves_connection": True}))

            summary = []
            for scenario in ("online-window", "online-background", "waiting-background"):
                data = profile(scenario)
                mode("unreachable" if scenario.startswith("waiting") else "online")
                command(env, "--fixture-setup")
                report = OUT / (scenario + ".json")
                process, log, launch_ms = run_app(env, data, "--measure", scenario, str(report), "60")
                finish(process, log, 95)
                measurement = json.loads(report.read_text())
                samples = measurement["samples"]
                assert len(samples) >= 29
                elapsed = samples[-1]["time"] - samples[0]["time"]
                cpu = (samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]) / elapsed * 100
                row = {
                    "scenario": scenario, "startup_ms": round(launch_ms, 1),
                    "rss_mib": round(statistics.mean(s["rss_bytes"] for s in samples) / 2**20, 2),
                    "footprint_mib": round(statistics.mean(s["physical_footprint_bytes"] for s in samples) / 2**20, 2),
                    "cpu_percent_one_core": round(cpu, 3),
                    "threads": max(s["thread_count"] for s in samples),
                }
                summary.append(row)
                print(json.dumps(row), flush=True)

            app_bytes = sum(path.stat().st_size for path in APP.rglob("*") if path.is_file())
            result = {"architecture": platform.machine(), "macos": platform.mac_ver()[0], "app_bytes": app_bytes,
                      "startup_smoke_ms": round(startup, 1), "measurements": summary,
                      "real_campus_network_tested": False, "physical_sleep_tested": False, "macos_13_tested": False}
            (OUT / "summary.json").write_text(json.dumps(result, indent=2))
            lines = ["# macOS cloud validation", "", f"Host: macOS {result['macos']} / {result['architecture']}",
                     f"App bundle: {app_bytes:,} bytes ({app_bytes / 2**20:.2f} MiB).", "",
                     "Passed: shared core regression, fixture authentication/errors/pause/cancellation/account protection, Keychain roundtrip/deletion, config secrecy, diagnostic redaction, native window/menu, close/reopen, repeated launch, actual worker reconnect, CLI stop without logout.", "",
                     "| Scenario | Launch ms | RSS MiB | Physical footprint MiB | CPU % (one core) |", "|---|---:|---:|---:|---:|"]
            for row in summary:
                lines.append(f"| {row['scenario']} | {row['startup_ms']} | {row['rss_mib']} | {row['footprint_mib']} | {row['cpu_percent_one_core']} |")
            lines += ["", "Measurements: 10 seconds warmup, 60 seconds sampling, 2 seconds interval; loopback fixture, release/ad-hoc-signed app.",
                      "Background scenarios open and close the real window before sampling. Raw samples include cumulative wakeups.",
                      "Targets: app <= 5 MiB; background physical footprint <= 40 MiB; average CPU < 1%. Targets are evaluated below, not asserted as achieved.",
                      f"App size target: {'met' if app_bytes <= 5*2**20 else 'exceeded'}."]
            for row in summary:
                if row["scenario"] != "online-window":
                    lines.append(f"{row['scenario']} footprint target: {'met' if row['footprint_mib'] <= 40 else 'exceeded'}; CPU target: {'met' if row['cpu_percent_one_core'] < 1 else 'exceeded'}.")
            lines += ["", "Not verified: real SNNU campus authentication, physical MacBook lid/sleep/wake, interactive permission-denial flows, real login after reboot, macOS 13 runtime compatibility.",
                      "Minimum deployment version is 13.0. Unsigned/ad-hoc permission prompts and login-item approval depend on the user's system."]
            (OUT / "VALIDATION.md").write_text("\n".join(lines), encoding="utf-8")
        finally:
            subprocess.run(["security", "delete-generic-password", "-s", service, "-a", "20241234"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
finally:
    server.shutdown()
