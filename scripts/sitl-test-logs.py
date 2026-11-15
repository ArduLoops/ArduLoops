"""Records the DataFlash logs the log-tool tests read, by flying ArduPilot SITL with a known fault.

Each scenario boots a fresh SITL (an X quad, or a plane, at ArduPilot's CMAC test field), sets the
fault in its parameters, flies a short scripted flight over MAVLink, and keeps only the messages the
tests read. The result goes to src-tauri/src/dflog/testdata/sitl/<scenario>.bin, small enough to
commit.

    python scripts/sitl-test-logs.py [scenario ...] [--sitl DIR] [--ardupilot DIR] [--speedup N]

--sitl       folder with ArduCopter.exe and ArduPlane.exe (and their DLLs on Windows), default:
             the app's own SITL download in %LOCALAPPDATA%/ArduLoops/sitl, or the binaries on PATH.
--ardupilot  an ArduPilot checkout, for its SITL default parameters, default: ../ardupilot next to
             this repository.

Needs pymavlink. Scenarios: healthy, notch, vibration, ekf_lane, gps_loss, crash, magfit, motors,
pid, plane_pid, sysid.
"""

import argparse
import os
import random
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from pymavlink import mavutil

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "src-tauri" / "src" / "dflog" / "testdata" / "sitl"
PORT = 5790
HOME = "-35.363261,149.165230,584,353"  # ArduPilot's CMAC test field, the SITL default.

# Every log keeps these: formats, units, parameters, text, modes and events.
BASE = {"FMT", "FMTU", "UNIT", "MULT", "PARM", "MSG", "MODE", "EV", "ERR", "ARM", "VER"}

# The SITL defaults ArduPilot's own tests start from, and the frame each vehicle flies.
VEHICLES = {
    "copter": {"exe": "ArduCopter", "model": "x", "defaults": "Tools/autotest/default_params/copter.parm",
               "params": {"FRAME_CLASS": 1, "FRAME_TYPE": 1}},
    "plane": {"exe": "ArduPlane", "model": "plane", "defaults": "Tools/autotest/models/plane.parm", "params": {}},
}

# Realistic noise for every flight: a little sensor noise and a light, gusty wind.
COMMON = {
    "LOG_DISARMED": 0,
    "SIM_GYR1_RND": 0.05,
    "SIM_ACC1_RND": 0.05,
    "SIM_WIND_SPD": 2,
    "SIM_WIND_TURB": 1,
}

AUDIT_KEEP = {"BAT", "GPS", "MOT", "PM", "POWR", "RCOU", "VIBE", "XKF4", "XKFS", "WDOG"}

