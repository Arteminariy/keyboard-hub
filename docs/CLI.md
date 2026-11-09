# `type84` command line

`type84` is the command-line tool for the IO by Red Square Type 84 Magnetic (USB `0c45:80d6`).
Everything it does is a read, except for the writes listed under [Writes](#writes). Each write
command takes `--dry-run`, which opens only the read-only channel and prints the exact diff and
frame count. The guards behind every write are described in `docs/SAFETY.md`, and the protocol in
`docs/PROTOCOL.md`.

## Global options

| Option | Effect |
|---|---|
| `-v, --verbose` | Print every frame sent and received to stderr. |
| `-q, --quiet` | Print only the essential result. Errors still go to stderr. See the per-command notes below. |
| `--json` | Machine-readable output for `info`, `dump` and `preset current`. |
| `--presets-dir DIR` | Preset directory. Default: `~/.config/type84/presets`. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | OK. For `preset current`: at least one saved preset matches the board in full. |
| 1 | Differs: `verify-backup` found the board different from the backup. |
| 2 | Error: bad arguments, I/O error, refused write, readback mismatch, … |
| 3 | No match: `preset current` found no saved preset that matches in full. |
| 4 | Device not found: no Type 84 configuration interface is connected. |

## Read-only commands

| Command | What it does | `--quiet` | `--json` |
|---|---|---|---|
| `type84 info` | Lists the HID interfaces and reads GET_DEVICE_INFO. | prints the firmware version only | device info, interfaces, raw bytes |
| `type84 dump` | Reads everything and prints the settings, lighting and the per-key table (actuation, RT, base and Fn action). | prints nothing (a read check) | firmware, settings, lighting, per-key list |
| `type84 backup [-o FILE]` | Writes a full JSON backup. Default name: `./backups/type84-<ts>-fw<ver>.json`. It never overwrites an existing file. | prints the path only | – |
| `type84 verify-backup FILE` | Re-reads the board and compares it with a backup. Exit 1 if it differs. | prints nothing | – |
| `type84 preset list` | Lists the presets with their sections. | prints names only | – |
| `type84 preset show NAME` | Prints a preset, section by section. | – | – |
| `type84 preset current` | Shows, per section, which saved presets the board already matches. | prints the fully matching names, one per line | see below |
| `type84 preset save NAME [--sections hall,fn_layer,lighting] [--force]` | Reads the board and saves those sections as a preset. The default section is `hall`. | – | – |
| `type84 preset import-backup FILE NAME [--sections …] [--force]` | Builds a preset from a backup file. No device access. | – | – |

`preset current --json`:

```json
{ "matching": ["cs2", "light-hub"],
  "by_section": { "hall": ["cs2"], "lighting": ["light-hub"] },
  "presets": [ { "name": "fn-fbx50c", "sections": { "fn_layer": false }, "matches": false }, … ] }
```

A preset section matches when applying it would change nothing.

## Writes

| Command | Writes |
|---|---|
| `type84 preset apply NAME [NAME…] [--dry-run] [--yes]` | The sections present in the presets. Each section may come from only one preset. |
| `type84 preset apply NAME… --flash #RRGGBB [--flash-effect breathing\|static] [--flash-speed N] [--flash-ms MS]` | A flash effect frame, the other sections, then the preset's lighting (one 0x23 frame, plus 0x24 if it differs). See `preset apply --flash` below. |
| `type84 restore FILE --only hall\|fn-layer\|lighting [--dry-run] [--yes]` | One section of a backup, verbatim. For `lighting`, an anim state file also works. |
| `type84 anim flash [--effect wave] [--color #RRGGBB] [--brightness N] [--speed N] [--duration S] [--dry-run] [--yes]` | Two single-frame SET_LED_EFFECT writes: switch to the effect, then restore. |
| `type84 anim restore [--dry-run] [--yes]` | Writes back the effect saved by an interrupted `anim flash`. |

Without `--dry-run`, every write command asks you to type `yes`. When stdin is not a terminal
(scripts, hotkeys), it refuses unless you pass `--yes`.

Writes take the write lock (`~/.local/state/type84/anim.lock`, see `docs/SAFETY.md`). If another
`type84` process or the GUI holds it, `preset apply` and `restore` wait up to 5 s and then exit 2
without writing; `anim flash` waits 0.5 s and then skips. On Windows there is no cross-process write lock
(`flock` is a no-op), so run only one writer at a time.

### Presets

Presets live in `~/.config/type84/presets/<name>.json`. A name can also be a path to a `.json`
file. A preset holds any of three sections. Keys or fields that a section leaves out stay as they
are on the board.

```json
{
  "format": "type84-preset", "format_version": 1, "name": "example",
  "hall": { "keys": [ { "index": 34, "label": "W", "actuation_mm": 0.8,
                        "rt": { "press_mm": 0.3, "release_mm": 0.3 }, "whole_travel": true } ] },
  "fn_layer": { "keys": [
      { "index": 1,  "label": "F1", "action": "consumer:WWWHome", "note": "home page" },
      { "index": 2,  "label": "F2", "action": "key:Ctrl+F10" },
      { "index": 17, "label": "1",  "action": "key:F13" } ] },
  "lighting": { "effect": "custom", "color": "#FFFFFF", "color_mode": "single",
                "brightness": 4, "speed": 3, "direction": "forward",
                "keys": [ { "index": 34, "label": "W", "color": "#FF6A00" } ] }
}
```

Each key is given by its index together with its label from the layout (`type84 dump` shows
both). If the two disagree, the preset is refused.

**Fn-layer actions:**

| Action | Record | Notes |
|---|---|---|
| `key:<Key>` | `02 00 uu 00` | Keyboard usage: `F13`, `PrintScreen`, `A`, `0x68`, … |
| `key:<Mod>+…+<Key>` | `02 mm uu 00` | Modifiers: `Ctrl Shift Alt Win RCtrl RShift RAlt RWin`. **UNVERIFIED** whether the firmware sends them (`PROTOCOL.md` §3.3). |
| `consumer:<Name>` | `03 lo hi 00` | `PlayPause NextTrack PrevTrack Stop Mute VolumeUp VolumeDown WWWHome WWWBack WWWForward WWWSearch WWWFavorites Browser Calculator Mail MyComputer`, or `0x0001`–`0x033C`. |
| `none` | `00 00 00 00` | Unassigned. |
| `factory` | from GET_DEFAULT_FN_KEY_MATRIX | Restores the key's factory Fn action. |

Vendor functions (Fn+Delete = reset, Fn+Win = Win lock, the lighting keys), macros, DKS and
advanced keys are never overwritten, and the Fn key itself cannot be remapped.

**Lighting fields:**

| Field | Values |
|---|---|
| `effect` | `off static onPress offPress stars snow riot breathing colorWheel fountain ripple wave serpentine flash double circles stream ridges slantRain backAndForth custom`, or an id. |
| `color` | `#RRGGBB`. Setting it selects single-colour mode, as the vendor hub does. |
| `color_mode` | `single` or `multi`. |
| `brightness`, `speed` | Steps 0–5. |
| `direction` | `forward` or `reverse`. |
| `keys` | Per-key colours. They are shown only by `custom`, so if the board is not in custom mode, add `"effect": "custom"`. Keys you don't list keep their stored colour. |

**Shipped presets** (in `presets/` in the repo; copy them to the preset directory):

| Preset | Section | Content |
|---|---|---|
| `fn-fbx50c` | `fn_layer` | The A4Tech FBX50C Fn row (`docs/FN_LAYER_PLAN.md`): Fn+F1 home, F2 Ctrl+F10, F3 back, F4 search, F5 browser, F6 Shift+PrintScreen, F7–F9 previous/play/next, F10–F12 mute/volume down/volume up. Fn+1/2/3 → F13/F14/F15. |
| `light-wasd` | `lighting` | Example: custom effect with the movement keys orange and Esc red. |

To keep your current lighting as a preset you can return to:
`type84 preset save light-mine --sections lighting`.

### Combining presets

Keep sections in separate presets and combine them on the command line:

```sh
type84 preset apply cs2 fn-fbx50c --dry-run   # hall from cs2, Fn layer from fn-fbx50c
type84 preset apply cs2 fn-fbx50c
```

The combined apply uses one write session and one pre-write backup, and writes the sections in a
fixed order (Fn layer, effect, colours, Hall). Each section is read back and verified. If two
presets both define the same section, the apply is refused.

### Lighting colour and the last-write record

The board applies the effect colour but never reports it: GET_LED_EFFECT always reads
`FF FF FF` / multicolour (`PROTOCOL.md` §3.5). So after every successful effect write the tool
saves the block it wrote and the block the board reported to
`~/.local/state/type84/lighting-last.json`.
- **Fresh record.** While the board still reports exactly that block, diffs (`preset apply`,
  `preset current`, `anim`) use the recorded colour. A preset whose only change is the colour is
  then written, and one that matches is skipped.
- **Missing or stale record.** The record is stale when the vendor hub or an Fn shortcut has
  changed the mode, brightness, speed or direction. In that case the colour counts as unknown and
  the effect is written anyway (once). After that the record is fresh again.
- **Colour changed elsewhere.** A colour-only change made by another program cannot be detected.

### `preset apply --flash`

```sh
type84 preset apply typing fn-fbx50c light-work --flash '#22D07C' --yes --quiet
```

**Sequence.**
1. One SET_LED_EFFECT frame shows the flash: by default a breathing effect («Динамическое
   дыхание», byte 7), brightness 5, speed `--flash-speed` (default 5), in the given colour.
2. The other changed sections (Hall, Fn layer) are written while it shows.
3. Once `--flash-ms` (default 2000) has passed since the flash write, the preset's lighting is
   written: always one 0x23 frame, plus the 10-frame 0x24 block only if the per-key colours differ
   from the board's readback. Without a lighting preset, the previous effect is written back,
   with the recorded colour.

**Options.** `--flash-effect static|breathing`, `--flash-speed 0..5` and `--flash-ms 100..10000`.
The breathing period per speed step has not been measured: tune `--flash-speed` and `--flash-ms`
until the flash looks like one breath. The breath may start mid-cycle.

**Write count per mode switch.** Lighting costs 2 effect frames, plus 10 frames when the per-key
block changes:

| Switch | 0x23 | 0x24 |
|---|---|---|
| cs → work | 2 | 10 (the stored block is still gamedev's, so Esc differs) |
| work → gamedev | 2 | 10 (Esc) |
| gamedev → cs | 2 | 0 |
| work → work | 2 | 0 |

**Safety.**
- One write session and one pre-write backup per invocation. Every write is read back.
- `--dry-run` prints the whole plan, flash included.
- Once the flash is on the board, the final lighting is written even if a section in between
  fails, and even when SIGINT, SIGTERM or SIGHUP arrives. A signal cuts the wait short and the
  exit status stays 0. A failed section still exits 2, after the final lighting.
- The run takes the same lock as `anim`.

### `anim flash`

```sh
type84 anim flash --effect wave --color '#00A0FF' --duration 2 --yes
```

- **Writes.** It reads the current effect, writes the flash effect, waits, and writes the old
  effect back: two single-frame writes per call, and none when nothing would change. The board
  has no volatile lighting channel (`PROTOCOL.md` §3.5a), so it does not stream animations.
- **One at a time.** If another `anim` is running, the call is skipped with no writes.
- **Interrupted runs.** The effect to restore is saved in `~/.local/state/type84/anim-state.json`
  until the restore succeeds. If a run is interrupted (kill, power loss), run
  `type84 anim restore --yes`. The next `anim flash` also restores to the saved effect, never to
  the stale flash effect.
- **Colour.** The restore uses the colour from the last-write record (see above). Without a valid
  record it restores the reported colour (white multicolour).
- **Effects.** `wave` (11) and `ripple` (10) are the ones with motion and a colour. `--color`
  switches the effect to single colour. Brightness and speed stay as they are unless you give them.

## Scripting examples

Check whether the board is connected:

```sh
type84 info -q >/dev/null 2>&1; case $? in 0) echo connected;; 4) echo "not plugged in";; *) echo error;; esac
```

Apply a preset only if the board doesn't already hold it:

```sh
type84 preset current -q | grep -qx cs2 || type84 preset apply cs2 --yes --quiet
```

**Desktop hotkey (KDE Plasma).** Open System Settings → Keyboard → Shortcuts → Add New → Command
or Script. Use an absolute path and a fixed working directory, so that the automatic backups land
in one place:

```sh
sh -c 'cd ~/type84 && /usr/local/bin/type84 preset apply cs2 --yes --quiet'
```

**Mode-switch hotkeys with the `fn-fbx50c` preset.** Fn+1/2/3 send F13/F14/F15. With default
xkb they arrive as the keysyms `XF86Tools`, `XF86Launch5` and `XF86Launch6` (`PROTOCOL.md` §8), and the KDE shortcut editor shows those names. Bind each one
to a script such as:

```sh
#!/bin/sh
# ~/bin/mode-cs2: switch to the CS2 profile with a short blue wave as feedback
cd ~/type84 || exit 2
type84 preset apply cs2 --yes --quiet || exit $?
type84 anim flash --effect wave --color '#00A0FF' --duration 1.5 --yes --quiet
```

Read the current state as JSON:

```sh
type84 dump --json | jq -r '.keys[] | select(.rt != null) | .label'   # keys with Rapid Trigger on
type84 preset current --json | jq -r '.by_section.hall[]?'
```

## Onboard profiles

The board has no onboard profiles that can be selected. Device info byte 19 (`currentProfile`)
reads 0, and the protocol has no profile-select command (`PROTOCOL.md` §11). Presets plus
`preset current` take their place.
