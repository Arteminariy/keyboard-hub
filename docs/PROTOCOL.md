# IO by Red Square Type 84 Magnetic (USB 0c45:80d6): configuration protocol ("HFD")

Status: derived by static analysis of the vendor hub, then partly verified on hardware.
Captured bytes quoted from the Python project (`reference/py-configurator/captures/...`) were
recorded earlier by its author from the old vendor app (web.io.vision) and serve only as a
cross-check.

**Hardware verification (2026-11-09, FW 1.17, read path only).** The `type84` CLI of this repo read
every block in §7.1 from a real board (IO Type 84 Magnetic Black, Linux hidraw, hidapi) and decoded
it against the state the user had set in the vendor hub (`docs/GROUND_TRUTH.md`). Every field
checked matched. Items marked **VERIFIED (FW 1.17)** below come from that session.

**Write status (2026-11-09).** Four write opcodes are implemented, each opted into separately
(`type84_hid::WriteSession`, guards in `docs/SAFETY.md`):

| Opcode | Block | Code | Hardware status |
|---|---|---|---|
| 0x27 SET_MAGNETIC_AXIS_RT | Hall/RT table, 1024 B (§4.3) | `type84_proto::hall` | **VERIFIED (FW 1.17)**, §10 |
| 0x26 SET_FN_KEY | Fn layer, 512 B (§4.2) | `type84_proto::fn_layer` | **VERIFIED (FW 1.17)** |
| 0x23 SET_LED_EFFECT | effect, 16 B (§4.5) | `type84_proto::led` | **VERIFIED (FW 1.17)**; the colour is applied but never reported by reads |
| 0x24 SET_CUSTOM_LED_DATA | per-key colours, 512 B (§4.6) | `type84_proto::led` | **VERIFIED (FW 1.17)**; persists across a replug |

No other write opcode exists in the code. **Not implemented:** SET_GAME_MODE (0x21, global settings),
SET_KEY (0x22, layer 0), macro (0x25) and DKS (0x28) writes. SET_KEY (0x22) in particular is
**not** implemented: the Fn layer has its own opcode, so the base layer stays read-only.

Primary source: the current Red Square Hub build (`hub.red-square.org`, entry `index-DQigh6Ga.js`).
This board maps to model id `hfd.io-type-84-magnetic-magnetic-pro` and protocol `hfd`. The protocol
code lives in these files:

| File (all in `reference/hub/`) | Role |
|---|---|
| `index-DQigh6Ga.js` | device registry, WebHID transport, KLE layouts, SONiX ISP flasher (dangerous) |
| `src-u8pKurse.js` | HFD manifest: interface match, probe, notification parser, RT codec, macro/DKS/settings capabilities |
| `owned-BuJoYyKq.js` | HFD core: opcode table, framing, transfer engine, record codecs |
| `driver-CWjFZVQ-.js` | HFD device driver: connect sequence, keymap/RT/lighting/calibration/OTA, snapshot/restore |
| `ui-C7M0IjYH.js` | HFD UI: field units, ranges, advanced-key parameter order |

How to re-verify a citation: `LC_ALL=C grep -o -b -F '<snippet>' reference/hub/<file>`. The number
printed is the byte offset and should equal the `@offset` given here. Every snippet cited below was
checked to occur exactly once in its file. Code quoted in prose comes from a de-obfuscated, prettified
copy, so its identifiers differ from the minified file. The cited snippets are the minified originals.

Integers are little-endian unless stated otherwise. "Payload" means the bytes starting at frame
offset 8.

---

## 1. Device identification and transport

### 1.1 Which interface

The board enumerates several HID interfaces. A capture from the old app (`read_01_initial_load.json`)
listed three WebHID devices with the same VID/PID:

| Usage page : usage | Reports | Use |
|---|---|---|
| `0xFF68 : 0x0061` | output ID 0, input ID 0 | **Configuration channel. Open only this one.** |
| `0xFF67 : 0x0061` | output ID 0, feature ID 0, no input | **Never open.** The hub uses usage page 0xFF67 (`Me=65383`) for TFT/screen channels, and this interface's feature report is what the ISP flasher probes (§5). **Absent on the verified unit** (see below). |
| `0x000C : 0x0001` | input ID 3 | Consumer-control keys. Not used. |

**VERIFIED (FW 1.17), Linux hidapi enumeration of the user's unit.** There is no 0xFF67 collection,
contrary to the old capture. The board exposes three hidraw nodes:

| USB interface | Collections (usage page : usage) |
|---|---|
| 0 | `0x0001 : 0x0006` (boot keyboard) |
| 1 | `0x000C : 0x0001`, `0x0001 : 0x0080`, `0x0001 : 0x0006`, `0x0001 : 0x0002` |
| 2 | `0xFF68 : 0x0061` only: the configuration channel, on its own node |

