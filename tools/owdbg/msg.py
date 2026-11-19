"""Network messages for `watch --msg` / `probe --msg`: the client handler and field layout of a
message id (from tools.owindex), the message object at handler entry, and marker sends through
ORPHEUS's debug endpoint.

Message object at handler entry: every client dispatcher (lobby/JAM and game link) calls or
tail-jumps to the handler as `handler(rcx = handler object, rdx = connection/context, r8 = message)`
(e.g. 20400 dispatcher 0x7A8310 case 0x7A8355: `mov r8, rdi; mov rdx, rbx; mov rcx, rsi;
jmp 0x849640`; game link 0x7D2000 case 0x7D20F7: `mov r8, rsi; ... call 0x7D88B0`). The tools do
not rely on it: at each handler hit the register among rcx/rdx/r8/r9 whose [reg] is the message
class vtable (`class_vtable_rva` in the index) is the message; other hits (a handler shared by
several ids) are skipped. Field address = message + field offset (index `fields[].off`, nested
struct members relative to their struct).

Field specs (`--field`, `--send` keys): dotted paths of `#index` (schema position, the same as
ORPHEUS's `index`) or `+0xOFF` (offset at that level), e.g. `#3`, `+0x128`, `#1.#0`,
`+0x80.+0x0`; an optional `:SIZE` overrides the size (needed for an offset that is no field).

Marker sends (`--send`): ORPHEUS `serve` runs with ORPHEUS_DEBUG_SEND=1; after the login
(lobby messages) or once the account is in a match (game-link messages: the endpoint answers an
error until then; `--send-after-match` waits for the server's `[MM] Match found` line), the
message is POSTed to http://127.0.0.1:3725/api/debug/send. The handler breakpoint arms only after
the first POST and, when the watched field has a marker value, only for a message carrying it.
"""
import json
import re
import struct
import threading
import queue
import urllib.error
import urllib.request

from tools import paths

from .names import hx

DASHBOARD = "http://127.0.0.1:3725"
ARG_REGS = ("rcx", "rdx", "r8", "r9")
PRIMITIVE = {"bool": "?", "u8": "B", "i8": "b", "u16": "H", "i16": "h", "u32": "I", "i32": "i",
             "u64": "Q", "i64": "q", "f32": "f", "f64": "d"}


def add_msg_args(p):
    g = p.add_argument_group("messages")
    g.add_argument("--msg", type=int, help="message id: break at its client handler (tools.owindex)")
    g.add_argument("--msg-link", choices=("game", "jam"), help="link of --msg when the id exists on both")
    g.add_argument("--send", help="JSON fields to send with --msg through ORPHEUS's debug endpoint, e.g. "
                                  "'{\"#3\": \"0x13572468\"}' (or a full /api/debug/send body)")
    g.add_argument("--send-every", type=float, default=0, help="send again every S seconds")
    g.add_argument("--send-count", type=int, default=0, help="stop after N sends (default 1, or unlimited "
                                                              "with --send-every)")
    g.add_argument("--send-delay", type=float, default=15.0,
                   help="seconds after the login (or the match) before the first send (default 15)")
    g.add_argument("--send-after-match", action="store_true",
                   help="wait for the server's '[MM] Match found' line before sending (game-link messages)")


# ---------------------------------------------------------------- index layout
class Message:
    """One message of tools.owindex Index.messages()."""

    def __init__(self, idx, mid, link=None):
        if idx is None:
            raise SystemExit(f"[msg] --msg needs the index {paths.INDEX} (python -m tools.owindex.build)")
        rows = [m for m in idx.messages() if m["id"] == mid and (link is None or m["link"] == link)]
        if not rows:
            raise SystemExit(f"[msg] message {mid} is not in the index" + (f" (link {link})" if link else ""))
        with_handler = [m for m in rows if m.get("handler_rva")]
        row = (with_handler or rows)[0]
        self.row = row
        self.id, self.link = mid, row["link"]
        self.handler_rva = row.get("handler_rva")
        self.vtable_rva = row.get("class_vtable_rva")
        self.fields = row.get("fields") or []
        self.notes = row.get("notes")
        if not self.handler_rva:
            raise SystemExit(f"[msg] message {mid}: no client handler in the index ({self.notes})")

    @property
    def endpoint_link(self):
        return "game" if self.link == "game" else "lobby"

    def describe(self):
        return (f"message {self.id} ({self.endpoint_link} link) handler rva 0x{self.handler_rva:X}, "
                f"dispatcher rva 0x{self.row.get('dispatcher_rva') or 0:X} ({self.row.get('handler_how')}), "
                f"vtable rva 0x{self.vtable_rva or 0:X}, {len(self.fields)} fields")

    def field(self, spec):
        """Field spec -> {"path", "off", "size", "type", "array", "fields"} (off from the message)."""
        return resolve_field(self.fields, spec)

    def layout(self):
        """Top-level fields: [(path, off, size, type, array)]."""
        out = []
        for i, f in enumerate(self.fields):
            out.append((f"#{i}", f["off"], field_span(self.fields, i), f.get("type_name"), f.get("array")))
        return out


