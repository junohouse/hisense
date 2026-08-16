# Hisense

Hisense (VIDAA) TVs, over the MQTT broker built into the set — the same one its own Remote NOW
app talks to, on port 36669.

One box, two things it does — same split as Roku: "watch Netflix" is the `media_player`, volume
and input are the `tv`. There is no player-only variant to ship separately; every Hisense set
speaking this protocol has a screen.

## Setup

No discovery beyond matching a known Hisense MAC prefix on an existing scan — the broker gives
away nothing before it is spoken to, so there is no probe that identifies one honestly. Setup
asks for the TV's address and MAC address; both are on the TV under Settings → Network → check
the network status, or the label on the back of the set.

The MAC is not cosmetic: the broker is not running while the TV is off or asleep, so `on` is a
Wake-on-LAN packet rather than a keypress, and Wake-on-LAN has nothing to aim at without it.

### The on-screen PIN

Some models pair a new remote with a 4-digit code shown on screen the first time it hears from
one. Nothing to press first — it appears on its own once this driver's first command reaches the
TV — and it is entered through the `pair` action on the device, not through setup. Until it is
entered, commands reach the TV and are ignored.

## Building

```bash
cargo build --release
```

Releases are built by [`junohouse/driver-ci`](https://github.com/junohouse/driver-ci): push to
`main` for a beta, tag `v1.2.0` for a release. To work on this against a local core checkout,
uncomment the `[patch]` block in `Cargo.toml`.
