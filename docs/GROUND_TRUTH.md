# Example validation state (FW 1.17)

The read path was validated by setting this state in the vendor hub and checking that a dump from the
keyboard matches it. It is kept as an example of what a "known good" check looks like; the
backup fixtures under `crates/*/tests/fixtures` were taken in this state.

## Keys
- W, A, S, D, L-Shift, L-Ctrl, Space: actuation point 0.80 mm, Rapid Trigger on,
  press sensitivity 0.30 mm, release sensitivity 0.30 mm, "separate" off, "whole travel" on.
- All other keys: actuation point 1.50 mm, Rapid Trigger off.

## Whole keyboard (as reported by the vendor hub)
- Polling rate: 8000 Hz
- Auto-calibration: on
- Stabilisation: on
- Dead zones: off