def field_span(fields, i):
    """Bytes of field i in its parent: up to the next field for arrays/strings/blobs."""
    f = fields[i]
    if f.get("type_name") in PRIMITIVE and not f.get("array"):
        return f["size"]
    if f.get("type_name") == "struct" and not f.get("array"):
        return f["size"]
    nxt = fields[i + 1]["off"] if i + 1 < len(fields) else None
    if nxt is not None and nxt > f["off"]:
        return nxt - f["off"]
    return f["size"] if f.get("type_name") in ("string", "blob") else 16


def resolve_field(fields, spec):
    text, _, size = spec.partition(":")
    if size and not size.strip().isdigit():
        raise SystemExit(f"[msg] bad size in field spec {spec!r}")
    off, level, f, path = 0, fields, None, []
    for seg in text.strip().split("."):
        seg = seg.strip()
        if level is None:
            raise SystemExit(f"[msg] {spec!r}: {'.'.join(path)} has no members")
        if seg.startswith("#"):
            i = int(seg[1:], 0)
            if not 0 <= i < len(level):
                raise SystemExit(f"[msg] {spec!r}: no field {seg} ({len(level)} fields)")
        elif seg.startswith("+"):
            o = int(seg[1:], 0)
            i = next((k for k, x in enumerate(level) if x["off"] == o), None)
            if i is None:
                if not size:
                    raise SystemExit(f"[msg] {spec!r}: no field at +0x{o:X}; give :SIZE for a raw offset")
                return {"path": spec, "off": off + o, "size": int(size), "type": None, "array": False,
                        "fields": None}
        else:
            raise SystemExit(f"[msg] bad field segment {seg!r} in {spec!r} (use #index or +0xOFF)")
        f = level[i]
        path.append(f"#{i}")
        off += f["off"]
        span = field_span(level, i)
        level = f.get("fields") if f.get("type_name") == "struct" and not f.get("array") else None
    return {"path": ".".join(path), "off": off, "size": int(size) if size else span,
            "type": f.get("type_name"), "array": bool(f.get("array")), "fields": level}


def encode(value, ftype, size):
    """Marker bytes for a sent value (None: not encodable here, e.g. "self")."""
    if isinstance(value, bool):
        return bytes([int(value)]) + b"\0" * (size - 1) if ftype in PRIMITIVE else None
    if isinstance(value, str):
        if not re.fullmatch(r"-?(0x[0-9A-Fa-f]+|\d+)", value.strip()):
            return None
        value = int(value, 0)
    if ftype in ("f32", "f64"):
        return struct.pack("<" + PRIMITIVE[ftype], float(value))
    if isinstance(value, float):
        return None
    if isinstance(value, int) and ftype in PRIMITIVE and ftype != "bool":
        return (value & ((1 << (8 * size)) - 1)).to_bytes(size, "little")
    if isinstance(value, int) and ftype == "struct" and size in (1, 2, 4, 8):
        return (value & ((1 << (8 * size)) - 1)).to_bytes(size, "little")
    return None


def marker_fields(msg, fields):
    """{path: {"off", "size", "bytes"}} for the encodable values of a --send fields object."""
    out = {}

    def walk(prefix, obj):
        for key, value in obj.items():
            spec = f"{prefix}.{key}" if prefix else key
            if isinstance(value, dict):
                walk(spec, value)
                continue
            if isinstance(value, list):
                continue
            f = msg.field(spec)
            if f["fields"]:  # a struct given as a scalar: its single member (entity ids)
                f = msg.field(spec + ".#0")
            data = encode(value, f["type"], f["size"])
            if data is not None:
                out[f["path"]] = {"off": f["off"], "size": f["size"], "bytes": data, "value": value}
    walk("", fields)
    return out


