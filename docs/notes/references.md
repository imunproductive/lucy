# References

External material worth consulting. Anything from another build is a lead only; check it against 1.74.0.0.104319 before relying on it.

## Architecture

- Timothy Ford, GDC 2017, [Overwatch Gameplay Architecture and Netcode](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and).
- Dan Reed, GDC 2017, [Networking Scripted Weapons and Abilities](https://www.gdcvault.com/play/1024653/Networking-Scripted-Weapons-and-Abilities).

Both explain the design. Neither is a wire specification or server source.

## Earlier 1.74 servers and research

- [Boi-027, Overwatch 1.74 Lobby Research](https://github.com/Boi-027/Overwatch-1-v1.74-Lobby-Research): the original lobby research and implementation this code descends from.
- [LizRollin, overwatch-1.74-lobby-server at `9f8f3bcb`](https://github.com/LizRollin/overwatch-1.74-lobby-server/tree/9f8f3bcb5eccab6fb7738bee3c2371fcf0623f29): the game runtime the Python server imported (expression interpreter, statescript codecs and driver, collision, character mover, combat). Its expression VM had an unbounded jump loop (`expr.run([1, 0])`); the imported copy bounds execution.
- AipNooBest's notes: [early weapon and snapshot proof of concept at `f8426859`](https://github.com/AipNooBest/overwatch-1.74-lobby-server/blob/f8426859c3195691d379f2fb291a475dfa40139e/docs/STATESCRIPT.md) and [HUD corrections at `860bab45`](https://github.com/AipNooBest/overwatch-1.74-lobby-server/blob/860bab453eaa8b49e5760b155f9a3fa68e164af3/docs/HUD-VARIABLES.md). The later notes retract the queue-mode-5 HUD explanation and the screenshot readings of ammo 30/30, health and armor numbers and ability rows (those digits were duplicate weapon rendering). Their docs and code disagree on the body-create descriptor list; check call sites.
- Logo2K: [1.74 lobby research](https://rentry.co/3bswrgmw#overwatch-174-lobby-protocol-channel-setup-login-and-loot-boxes) and the [lobby map list](https://rentry.co/wcwyv3r8).
- Gamayun (TripleFault): a 104319 server in Go. Useful for comparing VM natives (possessed body and current possessor, health pool selectors, on-fire lookup), states (synced game-code state, tips, input bridge, delayed switch timing, event order) and component 51. **It is AGPL-3.0-or-later**: use it only as a behavior reference, never copy its code.

## Other builds

- [Prometheus at `ad1fea7`](https://github.com/saturn-xvi/prometheus/tree/ad1fea7e6c04c9959c7115cb8ecd9c79c93c1ccb): beta client research. Its voice system triggerer shows a client audio endpoint; its selective map-resource hook shows where the beta client filters loaded entities (placement of several dynamic objects was wrong after the bypass). Its component IDs and memory writes don't transfer to 1.74.
- Claudewatch: Beta 1.0.0.3 leads for attacker-side hit-confirm effects and separate entity and announcer voice messages.
- Client build 155284 was inspected for the instance exit, parent completion, typed zero returns and the movement sampler inputs. Treat those as leads for 104319.

## Tools

- [overtools/OWLib](https://github.com/overtools/OWLib) (MIT): DataTool, TankLib, STU type definitions.
- [overtools/TACTLib](https://github.com/overtools/TACTLib) (MIT): CASC and TACT reading, including the per-build Overwatch manifest procedures.
- [Plasmawatch login-server](https://github.com/plasmawatch/login-server): Battle.net login emulation reference.
- [SplitMix64](https://prng.di.unimi.it/splitmix.c) (public domain): the generator behind the statescript VM's local random expressions.
