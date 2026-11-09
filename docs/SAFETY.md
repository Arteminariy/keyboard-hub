# Write safety

The tool can write four blocks to the board, each opted into separately:

| Section | Opcode | Block | Hardware status |
|---|---|---|---|
| Hall/RT table | SET_MAGNETIC_AXIS_RT 0x27 | 1024 bytes (`PROTOCOL.md` §3.4, §4.3) | **VERIFIED** (FW 1.17, `PROTOCOL.md` §10) |
| Fn layer (layer 1) | SET_FN_KEY 0x26 | 512 bytes (§3.3, §4.2) | **VERIFIED** (FW 1.17) |
| Lighting effect | SET_LED_EFFECT 0x23 | 16 bytes (§3.5, §4.5) | **VERIFIED** (FW 1.17, `PROTOCOL.md` §10); colour not reported by the board |
| Per-key colours | SET_CUSTOM_LED_DATA 0x24 | 512 bytes (§3.6, §4.6) | **VERIFIED** write, readback and persistence across a replug (FW 1.17) |

Everything else is read-only. In particular the base layer (SET_KEY 0x22), global settings
(SET_GAME_MODE 0x21), macros (0x25) and DKS (0x28) cannot be written. Every write is persistent: the
board has no RAM-only variant (`PROTOCOL.md` §3.5a, §4).

## Guards, from the outside in

1. **Commands.** Only these can write:
   - `type84 preset apply <name>...`
   - `type84 restore <backup> --only hall|fn-layer|lighting`
   - `type84 preset apply ... --flash <colour>`
   - `type84 anim flash` and `type84 anim restore`
   - the GUI (Keyboard Hub): «Применить» on the pending bar, «Пресеты» → «Применить»,
     «Восстановить из бэкапа…» and the banner's «Восстановить». Each one first lets the worker read
     the board fresh and plan the write with the same preview code as `--dry-run`, then shows the
     per-key diff in a confirm dialog; only «Записать» writes. The tray menu never writes: «Применить
     пресет» opens the window and the confirm dialog. A window opened on a backup file
     (`--from-backup`) cannot write.

   With `--dry-run` they open only the read-only transport, print the diff (`label: old → new`) and the
   frame count, and never open a write session. Without `--dry-run` they ask you to type `yes`. If stdin
   is not a terminal they refuse unless `--yes` is given. If the preview shows no change, no write session
   is opened. Several presets can be applied together only if they cover different sections.
2. **Separate session type with a per-section opt-in.** `type84_hid::WriteSession` is opened only by
   `Hid::open_write_session(WriteOptIn)`, which is a different entry point from the read-only
   `Hid::open_config`. Each section has exactly one constructor: `user_confirmed_hall_write`,
   `user_confirmed_fn_layer_write` and `user_confirmed_lighting_write` (0x23 + 0x24). The CLI builds the
   token from the sections that actually change. A plan step outside the token is refused before any I/O.
   The session has no public raw send. Its only write method is `apply_plan`, plus `apply_hall_table`
   as a wrapper around it.
3. **Frame allowlist, checked immediately before every `hid_write`.**
   - Read-only transport: request mark 0xAA, read opcodes 0x10–0x18 and 0x1C, zero reserved bytes and an
     all-zero payload.
   - Write session: the same reads. During the write of one section, it also allows that section's
     opcode, and only when the header is exactly a chunk of the full block:
     - `addr` is a multiple of 56 inside the block;
     - `len = min(56, size - addr)`;
     - byte 6 is set only on the last chunk;
     - reserved bytes and the padding past `len` are zero.

     The chunk plans are 1024 B = 18 × 56 + 16, 512 B = 9 × 56 + 8, and 16 B = a single frame. While one
     section is written, the other write opcodes are refused. The pre-write backup only ever gets a
     read-only view.
   - Refused everywhere: every other opcode. That includes 0x0F factory reset, 0x21, 0x22, 0x25, 0x28,
     the screen and animation opcodes 0x2A, 0x34, 0x3D, 0x41, 0x50 and 0x51, 0x64–0x6A calibration,
     0x80–0x86 OTA/bootloader, and the ISP magic frames. Feature reports can't be sent at all, because the
     transport trait has no such method.
   - Unit tests sweep all 256 opcodes, read-shaped and write-shaped, for the read-only view and for each
     single-section policy. They check against a mock device that nothing refused reached it.