# ---------------------------------------------------------------- runtime
def message_reg(dbg, hit, base, msg):
    """The argument register holding the message object at handler entry, or None."""
    want = base + msg.vtable_rva
    for r in ARG_REGS:
        v = hit.reg(r)
        if v and dbg.try_read(v, 8) == want.to_bytes(8, "little"):
            return r
    return None


def decode(dbg, va, fields, depth=0):
    """{path: value} of a message object's fields (primitives as numbers/hex, others raw hex)."""
    out = {}
    for i, f in enumerate(fields):
        key = f"#{i}+0x{f['off']:X}"
        t = f.get("type_name")
        span = field_span(fields, i)
        if t == "struct" and not f.get("array") and depth < 3:
            out[key] = decode(dbg, va + f["off"], f.get("fields") or [], depth + 1)
            continue
        data = dbg.try_read(va + f["off"], min(span, 48))
        if data is None:
            out[key] = None
        elif t in PRIMITIVE and not f.get("array"):
            v = struct.unpack("<" + PRIMITIVE[t], data[:f["size"]])[0]
            out[key] = v if isinstance(v, float) or t == "bool" else hx(v & ((1 << (8 * f["size"])) - 1))
        else:
            out[key] = {"type": (t or "?") + ("[]" if f.get("array") else ""), "raw": data.hex()}
    return out


_LOG_OFFSETS = {}  # (state dir, log name) -> bytes read; ("found", state dir) -> the line


def match_found(s):
    """The server's first `[MM] Match found` line in <state>/serve.log (its whole output; logs/
    rotates every 2 MB), or None (read incrementally: the log grows by megabytes in a match)."""
    server = s.server
    if server is None or server.state is None:
        return None
    hit = _LOG_OFFSETS.get(("found", server.state))
    if hit:
        return hit
    for path in (server.state / "serve.log",):
        key = (server.state, path.name)
        try:
            with open(path, "rb") as f:
                f.seek(_LOG_OFFSETS.get(key, 0))
                data = f.read()
        except OSError:
            continue
        cut = data.rfind(b"\n") + 1  # keep a partial last line for the next read
        _LOG_OFFSETS[key] = _LOG_OFFSETS.get(key, 0) + cut
        for line in data[:cut].decode("utf-8", "replace").splitlines():
            if "[MM] Match found" in line:
                _LOG_OFFSETS[("found", server.state)] = line.strip()
                return line.strip()
    return None


