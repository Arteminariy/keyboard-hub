# Changelog

## 0.1.0

First public release.

- `type84-proto`: protocol codec (HFD), decoding of device info, settings, keymap, Hall/RT table, lighting.
- `type84-hid`: hidapi transport; read-only by default, separate opt-in write session with an opcode allowlist.
- `type84` CLI: info, dump, backup, verify-backup, presets, restore, lighting animation helper.
- `type84-gui` ("Keyboard Hub"): keymap, response, lighting and device tabs, tray, confirm dialog with per-key diff.
- Writes verified on firmware 1.17: 0x27 (Hall), 0x26 (Fn layer), 0x23 (LED effect), 0x24 (per-key colours).
- Not implemented: global settings (0x21), base layer (0x22), macro and DKS writes.
- Windows has no cross-process write lock.