4. **Fixed sequence inside `apply_plan`.**
   1. Plan check: every step is covered by the opt-in and no section appears twice.
   2. GET_DEVICE_INFO. Refuse if VID/PID differ, the board is not magnetic or `firmwareStatus != 0`. A
      raw Hall restore is also refused if the backup's rtPrecision differs from the board's.
   3. Automatic pre-write backup: a full read of every block, saved as a new file under `backups/auto/`
      (relative to the working directory). The file name has a millisecond timestamp and a counter. It is
      opened with `create_new`, so nothing is ever overwritten, and it is re-read and validated after
      saving. If this step fails, nothing is written. (`anim` is the one exception, see below.)
   4. For every section, before the first write:
      1. Read the section fresh. Refuse if it differs from the block shown in the preview, for example
         because the case switch was moved since then.
      2. Merge and validate the new block:
         - **Hall.** Presets may touch only the 84 physical keys. `axis_type`, flag bits 1–7 and every
           non-physical record are kept as read. Actuation must be 0.10–3.20 mm. RT press and release
           must each be 0.01–3.20 mm, or both 0. A raw restore writes the backup's block verbatim.
         - **Fn layer.** Only physical keys below index 126 can change, and never the Fn key (85). A key
           can be changed only while it holds `none`, a keyboard record or a consumer record. Vendor
           functions (Fn+Delete = reset, Fn+Win = Win lock, lighting keys), macros, DKS and advanced keys
           are never overwritten, except by `factory`, which copies the record from
           GET_DEFAULT_FN_KEY_MATRIX. Every write, raw restores included, must keep `02 00 AF 00` at
           index 85 and leave records 126–127 unchanged.
         - **Lighting.** The effect id must be one this board offers (0–19, 128). Brightness and speed are
           steps 0–5, and direction and colour mode are 0 or 1. Mode 20 is also accepted, because the board
           reports the custom effect 128 as 20 and a backup taken in custom mode holds 20. Per-key colours are
           refused unless the effect after the write is a custom mode (20 or 128). Colours are re-encoded with
           `ledId = index`.

      If any section is invalid, nothing at all is written. Abort if no section changes.
   5. Write each changed section in a fixed order: Fn layer, effect, per-key colours, Hall. The effect
      goes first so that the board is already in custom mode when the colours land. One packet is
      in flight at a time, and each waits for its ACK (addr is matched for 0x26/0x27, as in the hub).
      Each packet has a 1000/2000 ms timeout and up to 3 resends. A device-reset notification aborts at
      once.
   6. After each section, wait 40 ms (Hall, Fn layer) or 500 ms (lighting), read it back and compare:
      - Hall and Fn layer byte for byte;
      - the effect on the fields the board reports (`led::effect_verify_eq`): the mode, with custom 128
        accepted as 20, plus brightness, speed, direction, secondary colour and effectModeType. The colour
        and colour mode are **not** compared, because the board applies them but always reports
        `FF FF FF` / multicolour (`PROTOCOL.md` §3.5). The CLI then prints "colour not verifiable by
        readback". Byte 4 and the `AA 55` check code read back as 0 and are not compared either;
      - per-key colours on RGB (ledId reads back as written, but it is re-encoded anyway).

      Diffs and skip decisions still include the colour, so a colour change is always written. Their
      only leniency is that modes 20 and 128 count as the same effect. The hub itself never verifies
      a 0x23 or 0x24 write. It caches what it wrote, or re-reads after 500 ms and shows the board's
      report (`driver-CWjFZVQ-.js` `rememberMode`).
5. **Errors name the backup.** Every failure after step 3 includes the pre-write backup path, the
   restore command for the affected section (`type84 restore <backup> --only hall|fn-layer|lighting`)
   and the sections already written and verified. There is no automatic rollback, because that would be
   a second, equally unverified write.

## Colour record and forced effect writes

- **Why.** The board never reports the effect colour (`PROTOCOL.md` §3.5). The CLI therefore keeps
  `~/.local/state/type84/lighting-last.json`: the block it last wrote and the block the board read
  back afterwards. It updates the file after every successful 0x23 write.