class Sender:
    """Sends --send through ORPHEUS's debug endpoint on a worker thread; poll() runs in the debug
    loop (timer) and decides when to send."""

    def __init__(self, s, args, msg, on_first_send=None):
        self.s, self.args, self.msg = s, args, msg
        body = json.loads(args.send)
        if "fields" not in body:
            body = {"fields": body}
        body.setdefault("id", msg.id)
        body.setdefault("account", args.account)
        self.body = body
        self.markers = marker_fields(msg, body["fields"])
        self.limit = args.send_count or (0 if args.send_every else 1)
        self.sent = 0
        self.errors = 0
        self.results = queue.Queue()
        self.busy = False
        self.ready_at = None  # elapsed time of the first send
        self.next_at = None
        self.on_first_send = on_first_send
        self.hinted = False
        self.match_seen = None
        self.history = []

    def log(self, m):
        self.s.log(m)

    def start(self):
        self.s.dbg.every(1.0, self.poll, first=1.0)
        what = ", ".join(f"{p}=+0x{m['off']:X}:{m['size']} {m['bytes'].hex()}" for p, m in self.markers.items())
        self.log(f"[send] will POST {json.dumps(self.body)} to {DASHBOARD}/api/debug/send"
                 f" ({self.msg.endpoint_link} link); markers: {what or 'none encodable'}")
        threading.Thread(target=self._get_schema, daemon=True).start()

    def _get_schema(self):
        try:
            with urllib.request.urlopen(f"{DASHBOARD}/api/debug/messages?id={self.msg.id}", timeout=10) as r:
                self.results.put(("schema", json.loads(r.read())))
        except urllib.error.HTTPError as e:
            self.results.put(("schema", {"error": f"HTTP {e.code}: {e.read()[:200]!r}"}))
        except Exception as e:
            self.results.put(("schema", {"error": str(e)}))

    def _post(self):
        req = urllib.request.Request(f"{DASHBOARD}/api/debug/send", data=json.dumps(self.body).encode(),
                                     headers={"Content-Type": "application/json"}, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                res = json.loads(r.read())
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                res = json.loads(raw)
            except ValueError:
                res = {"error": f"HTTP {e.code}: {raw[:200]!r}"}
        except Exception as e:
            res = {"error": str(e)}
        self.results.put(("send", res))

    def poll(self):
        dbg = self.s.dbg
        while not self.results.empty():
            kind, res = self.results.get()
            if kind == "schema":
                if "error" in res:
                    self.log(f"[send] GET /api/debug/messages failed: {res['error']} (ORPHEUS without "
                             f"the debug endpoint? cargo build --release, then rerun; {paths.ORPHEUS_EXE})")
                else:
                    self.log(f"[send] endpoint schema: {json.dumps(res)[:600]}")
                continue
            self.busy = False
            if "error" in res:
                self.errors += 1
                err = str(res["error"])
                if not self.hinted or self.errors % 10 == 0:
                    self.log(f"[send] t={dbg.elapsed:.0f}s endpoint error: {err}")
                if self.msg.endpoint_link == "game" and not self.hinted:
                    self.hinted = True
                    self.log("[send] game-link message: the account must be in a match. Enter one in the game "
                             "window (main menu TRAINING > PRACTICE RANGE > SELECT, or --enter-range); "
                             "retrying every 5 s")
                self.next_at = dbg.elapsed + 5
            else:
                self.sent += 1
                self.history.append({"t": round(dbg.elapsed, 1), "response": res})
                self.log(f"[send] t={dbg.elapsed:.1f}s sent #{self.sent}: {json.dumps(res)[:400]}")
                self._learn_values(res)
                self.next_at = dbg.elapsed + self.args.send_every if self.args.send_every else None
        if self.busy or (self.limit and self.sent >= self.limit):
            return
        if not self.s.logged_in:
            return
        if self.ready_at is None:
            if self.args.send_after_match:
                if self.match_seen is None:
                    line = match_found(self.s)
                    if line is None:
                        if not self.hinted:
                            self.hinted = True
                            self.log("[send] waiting for a match (server '[MM] Match found'): enter one in the "
                                     "game window (main menu TRAINING > PRACTICE RANGE > SELECT)")
                        return
                    self.match_seen = dbg.elapsed
                    self.log(f"[send] t={dbg.elapsed:.0f}s server: {line}")
                self.ready_at = self.match_seen + self.args.send_delay
            else:
                self.ready_at = dbg.elapsed + self.args.send_delay
                self.log(f"[send] logged in; first send at t={self.ready_at:.0f}s")
        if dbg.elapsed < self.ready_at or (self.next_at is not None and dbg.elapsed < self.next_at):
            return
        if self.sent and not self.args.send_every:
            return
        if self.on_first_send is not None:
            self.on_first_send()
            self.on_first_send = None
        self.busy = True
        threading.Thread(target=self._post, daemon=True).start()

    def _learn_values(self, res):
        """Marker bytes for values only the server knows ("self"): from the response's top-level values."""
        for f in res.get("fields") or []:
            i, value = f.get("index"), f.get("value")
            if i is None or value is None:
                continue
            spec = f"#{i}"
            try:
                fd = self.msg.field(spec)
                if fd["fields"]:
                    fd = self.msg.field(spec + ".#0")
            except SystemExit:
                continue
            if fd["path"] in self.markers or isinstance(value, (dict, list)):
                continue
            data = encode(value, fd["type"], fd["size"])
            orig = self.body["fields"].get(spec) if isinstance(self.body["fields"], dict) else None
            if data is not None and orig is not None and data.strip(b"\0"):
                self.markers[fd["path"]] = {"off": fd["off"], "size": fd["size"], "bytes": data, "value": value}

    def marker_for(self, off, size):
        """Marker bytes expected in [off, off+size) of the message, or None."""
        for m in self.markers.values():
            if m["off"] <= off and off + size <= m["off"] + m["size"]:
                return m["bytes"][off - m["off"]:off - m["off"] + size]
        return None


class RangeEntry:
    """--enter-range: main menu TRAINING > PRACTICE RANGE > SELECT (the preselected hero) by
    PostMessage clicks at relative client coordinates (measured at 1584x891). TRAINING is clicked
    25 s after the login (the main menu needs that long), PRACTICE RANGE 8 s later; the server's
    `[MM] Match found: Practice Range` line confirms; without it after 60 s the sequence is
    retried once (Escape first). In hero select the yellow SELECT button is clicked until it
    disappears; with `hero` ((x, y) client fractions of a portrait) that portrait is clicked first."""

    TRAINING = (0.0789, 0.2974)
    PRACTICE_RANGE = (0.5, 0.5)
    SELECT = (0.5, 0.935)
    SELECT_PIXEL = (0.47, 0.935)  # inside the button, left of its text

    def __init__(self, s, hero=None):
        self.s = s
        self.hero = hero
        self.hero_clicked = False
        self.state = "wait"
        self.at = None
        self.tries = 0
        self.select_clicks = 0

    def start(self):
        self.s.dbg.every(2.0, self.poll, first=2.0)

    def click(self, frac, what):
        from .game import capture, post_click
        hwnd = self.s.window()
        if not hwnd:
            return False
        width, height, _ = capture(hwnd)
        post_click(hwnd, int(width * frac[0]), int(height * frac[1]))
        self.s.log(f"[range] t={self.s.dbg.elapsed:.0f}s clicked {what}")
        return True

    def select_visible(self):
        from .game import capture
        hwnd = self.s.window()
        if not hwnd:
            return False
        try:
            width, height, rgb = capture(hwnd)
        except RuntimeError:
            return False
        r, g, b = rgb(int(width * self.SELECT_PIXEL[0]), int(height * self.SELECT_PIXEL[1]))
        return r > 180 and 0.65 * r < g < 0.85 * r and b < 0.5 * r  # (213, 159, 89)

    def poll(self):
        t = self.s.dbg.elapsed
        if self.state == "done" or not self.s.logged_in:
            return
        if self.state == "wait":
            self.at, self.state = t + 25, "training"
        elif self.state == "training" and t >= self.at:
            if self.click(self.TRAINING, "TRAINING"):
                self.at, self.state = t + 8, "range"
        elif self.state == "range" and t >= self.at:
            if self.click(self.PRACTICE_RANGE, "PRACTICE RANGE"):
                self.at, self.state = t + 60, "match"
                self.tries += 1
        elif self.state == "match":
            if match_found(self.s):
                self.s.log(f"[range] t={t:.0f}s server: match found (Practice Range); waiting for hero select")
                self.at, self.state = t + 120, "select"
            elif t >= self.at:
                self.snap_screen()
                if self.tries >= 2:
                    self.s.log("[range] no match after 2 tries: enter the Practice Range by hand")
                    self.state = "done"
                    return
                from .game import post_key
                hwnd = self.s.window()
                if hwnd:
                    post_key(hwnd, 0x1B)  # Escape back to the main menu
                self.s.log("[range] no match yet: retrying TRAINING > PRACTICE RANGE")
                self.at, self.state = t + 5, "training"
        elif self.state == "select":
            if self.select_visible():
                if self.hero and not self.hero_clicked:
                    self.click(self.hero, "the hero's portrait")
                    self.hero_clicked = True
                elif self.select_clicks < 6:
                    self.click(self.SELECT, "SELECT (hero select)")
                    self.select_clicks += 1
            elif self.select_clicks:
                self.s.log(f"[range] t={t:.0f}s SELECT gone: spawning in the Practice Range")
                self.state = "done"
            elif t >= self.at:
                self.s.log("[range] hero select not recognized; pick a hero by hand if needed")
                self.state = "done"

    def snap_screen(self):
        """The window as it was when no match came (what the clicks met), for the log."""
        from .game import out_path, screenshot
        hwnd = self.s.window()
        if not hwnd:
            return
        path = out_path("range-nomatch", ".png")
        try:
            screenshot(hwnd, path)
        except (RuntimeError, ImportError, OSError) as error:
            self.s.log(f"[range] no screenshot: {error}")
            return
        self.s.log(f"[range] screen without a match: {path}")