SCENARIOS = {
    # Nothing wrong: a throttle notch on the motor line at hover (FREQ and REF as the review of the
    # notch log proposes), its harmonic, failsafes set. The control case: a fault found here is wrong.
    "healthy": {
        "params": {
            "SIM_VIB_MOT_MAX": 280,
            "SIM_VIB_MOT_MULT": 4,
            "INS_LOG_BAT_MASK": 1,
            "INS_LOG_BAT_OPT": 4,
            "INS_HNTCH_ENABLE": 1,
            "INS_HNTCH_MODE": 1,
            "INS_HNTCH_FREQ": 158,
            "INS_HNTCH_REF": 0.37,
            "INS_HNTCH_BW": 79,
            "INS_HNTCH_HMNCS": 3,
            "INS_GYRO_FILTER": 40,
            "BATT_FS_LOW_ACT": 2,
            "BATT_FS_CRT_ACT": 1,
            "BATT_LOW_VOLT": 10.5,
            "BATT_CRT_VOLT": 10.0,
            "LOG_FILE_RATEMAX": 50,
        },
        "keep": AUDIT_KEEP | {"ISBH", "ISBD", "CTUN", "RATE", "FTN1", "FTN2", "FTNS", "ESC"},
        "flight": "hover_throttle",
    },
    # Motor noise that follows throttle, a gyro batch before and after the filters, and a throttle
    # notch that is set lower than the motor line.
    "notch": {
        "params": {
            "SIM_VIB_MOT_MAX": 280,
            "SIM_VIB_MOT_MULT": 4,
            "INS_LOG_BAT_MASK": 1,
            "INS_LOG_BAT_OPT": 4,
            "INS_HNTCH_ENABLE": 1,
            "INS_HNTCH_MODE": 1,
            "INS_HNTCH_REF": 0.3,
            "INS_HNTCH_FREQ": 60,
            "INS_HNTCH_BW": 30,
            "INS_HNTCH_HMNCS": 1,
            "INS_GYRO_FILTER": 40,
        },
        "keep": {"ISBH", "ISBD", "CTUN", "RATE", "FTN1", "FTN2", "FTNS", "ESC", "RCOU", "VIBE", "BAT"},
        "flight": "hover_throttle",
    },
    # Heavy vibration near 95 Hz on the accelerometers of both IMUs (SIM_ACCn_RND is its amplitude).
    "vibration": {
        "params": {"SIM_VIB_FREQ_X": 90, "SIM_VIB_FREQ_Y": 95, "SIM_VIB_FREQ_Z": 100,
                   "SIM_ACC1_RND": 50, "SIM_ACC2_RND": 50, "LOG_FILE_RATEMAX": 25},
        "keep": AUDIT_KEEP,
        "flight": "hover",
    },
    # An accelerometer bias on the first IMU in flight: EKF3 moves to the second lane.
    "ekf_lane": {
        "params": {},
        "keep": AUDIT_KEEP,
        "flight": "lane_switch",
    },
    # GPS lost in Loiter: EKF failsafe lands without position.
    "gps_loss": {
        "params": {"FS_EKF_ACTION": 1},
        "keep": AUDIT_KEEP,
        "flight": "gps_loss",
    },
    # Motor 1 stops in a hover: the copter falls, saturates its motors, and the crash check disarms.
    "crash": {
        "params": {"SIM_ENGINE_FAIL": 1, "SIM_ENGINE_MUL": 1, "LOG_FILE_RATEMAX": 50},
        "keep": AUDIT_KEEP | {"ATT"},
        "flight": "motor_out",
    },
    # Compass offsets the board does not know about, then turns and banks for coverage.
    "magfit": {
        "params": {
            "SIM_MAG1_OFS_X": 120,
            "SIM_MAG1_OFS_Y": -80,
            "SIM_MAG1_OFS_Z": 60,
            "COMPASS_OFS_X": 0,
            "COMPASS_OFS_Y": 0,
            "COMPASS_OFS_Z": 0,
            "COMPASS_USE2": 0,
            "COMPASS_USE3": 0,
            "ARMING_CHECK": 0,
        },
        "keep": {"ORGN", "POS", "GPS", "AHR2", "ATT", "XKF1", "BAT", "RATE", "CTUN", "MAG"},
        "flight": "turns",
    },
    # Motor 1 (front right on Quad X) gives 8% less thrust, so it runs harder than the others.
    "motors": {
        "params": {"SIM_ENGINE_MUL": 0.92, "SIM_ENGINE_FAIL": 1, "LOG_FILE_RATEMAX": 25},
        "keep": {"ESC", "RCOU"},
        "flight": "hover",
    },
    # Stick doublets of varied length on each axis in AltHold, logged with the rate PIDs.
    "pid": {
        # Rate PIDs at 100 Hz: unbroken in SITL on any computer, and small enough to commit.
        "params": {"LOG_BITMASK": 1 | 2 | 4 | 4096, "LOG_FILE_RATEMAX": 100, "INS_LOG_BAT_MASK": 0},
        # The PID messages carry Tar and Act; RATE would repeat them.
        "keep": {"PIDR", "PIDP", "PIDY"},
        "flight": "doublets",
        # Faster than real time, SITL on Windows drops stretches of the 100 Hz PID records.
        "speedup": 1,
    },
    # ArduCopter's System ID mode: a chirp on the roll, then the pitch angle input, from Stabilize, the
    # way a small room allows. The same settings are the indoor test on a real vehicle.
    "sysid": {
        "params": {
            "LOG_BITMASK": 1 | 2 | 4 | 4096, "LOG_FILE_RATEMAX": 100, "INS_LOG_BAT_MASK": 0,
            "SID_AXIS": 1, "SID_MAGNITUDE": 5, "SID_F_START_HZ": 1, "SID_F_STOP_HZ": 30,
            "SID_T_FADE_IN": 5, "SID_T_REC": 20, "SID_T_FADE_OUT": 2,
        },
        "keep": {"PIDR", "PIDP", "PIDY", "ATT"},
        "flight": "system_id",
        "speedup": 1,
    },
    # A plane in FBWA: aileron and elevator doublets, the fixed-wing rate loops at 100 Hz.
    "plane_pid": {
        "vehicle": "plane",
        "params": {"LOG_BITMASK": 1 | 2 | 4 | (1 << 20), "LOG_FILE_RATEMAX": 100},
        "keep": {"RATE", "PIDR", "PIDP", "PIDY", "ATT"},
        "flight": "plane_doublets",
        "speedup": 1,
    },
}