**VERIFIED (FW 1.17), HID report descriptors** (read from `/sys/class/hidraw/hidrawN/device/report_descriptor`,
the kernel's cached copy; nothing is sent to the board). They bound what keymap records can emit (§3.3):
- Interface 1, report 3, consumer control: `05 0C 09 01 A1 01 85 03 19 00 2A 3C 03 15 00 26 3C 03 95 01 75 10 81 00`,
  one 16-bit array slot, usages **0x000–0x33C**. AL Internet Browser (0x196) and AC Search/Home/Back
  (0x221/0x223/0x224) are inside the range.
- Interface 1, report 1, keyboard: bitmap of usages **0x00–0x77** (`19 00 29 77 95 78 75 01 81 02`), so
  F13–F24 (0x68–0x73) can be reported. Interface 0 (boot keyboard) carries the 8-bit modifier byte and a
  6-key array of usages 0x00–0xFF.
- Interface 1 also has report 2 (System Control, usages 0x81–0x83) and report 6 (mouse).

How the hub chooses: the HFD manifest matches any collection whose usage page is in
`[0xFF68, 0xFF80, 0xFF60, 0xFF00, 0xFF01, 0xFF1B]` and that has output report ID 0 and input report
ID 0. On this board only 0xFF68 qualifies.
- `src-u8pKurse.js@58108` `Mr=[65384,65408,65376,65280,65281,65307]`
- `src-u8pKurse.js@58290` `requiresOutputReport:0,requiresInputReport:0`
- Report size is 64 (`reportSize:64` in the same manifest object). `REPORT_ID` is 0.

Device registry entry:
- `index-DQigh6Ga.js@202715` `` productId:32982,productNamePattern:`^(?:IO Type 84 Magnetic Black|IO Type 84 Magnetic White)$` ``
- Quirks: report rates offered are 1000/4000/8000 only, the sleep setting is hidden, single-key wakeup
  is hidden, advanced keys are RS, SOCD, DKS, MT and TGL (no CB), and the board is wired-only.
  - `index-DQigh6Ga.js@203331` `` advancedKeys:[`RS`,`SOCD`,`DKS`,`MT`,`TGL`],fnLocked:[],wireless:!1},presentation:{displayName:`IO Type 84 Magnetic` ``

### 1.2 Report type and direction

- **Host to device:** HID output reports, report ID 0, always exactly 64 bytes.
  - WebHID call: `device.sendReport(0, frame64)`.
  - `index-DQigh6Ga.js@178025` `await this.#t.sendReport(t,e)`. The HFD engine always passes reportId 0.
- **Device to host:** HID input reports, report ID 0, 64 bytes, arriving on `inputreport`.
- **Feature reports are never used by the HFD protocol.** They appear only in the ISP flasher (§5).
  Treat any feature-report call as forbidden.

### 1.3 Porting from WebHID to hidapi (Linux hidraw)

WebHID passes the report ID separately. hidapi expects the report ID in byte 0 of the write buffer.
- **Write:** build a 65-byte buffer `[0x00] ++ frame64` and call `hid_write`. Byte 0x00 is the "no
  report ID" marker and is stripped by the kernel, so 64 bytes go on the wire.
  - Never send fewer than 64 frame bytes. Zero-pad.
  - If byte 0 of the buffer is the 0xAA mark, hidraw treats 0xAA as a report ID and the frame is
    corrupted.
- **Read:** `hid_read` returns the 64-byte input report. Because the device uses no report IDs, there
  is no ID prefix. Byte 0 is the 0x55 response mark.
- **Device selection:** enumerate VID 0x0C45 / PID 0x80D6 and keep the entry with
  `usage_page == 0xFF68`, `usage == 0x0061`.
  - VERIFIED (FW 1.17): 0xFF68 is alone on USB interface 2 (its own hidraw node) and hidapi's hidraw
    backend reports `usage_page`/`usage` per collection. The CLI still refuses a node that also lists
    any other collection, and refuses to choose when more than one 0xFF68 node exists.
  - VERIFIED (FW 1.17): a 65-byte `hid_write` with a leading 0x00 is accepted, and `hid_read` returns
    64 bytes that start with 0x55 (no report-ID prefix).

### 1.4 Frame format (request)

Built by `fe()` in owned (`owned-BuJoYyKq.js@25798` `n[0]=170,n[1]=e.cmd&255`):

| Offset | Value |
|---|---|
| 0 | `0xAA` (REQUEST_MARK) |
| 1 | `cmd` (opcode, §1.7) |
| 2 | `len`: number of payload bytes in this packet (0..56) |
| 3 | `addr & 0xFF`: byte offset of this chunk inside the logical block |
| 4 | `addr >> 8` |
| 5 | 0 (or `otherHeader[0]`, never used for this board's normal traffic) |
| 6 | **last-packet flag**: 1 on the final packet of a transfer, otherwise 0 (`owned-BuJoYyKq.js@26024` `n[6]=+!!e`) |
| 7 | 0 |
| 8..63 | payload (up to 56 bytes), zero padded. Read requests send an all-zero payload. |

There is no checksum and no CRC in HFD framing. The only "check" bytes in the whole protocol are the
`AA 55` trailer inside the 16-byte LED-effect record (§4.5).

Some commands (OTA, calibration V2) use a raw 8- or 9-byte `customHeader` in place of the header
above. Those are all on the blocklist (§5).

### 1.5 Frame format (response)

Parsed by `de()` (`owned-BuJoYyKq.js@25572` `e[0]!==85?null`, then `@25588`
`cmd:e[1],lenOrType:e[2]`):

| Offset | Value |
|---|---|
| 0 | `0x55` (RESPONSE_MARK) |
| 1 | echoed `cmd` |
| 2 | echoed `len` (or a type code in notifications) |
| 3..4 | echoed `addr` (LE) |
| 5..7 | echoed header bytes. VERIFIED (FW 1.17): byte 6 echoes the last-packet flag; bytes 5 and 7 are 0. |
| 8..63 | payload: data for reads; for writes the hub ignores the payload and treats the frame as an ACK |

### 1.6 Transfer engine (chunking, matching, timeouts)

Logical blocks are moved by `transfer({cmd, contentSize, addrStart=0, data?})` in owned
(`#c` method; `owned-BuJoYyKq.js@28510` `contentSize??24`, `@28718` `maxRetries:y=3`).

- Payload per packet: `64 - 8 = 56` bytes.
  - Packets = `ceil(contentSize / 56)`.
  - Packet `i` has `addr = addrStart + 56*i` and `len = min(56, remaining)`.
  - The last packet has byte 6 = 1.
  - Example: a 512-byte block is 9×56 + 1×8 (addresses 0x000 … 0x1C0, 0x1F8). A 1024-byte block is
    18×56 + 1×16 (last packet at addr 0x3F0).
- **Strictly one packet in flight.** Send packet `i`, then wait for a matching response before
  sending `i+1`.
- **Matching:** response byte 0 = 0x55 and byte 1 = cmd, for every opcode. Only for these opcodes
  must the echoed addr also equal the request addr: GET_DEVICE_INFO, GET_KEY, GET_FN_KEY,
  GET_MAGNETIC_AXIS_RT, SET_KEY, SET_FN_KEY, SET_MAGNETIC_AXIS_RT, GET_ALL_LIGHTS_RGB_24G_64_BYTE.
  - `owned-BuJoYyKq.js@24858` `return ie.has(e)` is the checkAddr set.
  - Do the same as the hub and match addr only for this set.
  - VERIFIED (FW 1.17): every read opcode (0x10–0x18, 0x1C) echoed `len` and `addr` exactly on every
    packet, including the DKS, game-mode, LED and macro-table transfers. Not yet exercised: GET_MACRO
    at addr ≥ 400 (the board had no macros) and all writes. Keep matching addr only for the set
    above.
- **Non-matching input reports must be skipped, not treated as errors.** This covers notifications
  (§2.3) and stale responses.
- **Timeouts per packet:**
  - DEFAULT 500 ms.
  - SHORT 1000 ms: GET_DEVICE_INFO, LED effect get/set, calibration on.
  - BLOCK 2000 ms: GET/SET_GAME_MODE, KEY/FN_KEY, RT, DKS, MACRO, CUSTOM_LED, OTA.
  - `owned-BuJoYyKq.js@23843` `D={timeoutMs:50,maxRetries:3},O={DEFAULT:500,SHORT:1e3,BLOCK:2e3}`
- **Retries:** up to 3 resends of the same packet on timeout. After that the whole transfer fails.
- **Reassembly:** concatenate `payload[0..len]` of each response in order. A short total raises
  `hfd.shortPayload`.
- The old app's ACK latency for writes was about 18–19 ms per packet (py `READ_PROTOCOL.md`). Reads
  took 3–6 ms.

### 1.7 Opcode table (current hub)

All entries come from the single object `C={...}` in `owned-BuJoYyKq.js` starting at `@22197`. The
offset given is that of the literal `NAME:value`. "Hub uses" means the HFD driver actually calls it
for this board.

| Dec | Hex | Name | Offset | Class |
|---|---|---|---|---|
| 16 | 0x10 | GET_DEVICE_INFO | @22197 | read, safe |
| 17 | 0x11 | GET_GAME_MODE | @22216 | read, safe |
| 18 | 0x12 | GET_KEY (layer 0) | @23017 | read, safe |
| 19 | 0x13 | GET_LED_EFFECT | @22455 | read, safe |
| 20 | 0x14 | GET_CUSTOM_LED_DATA | @22491 | read, safe |
| 21 | 0x15 | GET_MACRO | @22931 | read, safe |
| 22 | 0x16 | GET_FN_KEY (layer 1) | @23028 | read, safe |
| 23 | 0x17 | GET_MAGNETIC_AXIS_RT | @23071 | read, safe |
| 24 | 0x18 | GET_MAGNETIC_AXIS_DKS_DATA | @22957 | read, safe |
| 28 | 0x1C | GET_DEFAULT_FN_KEY_MATRIX | @23042 | read, safe (factory Fn layer) |
| 31 | 0x1F | GET_DEFAULT_KEY_MATRIX | @23816 | read; defined but not called by the HFD driver (UNVERIFIED on this board) |
| 33 | 0x21 | SET_GAME_MODE | @22233 | write |
| 34 | 0x22 | SET_KEY | @23119 | write |
| 35 | 0x23 | SET_LED_EFFECT | @22473 | write |
| 36 | 0x24 | SET_CUSTOM_LED_DATA | @22782 | write |
| 37 | 0x25 | SET_MACRO | @22944 | write |
| 38 | 0x26 | SET_FN_KEY | @23130 | write |
| 39 | 0x27 | SET_MAGNETIC_AXIS_RT | @23095 | write |
| 40 | 0x28 | SET_MAGNETIC_AXIS_DKS_DATA | @22987 | write |
| 51 | 0x33 | GET_ALL_LIGHTS_RGB | @22514 | live LED readback. Not used on this board: live lighting is off for magnetic boards. |
| 55, 59 | 0x37, 0x3B | GET_ALL_LIGHTS_RGB_24G(_64_BYTE) | @22536 (`GET_ALL_LIGHTS_RGB_24G:55`), @22562 (`GET_ALL_LIGHTS_RGB_24G_64_BYTE:59`) | 2.4G only. Not applicable. |
| 52 | 0x34 | SET_TEMPORARY_COMMAND_DATA | @23144 | clock set. Only for boards with a screen. Not applicable. |
| 61, 42, 80, 81, 65 | | dot-matrix / TFT / LED animation | @22805 (`GET_DOT_MATRIX_CONFIG:61`) through @22905 (`SET_LED_USER_ANIMATION:65`) | not applicable; blocklist |
| 15 | 0x0F | SET_FACTORY_RESET | @22250 | **DANGEROUS** |
| 100–106 | 0x64–0x6A | calibration / simulation test / axis status | @22271 (`SET_CALIBRATION_ON:100`) through @22428 (`SET_CALIBRATION_OFF_V2:106`) | **DANGEROUS** (except 104, which is read-only but only meaningful in calibration mode) |
| 128–134 | 0x80–0x86 | OTA_* | @22596 (`OTA_GET_DEVICE_SYSTEM_INFO:128`) through @22760 (`OTA_SET_DEVICE_SN:134`) | **DANGEROUS** (§5) |
| 250 | 0xFA | DEVICE notify (unsolicited) | `owned-BuJoYyKq.js@23175` `w={DEVICE:250,LINK:252}` | input only |
| 251 | 0xFB | calibration frame (unsolicited) | `owned-BuJoYyKq.js@61380` `e[1]!==251` | input only |
| 252 | 0xFC | LINK notify (unsolicited) | `owned-BuJoYyKq.js@23175` `w={DEVICE:250,LINK:252}` | input only |

The read/write pairs follow the rule `SET = GET + 0x10`.

---

## 2. Connect / handshake sequence

There is no session-open command. The old app's COMMUNICATION_START/END (1/2) is absent from the
current table. Opening the interface and sending GET_DEVICE_INFO is the handshake.

Order taken from `HfdDevice.connect()` in `driver-CWjFZVQ-.js`:

1. **Probe.** The manifest's `probe()` sends `GET_DEVICE_INFO` (contentSize 48) and decodes it
   (§3.1).
   - Writes are blocked when `firmwareStatus != 0`. The hub then sets `device.firmwareBusy` and shows
     "updating".
   - `owned-BuJoYyKq.js@30590` `e.firmwareStatus===0`
2. **Subscribe** to unsolicited frames (§2.3).
3. **Magnetic check.** If `axisInfo > 0`, the board is magnetic. In that case the hub creates the RT,
   DKS and calibration capabilities.
4. **Prime keymap:** GET_KEY (512), then GET_FN_KEY (512).
5. **Prime actuation:** GET_MAGNETIC_AXIS_RT (1024).
6. **Read settings:** GET_GAME_MODE (56).
7. **Lazy reads.** The hub reads these later, when the relevant tab or feature needs them:
   - GET_LED_EFFECT (16)
   - GET_CUSTOM_LED_DATA (512), read only when the effect is a custom one
   - GET_MAGNETIC_AXIS_DKS_DATA (1024)
   - GET_MACRO (table plus blocks)
   - GET_DEFAULT_FN_KEY_MATRIX (512)

Example request (decimal 16, GET_DEVICE_INFO, 48 bytes, single packet so byte 6 = 1):
`AA 10 30 00 00 00 01 00` followed by 56 zero bytes.

Example captured response (old app, FW 1.17):
`55 10 30 00 00 00 01 00 | 00 00 00 92 45 0C D6 80 17 01 00 00 66 01 0C 11 0A 00 00 00 0F 00 00 00 00 00 00 00 00 00 00 00 00 00 ...`

Response read from the verified unit (FW 1.17, 2026-11-09):
`55 10 30 00 00 00 01 00 | 00 00 00 92 45 0C D6 80 17 01 00 00 66 01 16 0A 22 00 00 00 0F 00 00 00 00 00 ...`
The product code (payload 14..15) reads 0x0A16 and workMode reads 0x22, where the capture had
0x110C and 0x0A. Everything else is identical. The meaning of these two fields is not known; nothing
in the hub depends on them for this board. A verbose full read (89 packets, all blocks of §7.1 plus
0x1C) saw no stray, stale or notification frames.

### 2.1 Power-poll

The hub polls GET_DEVICE_INFO periodically for battery only when `wireless !== false`. This board is
declared wireless:false, so there is no polling.

### 2.2 Report-rate change re-enumerates the device

When a SET_GAME_MODE changes `reportRate`, the hub expects the USB device to drop and come back:
- It sends the write with `maxRetries:0` and tolerates a missing ACK.
- It waits up to 5 s, re-opens the device, and verifies identity with GET_DEVICE_INFO (vendorId,
  productId, manufacturer, product, frameVersion, lightingVersion).
- It reads GET_GAME_MODE back and compares all 56 bytes.
- `src-u8pKurse.js@13929` `cmd:w.GET_DEVICE_INFO,contentSize:48,maxRetries:0,abortSignal`

**Rust port:** after a report-rate change, expect write/read errors and the hidraw node disappearing.
Close the handle, re-enumerate, re-open, verify, and read back. This behaviour is inferred from the
hub's recovery code, not observed.

### 2.3 Unsolicited input frames (must be filtered from the response stream)

Parser `Nn` in `src-u8pKurse.js` (`@6148` `Mn=[166,255,1]`, `@6117` `En=85,Dn=4,On=5,kn=6,An=6,jn=2`):

| Bytes | Meaning | Hub action |
|---|---|---|
| `A6 FF 01 …` | wake | resume |
| `55 FC 04 …` | link lost (2.4G) | mark offline |
| `55 FC 05 …` | **device reset** | re-read everything (keymap, RT, settings, …) |
| `55 FC 06 …` | sleep | mark offline |
| `55 FA 02 xx xx BB CC` | power: battery = byte 5 (≤100), charging = byte 6 (≤2) | wireless only |
| `55 FA 06 ww` | screen info wanted (ww = 1) | screen boards only |
| `55 FB …` (≥14 bytes) | calibration frame: key = b2, status = b3, max = u16 b4, min = u16 b6 & 0x7FFF, current = u16 b8, stroke = u16 b10, maxStroke = u16 b12 | calibration mode only |

---

## 3. READ commands

All reads send zero payloads and use the chunking in §1.6.

### 3.1 GET_DEVICE_INFO (0x10), 48 bytes

Decoder: `owned-BuJoYyKq.js@30129` `romSize:i(0)`. The payload offsets are:

| Payload offset | Field | Captured value (FW 1.17) |
|---|---|---|
| 0 | romSize | 0x00 |
| 2..3 | macroSpaceSize. Used only if quirk `readsMacroSpaceSize`, which is false for this board, so the hub assumes 512. | 0x9200 (ignored) |
| 4..5 | vendorId | 0x0C45 |
| 6..7 | productId | 0x80D6 |
| 8, 9 | version = `((b8&0x0F) + (b8>>4)*10 + b9*100) / 100` (`@30217` `version:ve(i(8),i(9))`) | 0x17, 0x01 → 1.17 |
| 10..11 | sensor | 0 |
| 12..13 | manufacturer code | 0x0166 |
| 14..15 | product code | 0x110C |
| 16 | workMode | 0x0A |
| 17 | batteryLevel | 0 |
| 18 | chargeStatus | 0 |
| 19 | currentProfile (read-only; the hub has no profile switching for HFD) | 0 |
| 20..21 | axisInfo (>0 means magnetic) (`@30359` `axisInfo:r(20)`) | 0x000F |
| 22..27 | tftMaxFrames, gifMaxFrames, ledMaxFrames | 0 |
| 28 | tftDirection | 0 |
| 29 | **rtPrecision**: selects RT units (§3.4). VERIFIED (FW 1.17): 0 on the user's unit, and the 0.01 mm decoding matches the hub UI exactly (§3.4). The hub feeds this byte straight into the RT scale (`driver-CWjFZVQ-.js@7554` `je(e.rtPrecision)`). | 0 |
| 30 | frameVersion: 1 selects calibration V2 | 0 |
| 31 | lightingVersion | 0 |
| 32 | **firmwareStatus**: 0 means operational. Block all writes otherwise. | 0 |

### 3.2 GET_GAME_MODE (0x11), 56 bytes: global settings

Field map: `owned-BuJoYyKq.js@30663` `gameMode:1,fnSwitch:2,sleepTime:3`, and `@30855`
`nkroSwitch:17,wirelessReportRate:18`.

| Payload byte | Field | Encoding | Shown in hub for this board |
|---|---|---|---|
| 0 | (unused / preserve) | | |
| 1 | gameMode | raw u8. Old-app/py label: "Game mode / Win-key lock". **UNVERIFIED.** The hub never writes it; Win lock exists only as Fn-layer function key `hfd.func.winLock`. | no |
| 2 | fnSwitch | u8 | no |
| 3 | sleepTime | minutes (meaning of 0 UNVERIFIED) | no (quirk sleep:false) |
| 4 | keyDelay | ms step | no (hidden on magnetic boards: `ui-C7M0IjYH.js@42330` `S=U(()=>R(p).keyDelay&&`) |
| 5 | **reportRate** | code: 0=125, 1=250, 2=500, 3=1000, 4=2000, 5=4000, 6=8000 Hz (`owned-BuJoYyKq.js@32547` `Oe=new Map([[0,125]`). This board offers 3/5/6. | yes |
| 6 | systemMode | u8 (py: 0 = Win, 1 = Mac; UNVERIFIED) | no |
| 7 | tftDisplayTime | u8 | no |
| 8 | **topDeadZone** | mm × 100 (u8). UI range 0–0.50 mm, step 0.01, preset 0.30 (`ui-C7M0IjYH.js@42504` `T={min:0,max:.5,step:.01,preset:.3}`) | yes ("dead zones" toggle sets both to 0.30 or both to 0) |
| 9 | **bottomDeadZone** | mm × 100 (u8) | yes |
| 11 | **stabilityMode** | 0/1 | yes |
| 14 | **autoCalibration** | 0/1 | yes |
| 15 | singleKeyWakeup | 0/1 | no |
| 16 | pushButtonMode | u8 | no |
| 17 | nkroSwitch | u8 (only when quirk nkro is set, which it is not here) | no |
| 18..19 | wirelessReportRateHz | u16 (wireless only) | no |
| others | unknown | **preserve verbatim** | |

Capture (FW 1.17):
`00 00 00 01 00 06 00 00 00 00 00 01 00 00 01 00 …`
This decodes as sleep=1, rate=8000 Hz, stability=1, autoCal=1.

VERIFIED (FW 1.17): the user's unit returns exactly the same 56 bytes (all bytes beyond 15 are 0).
The hub showed 8000 Hz, auto-calibration on, stability on and dead zones off, which matches bytes
5, 14, 11 and 8/9.

### 3.3 GET_KEY (0x12) / GET_FN_KEY (0x16) / GET_DEFAULT_FN_KEY_MATRIX (0x1C), 512 bytes each

Sources:
- Layer table: `driver-CWjFZVQ-.js@2120` `ze=[y.GET_KEY,y.GET_FN_KEY]`.
- Record type enum: `owned-BuJoYyKq.js@34494` `DEFAULT:0,MOUSE:1`.

Layout:
- 128 records of 4 bytes, `record[i] = block[4i .. 4i+4]`.
- `i` is the key index (§3.9).
- Layer 0 = base (GET_KEY). Layer 1 = Fn (GET_FN_KEY).
- GET_DEFAULT_FN_KEY_MATRIX returns the factory Fn layer. The hub uses it to restore Fn keys and to
  learn which function IDs exist.

Record `[t, a, b, c]`:

| t | Name | Meaning |
|---|---|---|
| 0 | DEFAULT | `00 00 00 00` = unassigned |
| 1 | MOUSE | opaque (preserve) |
| 2 | KEYBOARD | `02 mm uu 00`, where uu = HID keyboard usage and mm = HID modifier bitmask (see below). Example: Esc = `02 00 29 00`. The Fn key on this board is `02 00 AF 00`. |
| 3 | CONSUMER | `03 lo hi 00`, consumer usage u16. Example: `03 92 01 00` = AL Calculator. The hub's media palette offers 0x0CD, 0x0B7, 0x0B6, 0x0B5, 0x0E9, 0x0EA, 0x0E2, 0x223, 0x227, 0x225, 0x224, 0x22A, 0x221, 0x183, 0x18A, 0x192, 0x194 (`src-p2rkdD8u.js@71288` ``Rt=new Map([[205,`Play/Pause`]``); the record itself carries any u16, and the board's consumer report accepts 0x000–0x33C (§1.1). |
| 4 | SYSTEM | opaque |
| 5 | EXTRA | opaque |
| 6 | MACRO | `06 nn 00 00`, nn = macro id 0..99 |
| 7 | CB (combo) | `07 k1 k2 k3`. Not offered on this board. |
| 8 | DKS | `08 ss 00 00`, ss = DKS slot 0..63 (`owned-BuJoYyKq.js@35769` `DKS,e&255`) |
| 9 | MT (mod-tap) | `09 HOLD TAP tt`, holdMs = tt×10. Range 10..1000 ms, default 400 (`owned-BuJoYyKq.js@36332` `MT,e&255`). Parameter order is verified twice: by the summary label `ui-C7M0IjYH.js@110215` `hold:ne(t),tap:ne(r)`, and by the editor, where the input bound to byte 1 is labelled `hfd.adv.mt.hold` and the one bound to byte 2 `hfd.adv.mt.tap`. The hub pre-fills HOLD with the key's own usage. HOLD and TAP are HID keyboard usages. |
| 10 | TGL (toggle) | `0A uu 00 00`, uu = HID usage of the key |
| 11 | SOCD (snap tap) | `0B mm k1 k2`, mode mm = 1..4 (`owned-BuJoYyKq.js@35961` `Snap Tap вне диапазона`). The **same record is written to both keys** of the pair. VERIFIED (FW 1.17): k1 and k2 are **HID usages**, not key indices; the user's A/D pair reads `0B 03 04 07` at both index 49 (A) and index 51 (D). Mode semantics are still UNVERIFIED. The hub's locale table lists, in order, "Last input priority", "Key 1 priority", "Key 2 priority", "Neutral" (`index-DQigh6Ga.js@535335` `` [n(956)]:` ``), and the Fn-layer palette offers modes `[1,2,3,4]` (`owned-BuJoYyKq.js@60502` `for(let e of[1,2,3,4])`). The list order would suggest 1 = last input, but **observed on hardware (FW 1.17): the factory A/D record with mode 3 was displayed by the hub as "Приоритет последнего ввода" (Last input priority)**, so the order is NOT 1..4 = list order; mode 3 = last input. Modes 1, 2, 4 remain UNVERIFIED (set each via the hub and read back to pin them). The factory state of this unit ships with an A/D SOCD pair (mode 3) — CS2/FACEIT reject SOCD, so the GUI should surface it prominently. |
| 12 | RS (Rappy Snappy) | `0C 00 k1 k2`, written identically to both keys (`owned-BuJoYyKq.js@36543` `RS,0,e&255`) |
| 13 | FUNC | `0D 00 00 id`, vendor function id (e.g. Fn-layer lighting / Win lock). Preserve as opaque. |
| 14 | END, 15 MPT | opaque |

Advanced keys (types 7–12) are written on layer 0 only. The hub preserves any record it cannot decode
byte for byte.

**Key + modifier in one record (KEYBOARD byte 1).** The hub's record display treats byte 1 of a
KEYBOARD record as the HID modifier bitmask and renders e.g. `02 01 43 00` as "Ctrl+F10":
- Modifier names in bit order: `owned-BuJoYyKq.js@51695` ``In=[`Ctrl` `` = Ctrl, Shift, Alt, Win, RCtrl,
  RShift, RAlt, RWin, i.e. bit0 LCtrl … bit7 RGui, the standard HID modifier byte.
- Rendering: `owned-BuJoYyKq.js@52230` ``case R.KEYBOARD:return{kind:`text`,text:X(a,s)}`` (a = byte 1,
  s = byte 2).
- The typed decoder treats a non-zero byte 1 as opaque and preserves it (`owned-BuJoYyKq.js@34820`
  `return r||a?B(e):d(i)`); the hub's own encoder always writes byte 1 = 0. So the hub can show such
  records but never creates them.
- This repo writes them for Fn-layer combos (`key:Ctrl+F10` = `02 01 43 00`, `key:Shift+PrintScreen` =
  `02 02 46 00`). **UNVERIFIED** whether the firmware sends the modifier: the readback compare can only show
  that the record was stored. Check with `evtest`/`wev` after writing.
- CB (type 7, `07 k1 k2 k3`, up to three keyboard usages) is the hub's other combo form, but it is offered
  only behind the `comboKeys` quirk (shifted symbols, `owned-BuJoYyKq.js@58612` `action:ut(225,0,t)`) and
  this board's advancedKeys list has no CB. Not used.
- Fallback if the firmware ignores byte 1, without macros: map the key to an otherwise unused usage
  (e.g. `key:F16`) and bind that to the combo on the host.

**FUNC records (type 13)** carry a 24-bit vendor function id (`0D a b c`, id = a<<16 | b<<8 | c); the
hub's id table starts at `owned-BuJoYyKq.js@46438` `An=new Map([[1,n(51)]`. On the user's factory Fn
layer (VERIFIED FW 1.17, layer 1 == GET_DEFAULT_FN_KEY_MATRIX): Fn+Delete (106) = id 1 `hfd.func.reset`,
Fn+Win (81) = id 22 `hfd.func.winLock`, Fn+G (53) = 87, Fn+K (56) = 91, Fn+L (57) = 27 `frontEffect`,
Fn+M (71) = 9 `macMode`, Fn+RShift (75) = 11 `effect`, Fn+Enter (76) = 12 `color`, Fn+arrows (88–91) =
16/14/13/15 (lighting slower/dimmer/brighter/faster), Fn+End (107) = Pause `02 00 48 00`, Fn+Backspace (92) =
ScrollLock. Fn+F1…F12 are **unassigned** (`00 00 00 00`); most other keys repeat their base usage. The
Fn-layer writer (`type84_proto::fn_layer`) never overwrites FUNC, macro, DKS or advanced records and never
touches the Fn key (85).

Captured default layer 0 (FW 1.17): indices 0–12 = Esc, F1–F12 (`02 00 29 00`, `02 00 3A 00` …), and
index 99 = PrintScreen (0x46).

VERIFIED (FW 1.17) on the user's unit:
- Layer 0 holds the expected usage on every one of the 84 physical keys (Fn = `02 00 AF 00`), except
  A and D, which carry the SOCD record above.
- Layer 1 (GET_FN_KEY) is byte-identical to GET_DEFAULT_FN_KEY_MATRIX.
- Several non-physical indices carry keyboard usages, apparently a full-size template: 29 NumLock,
  30 KP/, 31 KP*, 45–47 KP7–9, 61–63 KP4–6, 77–79 KP1–3, 86 Menu, 93 KP0, 94 KP., 95 KPEnter,
  96 Calculator (consumer 0x0192), 97 NonUS#, 98 NonUS\, 100 ScrollLock, 101 RGui, 102 Pause,
  109 `03 56 00 00`, 110 KP+, 111 `02 00 00 00`. Preserve them.

### 3.4 GET_MAGNETIC_AXIS_RT (0x17), 1024 bytes: per-key Hall configuration

Sources:
- Read/write calls: `driver-CWjFZVQ-.js@7715` `cmd:y.GET_MAGNETIC_AXIS_RT,contentSize:W`, with W = 1024
  (`src-u8pKurse.js@6654` `R=1024`).
- Record codec `Un`/`Wn` in `src-u8pKurse.js`; flag constants at `@6661` `In=1,Ln=2,Rn=252`.

Layout: 128 records × 8 bytes; `record[i] = block[8i .. 8i+8]`, where `i` is the key index (§3.9).

| Byte | Field | Encoding |
|---|---|---|
| 0 | axisType | u8. Preserve. The hub only substitutes a quirk default when it is 0, and this board has no such quirk. The capture shows 0. |
| 1 | flags | bit0 = **wholeTravel** ("continuous RT", UI "на всём ходе"). bit1 = **rampage**. Bits 2–7 (mask 0xFC) must be preserved verbatim. |
| 2..3 | actuation | u16 ÷ `triggerScale` → mm |
| 4..5 | rtPress | u16 ÷ `sensScale` → mm. RT press sensitivity. |
| 6..7 | rtRelease | u16 ÷ `sensScale` → mm. RT release sensitivity. |

Units depend on device-info byte 29 (rtPrecision) (`src-u8pKurse.js@6700` `trigger:e===2?1e3:100`,
`@6722` `sensitivity:e>0?1e3:100`):
- `triggerScale = (rtPrecision == 2) ? 1000 : 100`
- `sensScale = (rtPrecision > 0) ? 1000 : 100`
- With the captured rtPrecision = 0, both are 0.01 mm per LSB. Example: actuation `8C 00` = 140 =
  1.40 mm (capture default).
- Encode with `round(mm * scale)`.

Semantics (hub `toSettings` / `fromSettings`; the old vendor app behaves the same):
- **RT enabled ⇔ rtPress > 0 || rtRelease > 0.** There is no enable bit.
- **Disabling RT** writes rtPress = rtRelease = 0.
- **Enabling RT with a missing value** uses 0.10 mm (RT_DEFAULT_SENSITIVITY).
- **"Separate press/release"** is a UI notion only: press ≠ release. Both fields always exist.
- **There is no separate release point.** The hub reports release = actuation.
- **UI ranges for this board** (quirk defaults):
  - actuation 0.10–3.20 mm, step 0.01, preset 1.20
  - sensitivity 0.01–3.20 mm, step 0.01, preset 0.10
- **Writable record indices** are those below 128 (`driver-CWjFZVQ-.js@10339` `r>=128`).

**The «на всём ходе» toggle is flags bit0 (wholeTravel).** RESOLVED, statically and on hardware:
- The hub's RT panel renders a checkbox whose state is `wholeTravel` and whose change handler writes
  `wholeTravel` (`ui-C7M0IjYH.js@38909` `R(m)?.wholeTravel===!0`, `@39052`
  `onChange:e=>void pe({wholeTravel:e})`). The adjacent locale entries are the label and hint
  «На всём ходе» / «Rapid Trigger работает и до точки срабатывания.» (`index-DQigh6Ga.js@339665`
  ``[n(604)]:`На всём ходе` ``; English `@518614` ``[n(604)]:`Whole travel` ``, hint "Rapid Trigger also
  works before the actuation point.").
- VERIFIED (FW 1.17): bit0 is set on exactly the seven keys where the user enabled it, and clear on
  every other key.

VERIFIED (FW 1.17) records for docs/GROUND_TRUTH.md (rtPrecision = 0):
- W/A/S/D/LShift/LCtrl/Space (0.80 mm, RT 0.30/0.30, whole travel on): `00 01 50 00 1E 00 1E 00`,
  exactly as predicted (axisType 0).
- All other 77 physical keys (1.50 mm, RT off): `00 00 96 00 00 00 00 00`. The flags byte is **0x00**;
  the earlier guess of 0xFF was wrong. Bits 2–7 were 0 on every key.
- The 44 non-physical indices (13–15, 29–31, 45–47, 61–63, 77–79, 86, 93–98, 100–102, 109–127) read
  `00 00 C8 00 00 00 00 00` (2.00 mm). In the old capture (py author's board, unknown history) all 44
  of those records were zero, so something (the current hub or the firmware) writes them. Preserve
  them as read.

### 3.5 GET_LED_EFFECT (0x13), 16 bytes

Codec: `owned-BuJoYyKq.js@38479` `driverSetting:4`, `@38608` `checkCodeL:14,checkCodeH:15`.

| Byte | Field |
|---|---|
| 0 | mode (effect id; 0 = off) |
| 1, 2, 3 | red, green, blue. **Write-only on FW 1.17**: applied, but always read back as `FF FF FF` (see below) |
| 4 | driverSetting (hub always writes 0xFF) |
| 5, 6, 7 | secondary R, G, B |
| 8 | colorMode: 0 = single colour, 1 = multicolour (hub labels `hfd.light.singleColor` / `multiColor`). Picking a colour in the hub also writes colorMode 0 (`ui-C7M0IjYH.js@123593` `colorMode:0,red:t,green:n,blue:r`). **Write-only on FW 1.17**: always read back as 1 |
| 9 | brightness, step 0..5 |
| 10 | speed, step 0..5 |
| 11 | direction: 0 = forward, 1 = reverse (`hfd.light.forward` / `reverse`), only for effects with a direction (ripple, wave, serpentine, stream, slantRain) |
| 12 | effectModeType |
| 13 | (0) |
| 14, 15 | check code `AA 55` (written by the hub) |

Effect ids. The firmware byte is the hub's `Kt.value`, written unchanged. The hub's model entry for
0c45:80d6 ("IO Type 84 Magnetic Black/White", `index-DQigh6Ga.js@202612`) declares no lighting quirks:
no `lightingParameterEncodings`, `lightingModeReadbacks`, `hiddenEffects` or `extraEffects`. So
`Ut`/`Rt` (`owned-BuJoYyKq.js`) encode the mode, brightness and speed as is. The labels below are
resolved from the obfuscated string tables (`hfd.effect.*` keys, then the ru table in
`index-DQigh6Ga.js`).

| Byte | hub key | hub label (ru) | Hardware (FW 1.17) |
|---|---|---|---|
| 1 | static | Статичный цвет | written and read back as 1 |
| 2 | onPress | Включение при нажатии | |
| 3 | offPress | Выключение при нажатии | |
| 4 | stars | Звёздное небо | |
| 5 | snow | Падающий снег | |
| 6 | riot | Цветочное великолепие | |
| 7 | breathing | Динамическое дыхание | |
| 8 | colorWheel | Спектральный цикл | |
| 9 | fountain | Цветной фонтан | |
| 10 | ripple | Многоцветные волны | |
| 11 | wave | По волнам | read back as 11. Looked like a blue *breathing* effect to the user (written #00A0FF single colour); see §8 |
| 12 | serpentine | Горный серпантин | |
| 13 | flash | Горизонтальные разряды | |
| 14 | double | Двух зайцев одним выстрелом | |
| 15 | circles | Расходящаяся рябь | VERIFIED: the hub's «Расходящаяся рябь» reads back as 15. Factory effect, reacts to key presses |
| 16 | stream | Непрерывный поток | |
| 17 | ridges | Горные хребты | |
| 18 | slantRain | Косой дождь | |
| 19 | backAndForth | Туда и обратно | |
| 128 | custom | Пользовательский | VERIFIED: written as 128, **read back as 20** |
| 20 | (reported custom) | shown as Пользовательский (`Xt` maps 20/21/22 to the 128 entry) | VERIFIED: shows the stored per-key block (§3.6) |

- Source: `owned-BuJoYyKq.js@41601` `value:128,labelKey`.
- Ids 23, 24, 25 and 254 are "extra" effects, not enabled for this board.
- Per-key colours are only meaningful and writable when mode ∈ {20, 21, 22, 128}
  (`owned-BuJoYyKq.js@38412` `e===20||e===21||e===22||e===128`). The hub refuses a per-key write otherwise
  (`driver-CWjFZVQ-.js@16676` `!T(o.mode)`), and so does this repo.
- VERIFIED (FW 1.17), read side: the user's unit returns `0F FF FF FF 00 00 00 00 01 05 04 00 00 00 00 00`:
  byte 4 (driverSetting) and bytes 14..15 (check code) read **0**, although the hub always writes `FF` and
  `AA 55`.
- VERIFIED (FW 1.17), what a GET after a 0x23 write reports (writes on 2026-11-09/10 through this repo):

  | Written (mode, colour, colorMode, bright, speed, dir) | Read back (bytes 0..11) | Board showed |
  |---|---|---|
  | 11, #00A0FF, 0, 5, 4, 0 (twice) | `0B FF FF FF 00 00 00 00 01 05 04 00` | blue breathing-like effect |
  | 128, (kept) FFFFFF, 1, 4, 4, 0 | `14 FF FF FF 00 00 00 00 01 04 04 00` | per-key colours (one key, H) |
  | 11, #00FF00, 0, 3, 2, 1 | `0B FF FF FF 00 00 00 00 01 03 02 01` | (not observed) |
  | 1, #FF0000, 0, 4, 4, 0 | `01 FF FF FF 00 00 00 00 01 04 04 00` | (not observed) |
  | 128, FFFFFF, 1, 4, 4, 0 | `14 FF FF FF 00 00 00 00 01 04 04 00` | per-key colours |
  | **20** (raw), FFFFFF, 1, 5, 4, 0 | `14 FF FF FF 00 00 00 00 01 05 04 00` | per-key colours; 0x14 unchanged |
  | 128, FFFFFF, 1, 4, 4, 0 | `14 FF FF FF 00 00 00 00 01 04 04 00 00 00 00 00` | (final state) |

  Consequences:
  - The mode, brightness, speed and direction are reported as written. The exception is custom 128,
    which reads back as **20**.
  - The colour and colorMode are **not reported**: always `FF FF FF` / 1, even though the board applies
    the written colour (blue was visible).
  - The secondary colour and effectModeType read 0. The hub writes them as last read, and so does this
    repo.

  This repo's post-write check therefore compares the mode (128 ≡ 20) plus bytes 5..7 and 9..12
  (`type84_proto::led::effect_verify_eq`) and prints "colour not verifiable by readback". Diffs and
  skip decisions still compare bytes 0..3 and 5..12, with 128 ≡ 20 (`effect_fields_eq`).
- The hub does not verify a 0x23 write either. After the write it caches the effect it wrote
  (`driver-CWjFZVQ-.js` `E=Promise.resolve(i)`). After a mode change it waits 500 ms and shows whatever
  GET returns (`rememberMode`). Its snapshot restore compares only *before* writing, to skip an
  unchanged block. So the hub UI shows white/multicolour after it re-reads.

### 3.5a Real-time / streaming lighting: none on this board

Searched in the hub bundle for any host-to-device lighting channel that does not persist (music/rhythm
mode, screen sync, "direct" mode, per-frame streaming, notification frames, TFT). Result: **none exists for
this board.** Evidence:
- The HFD opcode table (§1.7) has exactly two lighting writes, SET_LED_EFFECT (0x23) and
  SET_CUSTOM_LED_DATA (0x24), and the hub treats both as stored settings (no separate "preview" form).
- 0x33 GET_ALL_LIGHTS_RGB and 0x37/0x3B are **reads** (device-to-host colour readback, the hub's "Lighting
  now" view, which it calls streaming). The hub disables even that for magnetic boards
  (`driver-CWjFZVQ-.js@35230` `liveLighting??` falls back to `switches !== magnetic`).
- 0x41 SET_LED_USER_ANIMATION, 0x50 SET_TFT_USER_ANIMATION, 0x51 SET_TFT_BUILT_IN_INDEX, 0x2A/0x3D dot
  matrix and 0x34 clock serve screen/LED-matrix boards. They are uploads of whole animations, the ACK of
  0x50 is `responseCmd` 0x41 (`driver-CWjFZVQ-.js@19185` `responseCmd:y.SET_LED_USER_ANIMATION`), and this
  board reports `tftMaxFrames = gifMaxFrames = ledMaxFrames = 0` (device info payload 22..27,
  `owned-BuJoYyKq.js@30412` `ledMaxFrames:r(26)`) and has no 0xFF67 screen interface (§1.1).
- "Sound-reactive lighting" appears only as vendor FUNC ids for Fn-layer keys (ids 54–56, 130–133, 160),
  i.e. firmware-side modes toggled from the keyboard, with no host data channel.
- Consequence: any host-driven animation would be a stream of persistent 0x23/0x24 writes. This repo does
  not do that. `type84 anim flash` instead switches to a built-in effect and back: two single-frame 0x23
  writes per use (docs/CLI.md).

### 3.6 GET_CUSTOM_LED_DATA (0x14), 512 bytes

- 128 × `[ledId, R, G, B]`. Record index = LED index, which on this board equals the key index (the
  KLE layout defines no separate `led` field).
- VERIFIED (FW 1.17): the block reads back **as stored, in any effect mode**, ledId included.
  - A unit that never had per-key colours written returns all 512 bytes as 0, ledId included. That
    was the factory state, not a read quirk.
  - After the hub wrote one key (H = index 54, `36 FF 00 00`), the block read back with `ledId = index`
    while the board was in mode 15.
  - After this repo's 0x24 write it reads back exactly as written.
  - In mode 20 the board lights exactly the stored block: with only H red, only H lit.
- On write, the hub sets `ledId = record index` (position addressing).
- Capture cross-check: the old app colouring W wrote record 34 = `22 00 FF 00`. W is key index 34.

### 3.7 GET_MACRO (0x15): addressed reads, not one block

Read algorithm (`src-u8pKurse.js@10465` `cmd:w.GET_MACRO,contentSize:400`):
1. `transfer(GET_MACRO, contentSize=400, addrStart=0)` returns the table: 100 × u32 LE absolute
   offsets. 0 means the macro slot is empty.
2. For each non-zero offset `o`, call `transfer(GET_MACRO, 4, addrStart=o)`. Then `count = u16(b0,b1) / 2`.
3. Call `transfer(GET_MACRO, count*4, addrStart=o+4)` to get `count` actions of 4 bytes each:
   - `delay` = u16 LE ms (0..65535)
   - `code` = u8
   - `flags` = u8: press = bit7, kind = (flags >> 4) & 7
     - Encode: kind 1 or 2 (mouse) → press 0x90, release 0x10. Otherwise (kind 3, key) → press 0xB0,
       release 0x30 (`owned-BuJoYyKq.js@44737` `144:16`).

Block area:
- Starts at offset 400 and holds at most 512 bytes (default macroSpace).
- Each macro costs `4 + 4×actions`.
- At most 100 macros.

Addresses above 0xFFFF are impossible: the header addr field is 16-bit.

VERIFIED (FW 1.17): with no macros defined, the table read returns 400 zero bytes in 8 packets
(7×56 + 8, last packet at addr 0x188 with byte 6 = 1). The body walk (steps 2–3) has not been
exercised on hardware.

### 3.8 GET_MAGNETIC_AXIS_DKS_DATA (0x18), 1024 bytes

64 slots × 16 bytes (`owned-BuJoYyKq.js@37247` `yt=1024`, `@37439` `Ct=[5,7,9,11],wt=12`):

| Byte | Field |
|---|---|
| 0, 1 | makeFirst, makeSecond: press points 1 and 2, in **0.1 mm** units (UI 0–4.0 mm: `ui-C7M0IjYH.js@100854` `Math.min(40,Math.round(t*10))`) |
| 2, 3 | breakFirst, breakSecond: release points 1 and 2, 0.1 mm |
| 4, 6, 8, 10 | reserved. The old app writes 0; the hub preserves them. |
| 5, 7, 9, 11 | action 1..4: HID keyboard usage |
| 12..15 | per-phase state byte (phase p = 12+p): bit a (0..3) = action a "tap", bit a+4 = action a "hold" |

- A slot is empty when bytes 0..3 are all 0.
- A key uses slot `s` via keymap record `08 s 00 00`.
- Hub default for a new DKS: 1.0 / 3.6 / 3.6 / 1.0 mm, with action 1 = the key's own usage set to
  hold in phase 0.

### 3.9 Key index ↔ physical key (authoritative)

The index is the second number of the `"r,c"` label in the hub's KLE layout:
- Parser: `index-DQigh6Ga.js@166921` `/^(\d+)\s*,\s*(\d+)/`.
- Addressing: `driver-CWjFZVQ-.js@27253` `` addressing:(e,t)=>({kind:`index`,index:t}) ``.
- Layout array for this board: `index-DQigh6Ga.js@981642` `` MC=[[`0,0 ``. A copy is already at
  `docs/type84_layout.kle.json`.

The index equals matrix `row*16 + col`. The same index addresses the keymap record (×4), the RT record
(×8) and the LED record (×4).

| Row | Keys (index) |
|---|---|
| F-row | Esc 0, F1 1, F2 2, F3 3, F4 4, F5 5, F6 6, F7 7, F8 8, F9 9, F10 10, F11 11, F12 12, Print 99, Home 104, End 107 |
| Number | ` 16, 1 17, 2 18, 3 19, 4 20, 5 21, 6 22, 7 23, 8 24, 9 25, 0 26, - 27, = 28, Backspace 92, Insert 103, PgUp 105 |
| Top | Tab 32, Q 33, W 34, E 35, R 36, T 37, Y 38, U 39, I 40, O 41, P 42, [ 43, ] 44, \ 60, Delete 106, PgDn 108 |
| Home | Caps 48, A 49, S 50, D 51, F 52, G 53, H 54, J 55, K 56, L 57, ; 58, ' 59, Enter 76 |
| Shift | LShift 64, Z 65, X 66, C 67, V 68, B 69, N 70, M 71, , 72, . 73, / 74, RShift 75, Up 90 |
| Bottom | LCtrl 80, LWin 81, LAlt 82, Space 83, RAlt 84, Fn 85, RCtrl 87, Left 88, Down 89, Right 91 |

Cross-check against the captured default keymap: 99 = 0x46 PrintScreen, 103 = Insert, 104 = Home,
105 = PgUp, 106 = Delete, 107 = End, 108 = PgDn, 92 = Backspace, 76 = Enter, 90 = Up. Every value
matches.

Some indices carry records but are not physical keys: 13–15, 29–31, 45–47, 61–63, 77–79, 86, 93–98,
100–102, 109–127 (44 in total; VERIFIED on FW 1.17 that all of them hold RT records, and many hold
keymap records, see §3.3 and §3.4).
- Preserve them.
- Keymap writes are only accepted for index < 126 (`driver-CWjFZVQ-.js@6198` `s>=126`).

---

## 4. WRITE commands

General rules:
- **Same framing as reads**, with the data in the payload and byte 6 = 1 on the last packet. The ACK
  is `55 <cmd> <len> <addr>` per packet.
- **No separate save/commit-to-flash command exists.** Every SET is treated as stored: the hub has no
  RAM-only variant of any write, and the hub-set lighting (circles) persisted across host sessions (hub,
  then this CLI). Persistence is VERIFIED for 0x24 (per-key colours survive a replug); 0x23 is presumed stored too. The hub's
  `persisted:true` result is not proof of a flash write by itself (it returns the same flag for the clock
  write, `driver-CWjFZVQ-.js@37115` `SET_TEMPORARY_COMMAND_DATA,contentSize:24`). The old-app ACK latency of
  about 19 ms per packet is consistent with a flash write per packet. Treat every write as a flash write.
- **Always read-modify-write the whole block.** The hub never sends partial blocks; every SET sends
  the full block starting at addr 0.
- **Do not write while `firmwareStatus != 0`.**

### 4.1 SET_GAME_MODE (0x21), 56 bytes

- Start from the last GET_GAME_MODE raw 56 bytes. Patch only the intended bytes (§3.2) and write all
  56.
  - Dead zones: `round(mm*100)`.
  - Unknown bytes are kept as read.
- If `reportRate` changed, follow §2.2 (re-enumeration, verify by readback).
- The hub batches UI edits for 220 ms before writing.
- The old app zero-filled the 56 bytes and set only known fields. Do not copy that.

### 4.2 SET_KEY (0x22) / SET_FN_KEY (0x26), 512 bytes

- Read-modify-write the whole layer (`driver-CWjFZVQ-.js@2148` `J=[y.SET_KEY,y.SET_FN_KEY]`).
- The hub batches edits for 180 ms, writes the block, and does **no** readback for a normal key edit
  (only the macro-unbinding path reads back and compares).
- **SET_FN_KEY: IMPLEMENTED in this repo, VERIFIED on hardware (FW 1.17)** (`type84_proto::fn_layer`,
  `WriteSession::apply_plan`): 10 frames `AA 26 len addr 00 last 00` (9 × 56 bytes, then 8 bytes at
  addr 0x1F8 with byte 6 = 1); ACK `55 26 len addr` with addr matched (0x26 is in the hub's checkAddr set);
  2000 ms per packet; then 40 ms, GET_FN_KEY and a byte-for-byte compare. Only physical keys below 126,
  never the Fn key (85), never over a FUNC/macro/DKS/advanced record; the factory record comes from
  GET_DEFAULT_FN_KEY_MATRIX (action `factory`).
- SET_KEY (layer 0): **not implemented** (not on the allowlist).
- When a macro is deleted, the hub replaces keys bound to it:
  - on layer 0, with `00 00 00 00`;
  - on layer 1, with the record from GET_DEFAULT_FN_KEY_MATRIX.
  It then reads back and compares.
- Clearing a SOCD/RS pair clears both keys.

### 4.3 SET_MAGNETIC_AXIS_RT (0x27), 1024 bytes

Procedure (`driver-CWjFZVQ-.js@7821` `cmd:y.SET_MAGNETIC_AXIS_RT,contentSize:W`, `@7274` `Ge=40`):
1. Write the full 1024-byte block.
2. Wait 40 ms.
3. Run GET_MAGNETIC_AXIS_RT and compare. On mismatch, write the previous block back and report
   `hfd.rtReadbackMismatch`.

**IMPLEMENTED in this repo, VERIFIED on hardware (FW 1.17, §10)** (`type84_proto::hall::encode_write_frames`,
`type84_hid::WriteSession::apply_hall_table`):
- 19 frames `AA 27 len addr_lo addr_hi 00 last 00 | payload`: 18 × 56 bytes at addr 0x000, 0x038, …,
  0x3B8, then 16 bytes at addr 0x3F0 with byte 6 = 1 (`AA 27 10 F0 03 00 01 00`). Payload = the
  1024-byte block (§3.4) in order, zero padded. Unit test: for a backup, write frame *i* equals the
  board's read response frame *i* in bytes 2..63.
- One packet in flight; ACK = `55 27 len addr` with addr and len matched; 2000 ms per packet;
  up to 3 resends on timeout; a `55 FC 05` device-reset frame aborts the write (no resend).
- Then 40 ms, GET_MAGNETIC_AXIS_RT, byte-for-byte compare.
- Deliberate departure from the hub: **no automatic rollback** on mismatch (it would be a second,
  equally unverified write). The error names the automatic pre-write backup and the command
  `type84 restore <backup> --only hall`.
- Observed on hardware (§10): every 0x27 packet was ACKed with the echoed addr and len and the readback
  matched. Not measured: per-packet latency (old app: about 19 ms) and whether a notification follows.

### 4.4 SET_MAGNETIC_AXIS_DKS_DATA (0x28), 1024 bytes

Same pattern as RT: write, wait 40 ms, read back, compare, roll back on mismatch.
- `src-u8pKurse.js@12506` `cmd:w.SET_MAGNETIC_AXIS_DKS_DATA,contentSize:O,data:n`
- `@11871` `ur=40`

### 4.5 SET_LED_EFFECT (0x23), 16 bytes

- Layout as §3.5. Byte 4 = 0xFF, byte 13 = 0, bytes 14..15 = `AA 55` (encoder `owned-BuJoYyKq.js@39347`
  `checkCodeL,170)`; written from `driver-CWjFZVQ-.js@13944` `cmd:y.SET_LED_EFFECT,contentSize:16`).
- The hub batches edits for 220 ms and patches only mode, colour (with colorMode 0), brightness, speed,
  direction; other bytes keep the last read.
- brightness and speed are steps 0..5.
- After writing, the hub waits 500 ms before trusting a readback (LIGHTING_APPLY_DELAY_MS).
- Captured old-app write: `AA 23 10 00 00 00 01 00 | 80 FF 00 00 FF 00 00 00 00 03 05 00 00 00 AA 55`.
  This is mode custom, red, brightness 3, speed 5.
- **VERIFIED on hardware (FW 1.17)** (`type84_proto::led::EffectPatch`).
  - One frame `AA 23 10 00 00 00 01 00 | …`. A unit test reproduces the captured old-app write byte for
    byte.
  - ACK `55 23 10`, addr not matched, as in the hub. 1000 ms timeout.
  - Then 500 ms, GET_LED_EFFECT, and a compare of the fields the board reports (§3.5): the mode
    (128 ≡ 20), bytes 5..7 and 9..12. Not the colour.

### 4.6 SET_CUSTOM_LED_DATA (0x24), 512 bytes

- 128 × `[index, R, G, B]` (`driver-CWjFZVQ-.js@16929` `cmd:y.SET_CUSTOM_LED_DATA,contentSize:512`).
- Only meaningful when the effect mode is custom (128, or 20/21/22).
- **VERIFIED on hardware (FW 1.17)**.
  - 10 frames like §4.2. Every record is re-encoded with `ledId = index`.
  - Refused unless the effect after the write is a custom mode.
  - The readback compares colours, and it matched. ledId also reads back as written (§3.6).
  - VERIFIED: the per-key colours survive a replug (user test, 2026-11-10), so 0x24 is stored in
    flash, and every write costs flash wear.
- Order: the hub's snapshot restore writes 0x24 before the effect (§7.2) and never reads 0x24 back.
  This repo writes the effect first. GET_CUSTOM_LED_DATA returns the stored block in any mode (§3.6),
  so the order does not affect the readback.

### 4.7 SET_MACRO (0x25)

Write algorithm from `src-u8pKurse.js@11413` `cmd:w.SET_MACRO,contentSize:400`:
1. **Build the table** (400 bytes): for macro id k with ≥1 action, `u32 LE offset` at 4k. Offsets
   start at 400 and are packed in id order. Ids without actions get 0.
2. **Build the blocks**, one per macro, packed: `u16 LE (actions*2)`, `u16 0`, then actions of 4 bytes
   (`delay u16 LE, code, flags`).
3. **Send the table:** `transfer(SET_MACRO, 400, addrStart 0, markLastPacket=false)`. Byte 6 = 0 on
   all of its packets (`src-u8pKurse.js@11464` `markLastPacket:!1`).
4. **Send the blocks**, if their length is above 0: `transfer(SET_MACRO, blocksLen, addrStart 400)`.
   The final packet has byte 6 = 1.
   - Edge case, UNVERIFIED: when there are no macros, the hub sends only the table and never sets the
     last-packet flag.

Constraints:
- Total block bytes must be ≤ 512 and macro count ≤ 100.
- Before writing, unbind keys that reference removed macro ids (§4.2).

---

## 5. DANGEROUS: blocklist (do not implement)

**Recommendation: implement an allowlist, not a blocklist.** Only the opcodes the hub's HFD driver
actually sends to this board may ever appear in byte 1 of a frame sent by the app:
- reads: 0x10–0x18, 0x1C
- writes: 0x21–0x28

0x1F is defined but never sent, so it stays off the allowlist.

This repo is stricter still: the read-only transport allows only the reads, and the separate opt-in
`WriteSession` allows the reads plus, per call, the write opcode of the one section being written
(0x23, 0x24, 0x26 or 0x27; full-block chunk headers only). 0x21, 0x22, 0x25 and 0x28 are not
implemented anywhere (`docs/SAFETY.md`).

Everything else must be refused in the transport layer. In addition:

| What | Why | Citation |
|---|---|---|
| 0x0F SET_FACTORY_RESET | `len` byte = scope: 1 keys, 2 lighting, 4 macros, **5 calibration**, 255 all | `driver-CWjFZVQ-.js@24047` `SET_FACTORY_RESET,len:t`; `owned-BuJoYyKq.js@61545` `KEYS:1,LIGHTING:2,MACROS:4,CALIBRATION:5,ALL:255` |
| 0x64/0x65 SET_CALIBRATION_ON/OFF, 0x69/0x6A …_V2 | enters/leaves user calibration (rewrites Hall calibration) | `driver-CWjFZVQ-.js@26610` `SET_CALIBRATION_ON,contentSize:0`; `@26041` `SET_CALIBRATION_ON_V2` |
| 0x66/0x67 SET_SIMULATION_TEST_ON/OFF | factory test mode | `owned-BuJoYyKq.js@22318` `SET_SIMULATION_TEST_ON:102` |
| 0x68 GET_MAGNETIC_AXIS_STATUS | read-only, but only used inside calibration V2; not needed | `owned-BuJoYyKq.js@22373` `GET_MAGNETIC_AXIS_STATUS:104` |
| 0x80–0x86 OTA_* (system info, verify, **ENTER_BOOT 0x82**, **SEND_FIRMWARE 0x83**, SN, **SWITCH_APP_PARTITION 0x85** with A=0x41/B=0x42, **SET_SN 0x86**) | bootloader entry ("в загрузчик") / firmware update / identity change. 0x80 and 0x84 are reads, but their frames use a raw custom header; no reason to send them. | `owned-BuJoYyKq.js@22596` `OTA_GET_DEVICE_SYSTEM_INFO:128,OTA_VERIFY_FIRMWARE_INFO:129,OTA_DEVICE_ENTER_BOOT:130`; `owned-BuJoYyKq.js@22760` `OTA_SET_DEVICE_SN:134`; `driver-CWjFZVQ-.js@23481` `OTA_SEND_FIRMWARE_INFO,data:a`; `@20013` `A:65,B:66` |
| 0x2A, 0x34, 0x3D, 0x41, 0x50, 0x51 | dot-matrix / clock / TFT / LED-animation writes for other boards | `owned-BuJoYyKq.js@22805` `GET_DOT_MATRIX_CONFIG:61`; `owned-BuJoYyKq.js@22905` `SET_LED_USER_ANIMATION:65` |
| Any 8-byte frame starting `AA 42 89 5A FF 71 62 CC`, `AA 42 89 5A FF 75 62 CC` or `AA 55 A5 5A FF 00 33 CC` | magic boot/ISP sequences present in the HFD driver (unused constants) and the SONiX ISP | `driver-CWjFZVQ-.js@21097` `[170,66,137,90,255,117,98,204]` |
| **Any HID feature report** (`send_feature_report`/`get_feature_report`) and the 0xFF67 interface | the main bundle's SONiX ISP flasher (chip SN34F280) probes every interface with feature report 0. Its commands are GET_VERSION 1, ENABLE_PROGRAM 2, **CHIP_ERASE 3**, ERASE_RESULT 4, BEGIN_WRITE 5, FINISH 7. | `index-DQigh6Ga.js@919000` `GET_VERSION:1,ENABLE_PROGRAM:2,CHIP_ERASE:3`; `@919142` `[170,85,165,90,255,0,51,204]` |

---

## 6. Discrepancies: Python repo vs current hub

| # | Python repo claim | Current hub / evidence | Verdict |
|---|---|---|---|
| 1 | Hall image is 1008 bytes, plus a "session terminator" `AA 27 10 F0 03 01 00` that carries a commit flag (`PROTOCOL.md` §2) | The block is 1024 bytes (`R=1024`). The "terminator" is just the 19th chunk: len 0x10 = 16 at addr 0x3F0 = 1008, with byte 6 = last-packet flag. The py `hall.py` builder itself emits `AA 27 10 F0 03 00 01 00`. There is no commit command. | Hub. Treat as one 1024-byte block. |
| 2 | Key address = `bank*128 + 5 + col*8`, with a 5-byte bank header (`PROTOCOL.md` §3) | Record offset = `index*8`, index = `row*16+col` from the KLE. The py formula is offset by +3 bytes (a payload-start misalignment), which makes its "word 0 = actuation" really bytes 2..3 of the record. Key A: py 397 maps to hub record 49 bytes 2..3. | Hub. The py `hall.py` code (`<BBHHH`) already agrees with the hub. Only the docs are wrong. |
| 3 | flags bit0 = "Rapid Trigger enabled" (`hall.py`, `hall_rt_closure_report.md`) | RT on ⇔ press>0 or release>0. bit0 = wholeTravel (continuous). bit1 = rampage. The old vendor app confirms this (`u.value=ge.pressRT>0\|\|ge.releaseRT>0`; `isWholeFast` is a separate toggle). py's own `PROTOCOL.md` §4.4 says RT off = zero sensitivities. | Hub. bit0 is NOT the RT switch. |
| 4 | RT units are fixed at 0.01 mm | The scale depends on device-info byte 29 (rtPrecision). It is 0.01 mm only when rtPrecision = 0, which is what the capture shows. | Hub. Read byte 29 and scale accordingly. |
| 5 | Nav cluster hypothesis: End = (6,3), Print = (6,7), Ins = (6,8), Del = (6,9), Home = (6,10), PgUp = (6,11), PgDn = (6,12) | Print 99, Insert 103, Home 104, PgUp 105, Delete 106, End 107, PgDn 108. Confirmed by the captured keymap usages. | Hub. |
| 6 | Device info: "Chip ID 00 00 00 92" at payload 0..3; "Bootloader 1.66" at payload 12..13 | Payload 0 = romSize, 2..3 = macroSpaceSize (ignored). 12..13 = manufacturer code (0x0166). 14..15 = product code. Bootloader version is only available via OTA 0x80, which is blocklisted. | Hub. |
| 7 | Device info "16 bytes"; game mode "16 bytes" (`READ_PROTOCOL.md` table) | 48 and 56 bytes, respectively. | Hub. |
| 8 | W is at LED slot 35 (`PROTOCOL.md` §6) | W = index 34. The py capture `rgb_05_per_key.json` itself writes record 34 = `22 00 FF 00`. | Hub/capture. The py text is off by one. |
| 9 | Key A remap at "slot 50 (0x00C8)" (`PROTOCOL.md` §7) | A = index 49 (record bytes 196..199). The capture `remap_write_a_to_b.json` shows record 49 = `02 00 05 00`. | Hub/capture. |
| 10 | `AA 1C` = "Layer 3 raw table" | GET_DEFAULT_FN_KEY_MATRIX (factory Fn layer, read-only). | Hub. |
| 11 | gameMode byte 1 = Win-key lock (game_mode report) | The hub never writes byte 1. Win lock is a Fn function key. | UNVERIFIED. Do not expose it as Win lock. |
| 12 | Opcode numbers (from the old vendor bundle `layout-classic.js`) | Identical in the current hub for every opcode used here. The current hub drops COMMUNICATION_START/END (1/2), CLEAR_LED_DATA (54) and others. | Same values. No change needed. |
| 13 | py writes Game Mode by zero-filling 56 bytes | The hub does read-modify-write on the raw 56 bytes. | Hub. |

---

## 7. Full backup and restore

Authoritative source: `HfdDevice.snapshot()` / `restore()` (`driver-CWjFZVQ-.js@28149` `snapshot(){`,
`@28234` `for(let n of[y.GET_KEY,y.GET_FN_KEY])`, `@30102` `await d(o.keymap,y.SET_KEY,512`).

### 7.1 Backup (read set)

Do this after GET_DEVICE_INFO succeeds and `firmwareStatus == 0`. Store the device info alongside the
backup, since it carries the version and rtPrecision.

| # | Command | Size | Stored as |
|---|---|---|---|
| 1 | GET_KEY 0x12 | 512 | `keymap[0]` raw |
| 2 | GET_FN_KEY 0x16 | 512 | `keymap[1]` raw |
| 3 | GET_GAME_MODE 0x11 | 56 | `settings` raw |
| 4 | GET_LED_EFFECT 0x13 | 16 | `lighting`. Store the raw 16 bytes as well: the hub keeps only a normalised copy (byte4 = 0xFF, 14..15 = AA 55, unmodelled byte 13 zeroed). |
| 5 | GET_CUSTOM_LED_DATA 0x14 | 512 | `perKeyColors` raw |
| 6 | GET_MACRO 0x15 | table 400 + walk (§3.7) | `macros` decoded list (id, actions). Optionally also keep the raw table and blocks. |
| 7 | GET_MAGNETIC_AXIS_RT 0x17 | 1024 | `actuation` raw |
| 8 | GET_MAGNETIC_AXIS_DKS_DATA 0x18 | 1024 | `dks` raw |

Optional, for reference only (not restored): GET_DEFAULT_FN_KEY_MATRIX 0x1C (512).

### 7.2 Restore order

Validate the sizes first: keymap 512 ×2, settings 56, lighting 16, perKeyColors 512, RT 1024,
DKS 1024, macro ids < 100. Skip any damaged part.

1. SET_KEY 0x22 (512) ← keymap[0]
2. SET_FN_KEY 0x26 (512) ← keymap[1]
3. SET_GAME_MODE 0x21 (56) ← settings. Skip it if it equals the current readback. If reportRate
   differs, the board can re-enumerate in the middle of the restore (§2.2). Either re-open and verify
   the handle before step 4, or move this step to the end. Moving it is a deliberate departure from
   the hub's order, but it is safer for a hidraw client.
4. SET_CUSTOM_LED_DATA 0x24 (512). Only if it differs from the current GET. Re-encode with
   `ledId = index`.
5. SET_LED_EFFECT 0x23 (16). Only if it differs. Then wait 500 ms.
6. SET_MAGNETIC_AXIS_RT 0x27 (1024). Then wait 40 ms, read back and compare.
7. SET_MAGNETIC_AXIS_DKS_DATA 0x28 (1024). Then wait 40 ms, read back and compare.
8. SET_MACRO 0x25: the table, then the blocks (§4.7).
9. Re-read everything (GET_* 1–8) and diff against the backup.

The hub does not add any "commit" step at the end. All writes are persistent as sent.

---

## 8. Open questions / UNVERIFIED items

- RESOLVED: interface layout (§1.1: no 0xFF67 on this unit; 0xFF68 alone on interface 2) and hidapi
  `usage_page` reporting (§1.3).
- RESOLVED for FW 1.17: `rtPrecision` = 0 (§3.1). A newer firmware could report 1 or 2, which
  changes the RT scaling; the Rust code branches on byte 29.
- RESOLVED: the «на всём ходе» toggle is flags bit0 (§3.4).
- SOCD mode values 1..4 meaning (likely order in §3.3). MOUSE/SYSTEM/EXTRA/FUNC record payloads. DKS
  reserved bytes 4/6/8/10.
- Device-info product code (payload 14..15) and workMode (16) differ between the capture and the
  verified unit (§2); meaning unknown.
- Addr echo of GET_MACRO at addr ≥ 400 and of all writes.
- gameMode (byte 1), fnSwitch (2), systemMode (6), pushButtonMode (16) semantics. The hub never edits
  them on this board.
- Behaviour of the macro table write when there are zero macros (no last-packet flag sent).
- GET_DEFAULT_KEY_MATRIX (0x1F) is defined but never called by the hub on this board. Response size is
  presumably 512.
- Whether the firmware sends the modifier bits of a KEYBOARD record (`02 mm uu 00`, §3.3).
- RESOLVED: 0x24 writes go to flash, because the per-key colours survived a replug. 0x23 is
  presumably stored too, since the board came back in custom mode. 0x26, 0x23 and 0x24 writes ACK and read back as expected (§10); per-packet latency and notifications were not measured.
- Breathing (7) period per speed step: not measured. The live colour read (0x33) is not used on
  magnetic boards. `preset apply --flash` uses speed 5 and 2000 ms; both are tunable.
- Effect 11 ("wave" / «По волнам» in the hub, which writes 11 for it) looked like a blue *breathing*
  effect to the user. Either the single-colour wave on this board looks like breathing, or the firmware's
  numbering differs from the hub's from 11 on. 15 = «Расходящаяся рябь» is consistent, so a shift is
  unlikely. To settle it, write 7 and 11 one after the other and compare them by eye. Until then the
  names follow the hub.
- Modes 21 and 22 (custom in the hub's `It`) were never observed on this board.
- Linux/xkb: KEY_F13–F15 are delivered as keycodes 191–193, which the default xkb maps to
  `XF86Tools`, `XF86Launch5`, `XF86Launch6`. Desktop shortcut editors show those names; binding still works.

## 9. Hardware switches on the case (observed on FW 1.17, 2026-11-09)

### 9.1 Actuation preset switch (3 positions)
- Labelled on the case: right = 0.5 mm, left = 1.2 mm (viewed from the back); center = 2.00 mm (observed).
- **Destructive:** moving the switch makes the firmware rewrite the per-key Hall table (GET_MAGNETIC_AXIS_RT
  block) for **all 128 records**, the 44 non-physical indices included: actuation := preset, RT
  press/release := 0 (RT off). Flags bit0 ("whole travel") is preserved. Keymap, settings, LED, DKS are
  untouched (verified by block diff). Evidence: `backups/switch-test/3-right.json` holds 121 records
  `00 00 32 00 00 00 00 00` and 7 records `00 01 32 00 …` (0.50 mm); `4-left.json` the same with
  `78 00` (1.20 mm). An earlier note said "84 physical keys"; the block data shows all 128.
- Consequence for writes: `type84 preset apply` keeps the non-physical records as read (they then hold
  the switch preset), while `type84 restore <backup> --only hall` writes the backup's block verbatim
  (non-physical records back to what the backup holds, 0xC8 = 2.00 mm on the user's unit). With the
  switch in the centre (2.00 mm) both produce the same block.
- No notification frame is emitted on the vendor channel, and no host write is involved; the change is only
  visible by re-reading the table. Host software cannot read the switch position.
- Implication for the GUI: warn that touching the switch wipes custom actuation/RT; offer "restore Hall table
  from backup"; re-read the table when the window regains focus.

### 9.2 Snap Tap switch (2 positions)
- Flipping it changes nothing readable (no block diff, no device-info change, no notification frame).
- With no SOCD records in the keymap, raw keyboard reports showed A and D held simultaneously in both positions.
  Hypothesis: it gates SOCD records stored in the profile. Which position is "enabled" is UNVERIFIED.

## 10. Hardware verification log
- 2026-11-10, FW 1.17: lighting writes (0x23 ×5, 0x24 ×1) through the guarded path, see §3.5/§3.6:
  - 11/3/2/reverse #00FF00: mode, brightness, speed and direction read back as written; the colour read
    `FF FF FF` / 1.
  - 1 #FF0000: same, colour unreported.
  - custom 128 read back as 20, then 0x24 (light-wasd, every other key black) read back identical,
    ledId = index.
  - Writing mode 20 itself (a brightness change in custom mode, or a restore of a backup taken in custom
    mode) is accepted: it reads back as 20, and the per-key block is unchanged.
  - Final state: `backups/type84-20261110-001012-fw1.17.json`. Final raw GET_LED_EFFECT: `14 FF FF FF 00 00 00 00 01 04 04 00 00 00 00 00`.
  - 0x23 and 0x24 write paths: VERIFIED. Readback compares only the reported fields.
- 2026-11-10, FW 1.17: `preset apply ... --flash`:
  - The breathing flash 7 (#3D7EFF / #22D07C) was followed by the final custom effect and the 0x24
    block. All of it read back as expected.
  - SIGTERM during the wait cut it short and the final lighting was still written (exit 0).
  - The 0x24 readback after light-work: 83 physical keys #FFFFFF, Esc #22D07C, ledId = index.
- 2026-11-09, FW 1.17: first real SET_MAGNETIC_AXIS_RT (0x27) write — 19 frames, full 1024-byte table
  (preset `cs2`). ACKs accepted by the write engine; read-back matched byte for byte; `verify-backup` against
  the user's 22:03 backup reports all restorable blocks identical. 0x27 write path: VERIFIED.

## 11. Onboard profiles: not supported

- Device info payload 19 is `currentProfile` (`owned-BuJoYyKq.js@30338` `currentProfile:i(19)`); the user's
  unit reports 0.
- The HFD opcode table (§1.7) has no profile-select or profile-read opcode, and the HFD driver never reads
  or writes a profile. The only "profiles" in the vendor code are in the old app (web.io.vision,
  `useProfiles-*.js`, built on the `layout-classic` driver): host-side configurations kept in the browser
  and written to the board as a whole, i.e. ordinary writes, not an onboard switch.
- Consequence: no `type84 profile` command. Host-side presets (`type84 preset apply`, combinable per
  section) are the replacement; `type84 preset current` reports which one the board holds.
