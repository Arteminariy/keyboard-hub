# Contributing

Thanks for helping. Please read [docs/SAFETY.md](docs/SAFETY.md) first: this tool writes to device flash.

## Capturing protocol traffic

1. Use `type84 -v <command>` to print every frame sent and received.
2. Take a full backup (`type84 backup`) before and after any experiment and diff the blocks.
3. Record firmware version, what you changed, and the raw bytes in `docs/PROTOCOL.md`, marking items
   as VERIFIED (with firmware version) or UNVERIFIED.
4. Do not paste vendor source code into the repository. Cite short identifiers and byte offsets only.

## Rules for new opcodes

- Reads are free; writes are not. A new write opcode needs: an explicit entry in the allowlist, tests
  that sweep all 256 opcodes, a preview/diff, an automatic pre-write backup and a readback comparison.
- Never send calibration, reset or firmware-flash opcodes (see `docs/PROTOCOL.md` section 5).
- Verify on real hardware, on a spare or well-backed-up board, and log it in section 10 of the protocol doc.
- Do not widen the read-only transport.

## Before sending a PR

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Do not include personal data (paths, serial numbers, screenshots with private content) in fixtures or docs.
