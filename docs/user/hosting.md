# Hosting and joining

One PC runs the server (the host). Other PCs join it. Every player needs their own game install, build 1.74.0.0.104319, and their own Lucy build.

## Host and play: `lucy host`

```powershell
target\release\lucy.exe host
```

Mode 3. This is `play`, but the lobby and the game server listen on every address (`0.0.0.0`), so other PCs can connect. You play on the same PC.

## Host without playing: `lucy server`

```powershell
target\release\lucy.exe server
```

Mode 4. Only the lobby and the game server, open to other PCs. No game starts here and there is no local Battle.net. It runs until Ctrl+C. To play on the same PC too, use `host` instead.

## Listen address

`--host <address>` sets where the lobby and the game server listen. The default is `0.0.0.0` (every address) for `host` and `server`, and `127.0.0.1` (this PC only) for the other modes.

## Players on the internet: `--game-host`

Players on your network (or a VPN) can join with your LAN address. The banner lists it:

```
 [*] Bind:           0.0.0.0:3724
 [*] Your network:   192.168.1.20:3724
 [*] Internet:       203.0.113.7:3724 (open TCP 3724 and UDP 3730 in your router and firewall)
```

For players on the internet:

1. Find your public address (for example on whatismyip.com).
2. Give it with `--game-host 203.0.113.7`. When you choose mode 3 or 4 in the menu, `lucy` asks for it and remembers it in `public_address.txt`. Press Enter to reuse the last one, or type `-` for none.
3. Forward the ports in your router and allow them in the Windows firewall.

The lobby sends each player the address of the game server for a match. Players from outside your network get the `--game-host` address. Players on your network get the address they used to reach the lobby.

Without `--game-host`, the banner shows `Internet: off`.

## Ports to forward

| Port | Protocol | Service | Forward for hosting? |
| --- | --- | --- | --- |
| 3724 | TCP | Lobby | Yes |
| 3730 | UDP | Game server | Yes |
| 1119 | TCP | Local Battle.net (the game dials it) | No, this PC only |
| 21119 | TCP | Battle.net RPC (the relay pipes to it) | No, this PC only |
| 3725 | TCP | Dashboard | No. It listens on `127.0.0.1` only. |

You can change each port with a flag (`--lobby-port`, `--game-port`, ...) or in `[ports]` in `config.toml`. If you change the lobby port, joining players must use the new port in `--server`.

## Join with the full menu: `lucy join`

```powershell
target\release\lucy.exe join --server 203.0.113.7:3724 --name Ana
```

Mode 5. `join`:

1. asks the server which BattleTag belongs to your name (this also checks that the server is up and new enough);
2. starts a local Battle.net on this PC that logs you in with that name and sends the game to the server's lobby;
3. starts the game with the relay.

Without `--server` or `--name`, `lucy` asks for them and remembers the answers (`server_address.txt`, `player_name.txt`). A name has at most 24 characters. Keep the window open while you play; closing it disconnects the game.

`join` needs the Battle.net ports 1119 and 21119 free on your PC. Do not run a server or another game on the same PC at the same time.

## Join in tournament mode: `lucy join-tournament`

```powershell
target\release\lucy.exe join-tournament --server 203.0.113.7:3724
```

Mode 6. It checks that the server can be reached, then starts the game in tournament mode, pointed at that lobby. There is no relay and no Battle.net. You can close the console window after the game starts.

## Join messages

| Message | Meaning |
| --- | --- |
| `Can't find the server ...` | The address has no IPv4 address. Check the spelling. |
| `Can't reach the server ... Check the address, and that the server runs.` | No answer on that address and port. Check the address, the host's port forwarding and firewall, and that the host's server runs. |
| `The server ... runs an older version. Ask the host to update it, or choose 6.` | The host's server cannot answer the BattleTag question. The host must update, or you use `join-tournament`. |
| `The Battle.net ports are taken (...). Is a server or another game running here?` | Port 1119 or 21119 is in use on your PC. Stop the other program. |