- **Fresh record.** While the board still reports exactly the recorded readback, diffs use the
  recorded colour.
- **Missing or stale record.** The effect is written even if it looks unchanged.
- **How the write is sent.** Effect blocks from presets and from `anim` go to `apply_plan` as
  `EffectTarget::Forced`. That is a raw block written even when it equals the readback; the preview
  check (`expected_base`) still compares against the raw readback. When the CLI decides from the
  record that nothing changes, it leaves the step out of the plan.

## `preset apply --flash`

- **Write count.** One flash 0x23 frame, then the other sections, then the final 0x23 frame, plus
  the 0x24 block only if it differs from the readback. All of it runs in one write session with one
  pre-write backup.
- **Opt-in.** The opt-in covers lighting plus whichever other sections change.
- **The final lighting is always written once the flash is on the board.**
  - A section in between fails: the final lighting is still written, and the exit code is 2.
  - SIGINT, SIGTERM or SIGHUP arrives: the wait is cut short and the final lighting is written.
  - The flash write fails after its frame may have been sent: the final effect is written back
    without the preview check.
- **Locking.** It holds the `anim` lock.

## `type84 anim flash`## `type84 anim flash`

- A flash is exactly two single-frame 0x23 writes: switch to the built-in effect, then put the previous
  effect back. Each write is skipped when it would change nothing.
- There is no host-streamed animation, because each frame would be a flash write (`PROTOCOL.md` §3.5a).
- **Pre-write backup.** The only block written is the 16-byte effect. Its pre-write backup is the anim
  state file `~/.local/state/type84/anim-state.json`, which is saved and re-read before the first write.
  A full board backup per hotkey press would cost about 90 read packets and a file each time.
- **Colour.** The board does not report its colour (`PROTOCOL.md` §3.5). The restore therefore writes
  the colour from the read, which is always `FF FF FF` multicolour, not the colour the board actually
  showed.
- **Interrupted runs.** If a run is interrupted, the next run reuses the saved effect as the one to
  restore. `type84 anim restore` writes it back on its own, and so does
  `type84 restore <state file> --only lighting`.
- **One at a time.** An exclusive non-blocking `flock` makes a concurrent second run skip, with no
  writes. SIGINT, SIGTERM and SIGHUP cut the wait short and still restore.

## Where automatic backups go

`backups/auto/` is relative to the working directory. A command bound to a desktop hotkey usually runs
in `$HOME`, so each `preset apply` from a hotkey that changes something adds a full backup (~25 KB)
under `~/backups/auto/`. Run hotkey commands from a fixed directory (`cd ~/type84 && type84 …`) and
prune old files from time to time.

## The write lock

Every write takes an exclusive `flock` on `<state dir>/anim.lock` (`~/.local/state/type84/anim.lock`):
`preset apply`, `restore`, `anim` and the GUI. A write waits up to 5 s for another holder to release
it, then is refused with "another type84 process is writing to the keyboard" and nothing is written.
`anim flash` waits only 0.5 s and then skips quietly, so a concurrent animation is still skipped. The
GUI holds the same lock for every device access: its 5 s Hall poll takes it without waiting and is
skipped while another process writes; full reads, previews, backups and writes (with the re-read
after them) wait up to 5 s. So no GUI read overlaps a `type84` write, and a hotkey that lands during a
GUI poll waits a few hundred milliseconds instead of failing. The lock is a no-op on Windows (no
`flock`): there is **no cross-process write lock on Windows**, so do not run two writers (CLI, GUI,
hotkey scripts) at the same time there.

GUI files: pre-write backups go to `<state dir>/backups/auto/`, «Сделать бэкап» to
`<state dir>/backups/`; presets are the CLI's (`~/.config/type84/presets`); the lighting colour
record is shared with the CLI. The write itself goes through the same library code as the CLI
(`type84_cli::apply::run_steps`).

## The case switch

Moving the actuation switch on the case rewrites the whole Hall table: every record's actuation
becomes the preset and RT turns off (`PROTOCOL.md` §9.1). Re-apply your preset afterwards with
`type84 preset apply <name>`.
