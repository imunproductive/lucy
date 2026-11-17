# Protocol notes

Facts about the client and its network protocol for build **1.74.0.0.104319**. They were saved from the inherited documentation. Constants that already live in code are listed only when the code does not explain them.

## Ports

| Port | Use |
| --- | --- |
| TCP 3724 | Lobby (JAM protocol) |
| TCP 1119 | Battle.net "stall": the port the client dials. It is held open; the relay sends the real traffic elsewhere. |
| TCP 21119 | Battle.net RPC: plaintext WebSocket upgrade, then BGS frames, piped here by the relay |
| UDP 3730 | Game link |
| TCP 3725 | Dashboard |

The Python start scripts passed `--bnet-port 1118`.

## Lobby handshake

- Hello strings: `HELLO PRO CLIENT\0` and `HELLO PRO SERVER\0`.
- Both sides derive the stream keys from the two nonces with HMAC-SHA256 under an all-zero 32-byte key. The same HMAC values are the proofs each side sends, so a matching proof confirms the keys. The client does not check a session key.
- Client messages are 40 bytes: 8 ignored bytes, then a 32-byte nonce or proof.
- The 292-byte state blob is signed with a fixed 64-byte key for this build: HMAC-SHA256 over blob bytes `[0:176] + [208:256]`, written to `[176:208]`.
- The lobby schemas describe 707 messages in 87 protocol groups. Fields are identified by their offset in the client's C++ message object (`"+0x78"`).

## Game link

- Maximum datagram 1,264 bytes (`0x4F0`): a 34-byte encrypted link header and up to 1,230 bytes of world payload. AES-GCM with a 12-byte tag.
- The packet sequence is the AES-GCM nonce. Once encryption or a send was attempted, that sequence number is consumed and never reused, even when the send fails.
- Per frame: at most 15 messages of each kind (the counts are 4 bits) and at most 127 entity records.
- When a frame is over budget, drop entity candidates first (down to zero), then unreliable messages, then reliable ones, always keeping the oldest reliable message. Size each trial with the real serializer, never by adding raw lengths.
- Commit queues, corrections and remote-body visibility only after a successful send. On a send error keep the queues. A remote body becomes visible only after its create was sent, so its first movement can't go out before the create.
- Op, chunk and ch2 records go through ACK and resend. Pure movement and no-op records are consumed without reliable retransmission. Ch2-only projectile shot events must be consumed too.
- One record bigger than the payload limit would need fragmentation. The fragment encoding is unverified, so such a record is rejected explicitly. It is never sent oversized or dropped silently.
- Server tick 16 ms, a datagram to each client every tick, first tick 1000.

## Input commands

- Each player has a 32-bit command frame sequence per connection epoch. Unwrap by taking the signed difference closest to the highest frame seen (half-cycle horizon): `fffffffe, ffffffff, 0, 1` are consecutive. A separation of exactly 2³¹ is ambiguous and rejected.
- The same frame with an identical payload is a duplicate. The same frame with a different payload is an error.
- A client restart can't be reliably detected from frame numbers: a late old packet can look the same. A low frame (≤ 128) after a backward gap of ≥ 1,024 is at most a hint.
- Basic fields: forward and right throttle (signed 8 bits), buttons (unsigned 24 bits), action (unsigned 8 bits), yaw and pitch as integer angles. Yaw 0 points along +z, positive yaw turns toward +x, y is up, positive pitch is down.
- Extension fields: `extra` (default 0), `subframe` (default 255), `click_yaw` and `click_pitch`. They carry view delay and click-time aim.
- The real duration of one command frame is unknown. 16 ms was only a model choice.

## Raw statescript payloads

Two state classes have verified layouts but no runtime meaning yet:

- **`STU_9935FBC8` (linked target).** Always carries `link`. Only in owner, full frames does it add `has_entity`, the entity when set, and seven items that are either `{set: 0}` or `{set: 1, value: f32}`. The seven floats have no known purpose.
- **`STU_861D9015`.** Either `a = 0`, or `a = 1` followed by `v0..v6` (unsigned 16/16/32/32/32/16/16 bits, kept as raw bit patterns even when they look like floats), `has_entity`, the optional entity, `has_extra`, an optional unsigned 32-bit `extra`, and `g`. `g` is present only when `a = 1`, in every owner/full context.

A link is `(instance, raw index)`. Owner frames (H = 1) use the owner state indexes, remote frames (H = 0) the remote-sync indexes, each with the graph's own bit width. An entity id is 0 (none) or a 32-bit id with bit 31 set and a nonzero 29-bit index.

Each statescript host keeps a command frame on the client. An owner frame (H = 1) sets it to its CmFD; a frame without a CmFD (H = 0) sets it to the frame (server tick) of the datagram that brought the chunk. An owner frame whose CmFD is not past the host's command frame is stale: the client reads past its payload and counts the chunk as applied (`0x7FF78991E050` returns 2, `0x7FF78991E3C0`). So every owner frame of a host needs a CmFD above the last one and above the datagram frame of any H = 0 chunk before it (`Stream::full_owner`, `BodyStream::owner_kind`).

## Relay hook points

From the relay (`crates/relay`, ported from the community C++ relay; addresses are offsets from the `Overwatch.exe` image base):

| Item | Value |
| --- | --- |
| wfd object | `0x298D6D8`; its byte-stream is at wfd `+0x80` |
| Byte-stream vtable | `0x298E2F8`. Slot 1 is the connection state. |
| Byte-stream fields | `+0x20` state, `+0x48` the `ssl_stream`, `+0x80`/`+0x88` host pointer and length, `+0xA8` target port |
| `ssl_stream` vtable | `0x298BED8`. Slot 2 (`+0x10`) is `recv(this, dst, max, *out)`, slot 3 (`+0x18`) is `send(this, buf, len, *out)`. Both return 0 on success. |
| Send closure | `0x21758A0`. It only sends when the byte-stream state is 1. |

Forcing the state to 1 makes the client believe it is connected. It then writes plaintext: a WebSocket upgrade, then BGS frames. The relay swaps `send`/`recv` for a pipe to `127.0.0.1:21119`, so TLS never runs.

## Statescript graph evidence

- A graph's reference closure includes dormant scripts and conditional branches. It is not a list of executed behavior.
- Following plug links without evaluating conditions, triggers, time or lifecycle gives candidates, not reachability.
- A class name, a readable parameter or a numeric constant alone does not prove what a node does. An unsupported node or expression that blocks a scenario is a blocker, never "supported" by presence.
