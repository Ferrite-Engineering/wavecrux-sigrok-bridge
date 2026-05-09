# Reference fixtures

Each subdirectory contains one VCD file plus its
`.expected_transactions.json` companion. The fixtures are committed
binary-deterministic — re-running the regenerator produces byte-
identical output.

## Layout

```
test/fixtures/
├── onewire/
│   ├── onewire_basic.vcd
│   └── onewire_basic.expected_transactions.json
├── jtag/
│   ├── jtag_idcode.vcd
│   └── jtag_idcode.expected_transactions.json
├── pwm/
│   ├── pwm_steps.vcd
│   └── pwm_steps.expected_transactions.json
├── dmx512/
│   ├── dmx_two_slots.vcd
│   └── dmx_two_slots.expected_transactions.json
└── modbus/
    ├── modbus_read_holding.vcd
    └── modbus_read_holding.expected_transactions.json
```

## Regenerating

```bash
cargo run -p wavecrux-sigrok-bridge-generate-fixtures -- --root .
```

The tool writes deterministically. Commit any diff that comes out of
the regenerator alongside the source change that caused it.

## What the fixtures encode

| Decoder | Channels | Scenario |
|---|---|---|
| `sigrok.onewire` | `data` | RESET, presence, READ ROM, ROM byte stream |
| `sigrok.jtag` | `tck`, `tms`, `tdi`, `tdo` | TAP traversal, IR/DR scan, IDCODE |
| `sigrok.pwm` | `signal` | 25% duty for 5 cycles → 75% duty for 5 cycles |
| `sigrok.dmx512` | `data` | BREAK, MAB, start code, two slots |
| `sigrok.modbus` | `rx` | Read Holding Registers, fn 0x03, CRC |

## Companion JSON shape

```json
{
  "decoder_id": "sigrok.<protocol>",
  "transactions": [
    {
      "start_fs": 0,
      "end_fs": 5000000000,
      "label": "...",
      "is_error": false
    }
  ]
}
```

The integration tests (`crates/bridge/tests/end_to_end.rs`) load each
companion, replay the matching VCD through the bridge subprocess, and
diff the emitted annotations against `transactions[*]`.

## Notes

* The 1-Wire fixture's timing is approximate — the mock emits scripted
  annotations on the first edge regardless of the real 1-Wire bit
  timing. When the `sigrok` feature is on, libsigrokdecode performs
  the real bit-level decode and the mock script becomes irrelevant.
* Real libsigrokdecode annotation labels may differ from the mock
  labels in wording. The verification guide flags this and treats the
  mock as the authoritative shape *for IPC contract testing*. Once the
  real backend lights up, the expected JSON will be regenerated from
  it.
