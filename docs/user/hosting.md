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

## How many players

The lobby serves at most 128 players at once, and at most 16 of them from one IP address. The banner shows both:

```
 [*] Players:        at most 128, 16 from one address
```

The first limit counts everyone online, in the menus and in every match: three 12-player matches and the players waiting in the menus all count. The second limit is for players who share one public address: a household, a dormitory, or a mobile or carrier-grade NAT. Players on your LAN each have their own address. A game on this PC counts for `127.0.0.1`.

Set them with `--max-players` and `--max-players-per-address`, or `max_players` and `max_players_per_address` in `config.toml` ([configuration](configuration.md#keys)). Each must be at least 1, and the second must not be above the first. A player over a limit can't connect; the server log says "The lobby serves N connections, its most" or "N lobby connections from ADDRESS, the most from one address". A player who joins with `lucy join` holds a second connection for a moment, while the launcher asks for the BattleTag.

## Join with the full menu: `lucy join`

```powershell
target\release\lucy.exe join --server 203.0.113.7:3724 --name Ana
```

Mode 5. `join`:

1. asks the server which BattleTag belongs to your name (this also checks that the server is up and new enough);
2. starts a local Battle.net on this PC that logs you in with that name and sends the game to the server's lobby;
3. starts the game with the relay.

Without `--server` or `--name`, `lucy` asks for them and remembers the answers (`server_address.txt`, `player_name.txt`). The name must follow the rule in [Player names](#player-names). Keep the window open while you play; closing it disconnects the game.

`join` needs the Battle.net ports 1119 and 21119 free on your PC. Do not run a server or another game on the same PC at the same time.

## Join in tournament mode: `lucy join-tournament`

```powershell
target\release\lucy.exe join-tournament --server 203.0.113.7:3724
```

Mode 6. It checks that the server can be reached, then starts the game in tournament mode, pointed at that lobby. There is no relay and no Battle.net. You can close the console window after the game starts.

## Player names

Your name is your account on the server: the host keeps your profile in the file `profiles\<name>.json`. So the server accepts only names that can be a file name as you typed them:

- 1 to 24 characters, and at most 58 bytes in UTF-8 (the Battle.net login has room for 58). A letter outside Latin can take 2 or 3 bytes: a Chinese or Korean name can have up to 19 characters.
- Only letters and digits of any alphabet (`Ana`, `Zoë`, `Жора`, `玩家`), `-` and `_`. No spaces, dots, slashes or other symbols.
- Not a name that Windows keeps for a device: `CON`, `PRN`, `AUX`, `NUL`, `COM0` to `COM9`, `LPT0` to `LPT9`, in any letter case.

Letter case does not matter: `Ana` and `ana` are the same account. Different names are different accounts: `Ana` with a Latin A and `Аna` with a Cyrillic А are two accounts.

`lucy join` checks the name before it starts the game. If the server gets a name that breaks the rule (in tournament mode, the game sends the name you typed), it disconnects the game. The game shows "Name contains invalid characters." when a character is not allowed, and "Account has an invalid battle tag" when the name is too long or is a device name. The host's log shows the name and the reason.

Profiles saved by an older version under a name that breaks the rule (for example `John Smith.json`) still load on the host: the dashboard lists them and `--account` chooses them. Players can't log in to them from another PC. To make one usable again, rename its file to a name that follows the rule.

## Join messages

| Message | Meaning |
| --- | --- |
| `Can't find the server ...` | The address has no IPv4 address. Check the spelling. |
| `Can't reach the server ... Check the address, and that the server runs.` | No answer on that address and port. Check the address, the host's port forwarding and firewall, and that the host's server runs. |
| `The server ... runs an older version. Ask the host to update it, or choose 6.` | The host's server cannot answer the BattleTag question. The host must update, or you use `join-tournament`. |
| `The Battle.net ports are taken (...). Is a server or another game running here?` | Port 1119 or 21119 is in use on your PC. Stop the other program. |