class Sim:
    def __init__(self, exe: Path, vehicle: dict, defaults: Path, params: dict, speedup: int):
        self.dir = Path(tempfile.mkdtemp(prefix="arduloops-sitl-"))
        merged = {**vehicle["params"], **COMMON, **params}
        lines = defaults.read_text().splitlines() + [f"{k} {v}" for k, v in merged.items()]
        (self.dir / "defaults.parm").write_text("\n".join(lines) + "\n")
        env = dict(os.environ)
        env["PATH"] = str(exe.parent) + os.pathsep + env.get("PATH", "")
        env["HOME"] = str(self.dir)
        self.proc = subprocess.Popen(
            [str(exe), f"-M{vehicle['model']}", f"-O{HOME}", f"-s{speedup}", "--wipe", "--defaults", "defaults.parm",
             "--serial0", f"tcp:{PORT}"],
            cwd=self.dir, env=env, stdout=open(self.dir / "sitl.log", "w"), stderr=subprocess.STDOUT,
        )
        # SITL's TCP serial port takes one client, so the first connection must be the real one.
        deadline = time.time() + 30
        while True:
            time.sleep(1)
            if self.proc.poll() is not None or time.time() > deadline:
                raise RuntimeError(f"SITL did not start; see {self.dir / 'sitl.log'}")
            try:
                self.m = mavutil.mavlink_connection(f"tcp:127.0.0.1:{PORT}", source_system=255, retries=1)
                break
            except Exception:
                continue
        self.m.wait_heartbeat(timeout=30)
        self.m.mav.request_data_stream_send(self.m.target_system, self.m.target_component, mavutil.mavlink.MAV_DATA_STREAM_ALL, 10, 1)
        self.boot_ms = 0
        self.sticks = None
        self.sticks_ms = 0

    # --- time is the vehicle's, so a run at any speedup records the same flight ---
    def pump(self):
        msg = self.m.recv_match(blocking=True, timeout=1)
        if msg is not None and hasattr(msg, "time_boot_ms"):
            self.boot_ms = max(self.boot_ms, msg.time_boot_ms)
        # An RC override lapses after RC_OVERRIDE_TIME (3 s): send the held sticks again every 200 ms.
        if self.sticks is not None and self.boot_ms - self.sticks_ms >= 200:
            self.m.mav.rc_channels_override_send(self.m.target_system, self.m.target_component, *self.sticks, 0, 0, 0, 0)
            self.sticks_ms = self.boot_ms
        if msg is not None and msg.get_type() == "STATUSTEXT":
            print("   ", msg.text)
        return msg

    def wait(self, seconds: float, each=None):
        end = self.boot_ms + seconds * 1000
        while self.boot_ms < end:
            self.pump()
            if each:
                each((self.boot_ms - (end - seconds * 1000)) / 1000)

    def until(self, test, seconds: float, what: str):
        end = self.boot_ms + seconds * 1000
        while self.boot_ms < end or self.boot_ms == 0:
            msg = self.pump()
            if msg is not None and test(msg):
                return msg
        raise RuntimeError(f"timed out: {what}")

    def param(self, name: str, value: float):
        self.m.mav.param_set_send(self.m.target_system, self.m.target_component, name.encode(), float(value), mavutil.mavlink.MAV_PARAM_TYPE_REAL32)
        self.until(lambda m: m.get_type() == "PARAM_VALUE" and m.param_id == name, 10, f"param {name}")

    def mode(self, name: str):
        self.m.set_mode(self.m.mode_mapping()[name])
        self.until(lambda m: m.get_type() == "HEARTBEAT" and m.custom_mode == self.m.mode_mapping()[name], 10, f"mode {name}")

    def rc(self, roll=1500, pitch=1500, throttle=1500, yaw=1500):
        self.sticks = (roll, pitch, throttle, yaw)
        self.sticks_ms = self.boot_ms
        self.m.mav.rc_channels_override_send(self.m.target_system, self.m.target_component, *self.sticks, 0, 0, 0, 0)

    def takeoff(self, alt: float = 10):
        # EKF and GPS settle on the ground; arming is retried until the pre-arm checks pass.
        self.until(lambda m: m.get_type() == "GPS_RAW_INT" and m.fix_type >= 3, 120, "GPS fix")
        self.mode("GUIDED")
        for _ in range(60):
            self.m.mav.command_long_send(self.m.target_system, self.m.target_component, mavutil.mavlink.MAV_CMD_COMPONENT_ARM_DISARM, 0, 1, 0, 0, 0, 0, 0, 0)
            self.wait(1)
            if self.m.motors_armed():
                break
        else:
            raise RuntimeError("did not arm")
        self.m.mav.command_long_send(self.m.target_system, self.m.target_component, mavutil.mavlink.MAV_CMD_NAV_TAKEOFF, 0, 0, 0, 0, 0, 0, 0, alt)
        self.until(lambda m: m.get_type() == "GLOBAL_POSITION_INT" and m.relative_alt > (alt - 1) * 1000, 60, "takeoff")

    def land(self):
        self.rc()
        self.mode("LAND")
        self.sticks = None
        self.until(lambda m: m.get_type() == "HEARTBEAT" and not (m.base_mode & mavutil.mavlink.MAV_MODE_FLAG_SAFETY_ARMED), 120, "landed")

    def close(self) -> Path:
        # The log is written through a buffer: give it time to reach the file before SITL stops.
        try:
            self.wait(5)
        except Exception:
            pass
        self.m.close()
        self.proc.terminate()
        try:
            self.proc.wait(10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        logs = sorted((self.dir / "logs").glob("*.BIN"), key=lambda p: p.name)
        if not logs:
            raise RuntimeError(f"no log in {self.dir / 'logs'}")
        return logs[-1]


def hover_throttle(s: Sim):
    s.takeoff(10)
    s.mode("ALT_HOLD")
    s.rc()
    s.wait(8)
    # Climb and descend, so the motor line moves with throttle.
    for throttle in (1750, 1500, 1300, 1500, 1800, 1500):
        s.rc(throttle=throttle)
        s.wait(3)
    s.wait(5)
    s.land()


def lane_switch(s: Sim):
    s.takeoff(10)
    s.mode("LOITER")
    s.rc()
    s.wait(10)
    s.param("SIM_ACC1_BIAS_Z", 1.5)
    s.wait(20)
    s.param("SIM_ACC1_BIAS_Z", 0)
    s.land()


def gps_loss(s: Sim):
    s.takeoff(15)
    s.mode("LOITER")
    s.rc()
    s.wait(10)
    s.param("SIM_GPS1_ENABLE", 0)
    # EKF failsafe switches to LAND; wait for the landing.
    s.until(lambda m: m.get_type() == "HEARTBEAT" and not (m.base_mode & mavutil.mavlink.MAV_MODE_FLAG_SAFETY_ARMED), 180, "landed after GPS loss")


def turns(s: Sim):
    s.takeoff(15)
    s.mode("ALT_HOLD")
    # Yaw turns all the time, with full banks in each direction, so the compass sees many attitudes.
    s.rc(yaw=1850)
    s.wait(10)
    for _ in range(3):
        for roll, pitch in ((1950, 1500), (1500, 1950), (1050, 1500), (1500, 1050), (1900, 1900), (1100, 1100)):
            s.rc(roll=roll, pitch=pitch, yaw=1850)
            s.wait(1.5)
            s.rc(yaw=1850)
            s.wait(1.5)
    s.land()


def hover(s: Sim):
    s.takeoff(10)
    s.mode("ALT_HOLD")
    s.rc()
    s.wait(30)
    s.land()


def doublets(s: Sim):
    s.takeoff(15)
    s.mode("ALT_HOLD")
    s.rc()
    s.wait(3)
    rnd = random.Random(7)
    for axis, count in (("roll", 8), ("pitch", 8), ("yaw", 4)):
        for _ in range(count):
            size = rnd.choice((150, 220, 300))
            hold = rnd.choice((0.25, 0.4, 0.6, 0.9))
            for sign in (1, -1):
                s.rc(**{axis: 1500 + sign * size})
                s.wait(hold)
            s.rc()
            s.wait(rnd.choice((0.5, 0.8, 1.2)))
        s.wait(2)
    s.land()


def motor_out(s: Sim):
    s.takeoff(15)
    s.mode("ALT_HOLD")
    s.rc()
    s.wait(6)
    s.param("SIM_ENGINE_MUL", 0)
    s.wait(12)
    # On the ground after the fall: disarm by force (21196) if the crash check has not.
    s.sticks = None
    s.m.mav.command_long_send(s.m.target_system, s.m.target_component, mavutil.mavlink.MAV_CMD_COMPONENT_ARM_DISARM, 0, 0, 21196, 0, 0, 0, 0, 0)
    s.wait(2)


def plane_doublets(s: Sim):
    s.until(lambda m: m.get_type() == "GPS_RAW_INT" and m.fix_type >= 3, 120, "GPS fix")
    s.wait(10)
    s.mode("TAKEOFF")
    for _ in range(60):
        s.m.mav.command_long_send(s.m.target_system, s.m.target_component, mavutil.mavlink.MAV_CMD_COMPONENT_ARM_DISARM, 0, 1, 0, 0, 0, 0, 0, 0)
        s.wait(1)
        if s.m.motors_armed():
            break
    else:
        raise RuntimeError("did not arm")
    s.until(lambda m: m.get_type() == "GLOBAL_POSITION_INT" and m.relative_alt > 40_000, 120, "takeoff")
    s.rc(throttle=1700)
    s.mode("FBWA")
    s.wait(3)
    rnd = random.Random(11)
    for axis, count in (("roll", 8), ("pitch", 8)):
        for _ in range(count):
            size = rnd.choice((200, 300, 400))
            hold = rnd.choice((0.3, 0.5, 0.8, 1.2))
            for sign in (1, -1):
                s.rc(throttle=1700, **{axis: 1500 + sign * size})
                s.wait(hold)
            s.rc(throttle=1700)
            s.wait(rnd.choice((1.0, 1.5, 2.0)))
    s.sticks = None
    s.mode("RTL")
    s.wait(5)


def system_id(s: Sim):
    s.takeoff(5)
    s.mode("STABILIZE")
    # Stabilize has manual throttle: the script holds 5 m as a pilot would.
    state = {"alt": 5.0, "climb": 0.0}

    def hold(_):
        msg = s.m.messages.get("GLOBAL_POSITION_INT")
        if msg is not None:
            state["alt"], state["climb"] = msg.relative_alt / 1000, -msg.vz / 100
        throttle = int(1500 + 60 * (5 - state["alt"]) - 80 * state["climb"])
        s.sticks = (1500, 1500, max(1300, min(1700, throttle)), 1500)

    s.rc()
    s.wait(5, hold)
    for axis in (1, 2):
        s.param("SID_AXIS", axis)
        s.mode("SYSTEMID")
        s.wait(29, hold)
        s.mode("STABILIZE")
        s.wait(3, hold)
    s.land()


FLIGHTS = {f.__name__: f for f in (hover_throttle, lane_switch, gps_loss, turns, hover, doublets, motor_out, plane_doublets, system_id)}


SIZES = {"b": 1, "B": 1, "M": 1, "h": 2, "H": 2, "c": 2, "C": 2, "g": 2, "i": 4, "I": 4, "f": 4, "e": 4,
         "E": 4, "L": 4, "n": 4, "d": 8, "q": 8, "Q": 8, "N": 16, "Z": 64, "a": 64}


def text_field(fmt: str, columns: list, name: str):
    """Offset and size of a column in a record body, or None."""
    offset = 0
    for char, column in zip(fmt, columns):
        if column == name:
            return offset, SIZES[char]
        offset += SIZES[char]
    return None


def trim(src: Path, dst: Path, keep: set):
    """Writes only the records of `keep` (and BASE) messages, with every FMT so the file stays valid.

    SITL on a computer without /etc/machine-id names itself after the host in a MSG at boot: that
    text becomes "sitl", so no computer name goes into the repository.
    """
    host = socket.gethostname().encode().lower()
    data = src.read_bytes()
    lengths, names = {128: 89}, {128: "FMT"}
    msg_kind, msg_at = None, None
    out = bytearray()
    i = 0
    while i + 3 <= len(data):
        if data[i] != 0xA3 or data[i + 1] != 0x95:
            i += 1
            continue
        kind = data[i + 2]
        size = lengths.get(kind)
        if size is None or i + size > len(data):
            i += 1
            continue
        if kind == 128:
            body = data[i + 3:i + size]
            fmt_type, fmt_len = body[0], body[1]
            lengths[fmt_type] = fmt_len
            names[fmt_type] = body[2:6].rstrip(b"\0").decode("ascii", "replace")
            if names[fmt_type] == "MSG":
                fmt = body[6:22].rstrip(b"\0").decode("ascii")
                columns = body[22:86].rstrip(b"\0").decode("ascii").split(",")
                msg_kind, msg_at = fmt_type, text_field(fmt, columns, "Message")
            out += data[i:i + size]
        elif names.get(kind) in keep or names.get(kind) in BASE:
            record = bytearray(data[i:i + size])
            if kind == msg_kind and msg_at:
                at, width = msg_at
                text = bytes(record[3 + at:3 + at + width]).rstrip(b"\0")
                if host and text.lower() == host:
                    record[3 + at:3 + at + width] = b"sitl".ljust(width, b"\0")
            out += record
        i += size
    dst.write_bytes(bytes(out))


def find_exe(arg: str | None, base: str) -> Path:
    name = f"{base}.exe" if os.name == "nt" else base.lower()
    dirs = [Path(arg)] if arg else []
    if os.environ.get("LOCALAPPDATA"):
        dirs.append(Path(os.environ["LOCALAPPDATA"]) / "ArduLoops" / "sitl")
    for d in dirs:
        if (d / name).exists():
            return d / name
    found = shutil.which(name) or shutil.which(base)
    if found:
        return Path(found)
    sys.exit(f"{name} not found; pass --sitl")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("scenarios", nargs="*", default=list(SCENARIOS))
    ap.add_argument("--sitl")
    ap.add_argument("--ardupilot", default=str(REPO.parent / "ardupilot"))
    ap.add_argument("--speedup", type=int, default=5)
    args = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    for name in args.scenarios:
        spec = SCENARIOS[name]
        vehicle = VEHICLES[spec.get("vehicle", "copter")]
        exe = find_exe(args.sitl, vehicle["exe"])
        defaults = Path(args.ardupilot) / vehicle["defaults"]
        if not defaults.exists():
            sys.exit(f"{defaults} not found; pass --ardupilot")
        print(f"== {name}")
        sim = Sim(exe, vehicle, defaults, spec["params"], min(args.speedup, spec.get("speedup", args.speedup)))
        try:
            FLIGHTS[spec["flight"]](sim)
        finally:
            log = sim.close()
        dst = OUT / f"{name}.bin"
        trim(log, dst, spec["keep"])
        print(f"   {log.stat().st_size // 1024} KB -> {dst.relative_to(REPO)} {dst.stat().st_size // 1024} KB")
        shutil.rmtree(sim.dir, ignore_errors=True)


if __name__ == "__main__":
    main()
