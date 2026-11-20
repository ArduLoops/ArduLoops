# ArduLoops

Ground station and AI assistant for ArduPilot copters and planes — set up your vehicle, inspect flight logs, and tune control loops.

## Quick start

1. **Connect or simulate** — Pick USB, UDP (`14550`), or TCP in **Link**, or open **SITL** in the top bar to launch a simulated copter or plane in one click.
2. **Set up & inspect logs** — Step-by-step wizards for sensors, radio, motors, flight modes, and failsafes. Drop any DataFlash `.bin` log to see automatic diagnostics and charts.
3. **See the control loops** — Watch Attitude, Rate, PosControl, L1, and TECS loops live in flight or simulation, and ask the AI assistant to check parameters and tuning.

## Workspaces

### Copter

![Copter control loops](copter-loops.png)

PosControl and Attitude/Rate loops, motor test, and guided setup.

### Plane

![Plane control loops](plane-loops.png)

L1 navigation, TECS energy loop, roll/pitch rate, and servo setup.
