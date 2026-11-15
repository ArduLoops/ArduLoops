# SITL test logs

DataFlash logs from ArduPilot SITL, each flown with one known fault (or none), for the log-tool
tests in `src/dflog/tests.rs`. They are recorded by `scripts/sitl-test-logs.py` and trimmed to the
messages the tests read. Location is ArduPilot's CMAC test field; the SITL host name is replaced
with `sitl`.

| File | Vehicle | Fault | Read by |
|---|---|---|---|
| `healthy.bin` | Quad X | None: a throttle notch on the motor line, failsafes set. A rule that fires here is wrong | `log_audit`, `log_filter_review`, `log_motor_balance` |
| `notch.bin` | Quad X | Motor noise near 155 Hz; throttle notch at 60 Hz, REF 0.3; gyro batch before and after the filters | `log_filter_review` |
| `vibration.bin` | Quad X | Accelerometer vibration near 95 Hz on both IMUs | `log_audit` |
| `ekf_lane.bin` | Quad X | Accelerometer bias on IMU 1 in Loiter: EKF3 lane switch | `log_audit` |
| `gps_loss.bin` | Quad X | GPS off in Loiter: EKF failsafe, Land without position | `log_audit` |
| `crash.bin` | Quad X | Motor 1 stops in a hover: the vehicle falls | `log_audit` |
| `magfit.bin` | Quad X | Compass offsets 120, -80, 60 unknown to the board; yaw turns with banks | `log_magfit` |
| `motors.bin` | Quad X | Motor 1 gives 8% less thrust | `log_motor_balance` |
| `pid.bin` | Quad X | Stick doublets on each axis in AltHold, rate PIDs at 100 Hz | `log_pid_review` |
| `sysid.bin` | Quad X | System ID mode chirps on the roll and pitch angle input, from Stabilize | `log_pid_review` |
| `plane_pid.bin` | Plane | Aileron and elevator doublets in FBWA | `log_pid_review` |

SITL limits that the tests take into account:

- SITL on Windows drops stretches of records above about 200 Hz, and faster than real time. The PID logs are written at 100 Hz, in real time.
- SITL levels a vehicle that hits the ground, so ArduPilot's crash check does not fire in `crash.bin`.
- The ESC-RPM notch (`INS_HNTCH_MODE` 3) stays at `INS_HNTCH_FREQ` in this SITL, so `healthy.bin` uses a throttle notch.

To record them again: `python scripts/sitl-test-logs.py [scenario ...]`. A new recording is a
different flight, so check the test bounds after it.
