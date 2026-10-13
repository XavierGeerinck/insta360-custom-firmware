# waved-mcu

The link to the Wave's MCU: `/dev/ttyS4` at 115200, MAVLink v2. The protocol, the stock
behaviour it copies and what is verified on the device are in
[`docs/MCU_PROTOCOL.md`](../../docs/MCU_PROTOCOL.md).

| Module | What it does |
|--------|--------------|
| `mavlink.rs`, `protocol.rs` | Framing; decoding the MCU's messages; the commands waved sends |
| `handler.rs` | Heartbeat replies, the battery (stock's glitch filter), the power key as `DeviceEvent::PowerKey` |
| `lift.rs` | Mic height: moves, start height, motor reports, resends |
| `power_off.rs` | Stock's power-off sequence |
| `leds.rs` | The status LEDs: stock's event table and scheduler, which waved states raise which event, LED_CTRL resends (`McuService::with_leds`; `WAVED_LEDS=0` leaves them alone) |
| `link.rs` | The serial port |
| `simulate.rs` | `SimMcu`, a frame-level MCU for the host tests and the `simulate` build |

The Wave has no volume buttons: the power key is its only key (`docs/MCU_PROTOCOL.md`,
"Physical controls"). `ButtonType::VolumeUp/Down` and `SimMcuHandle::press_button` are left
from an earlier guess and carry no device traffic.

Tests: `cargo test -p waved-mcu --lib` and `cargo test -p waved-mcu --lib --features simulate`.
