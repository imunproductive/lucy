"""The connection to owprobe.dll: framed JSON over 127.0.0.1 (a u32 little-endian length, then UTF-8 JSON).

    from tools.owprobe import Probe
    p = Probe()                       # port: ORPHEUS_PROBE_PORT or 21150
    p.info()["base"]
    p.read_f32(p.addr("c4") + 0x160, 3)
    hook = p.hook(0x7FF78991F5D0, name="rollback", stack=4)   # ORPHEUS VAs are converted
    for record in p.records(kinds=["hit"], seconds=5): ...
"""
import json
import os
import socket
import struct
import time

from tools.owdbg.names import ORPHEUS_BASE

DEFAULT_PORT = 21150

# Command bits of the input sample (ClientInput.md; crates/probe/src/input.rs BUTTONS).
BUTTONS = {"fire": 0x1, "fire2": 0x2, "interact": 0x4, "ability1": 0x8, "ability2": 0x10, "ultimate": 0x20,
           "jump": 0x40, "crouch": 0x80, "reload": 0x400, "melee": 0x800}


class ProbeError(Exception):
    pass


def port_from_env():
    try:
        return int(os.environ.get("ORPHEUS_PROBE_PORT", DEFAULT_PORT)) or DEFAULT_PORT
    except ValueError:
        return DEFAULT_PORT


class Probe:
    """One connection. Methods raise ProbeError with the probe's message when a request fails."""

    def __init__(self, port=None, host="127.0.0.1", timeout=30.0):
        self.port = port or port_from_env()
        self.sock = socket.create_connection((host, self.port), timeout=timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self._id = 0
        self._info = None
        self._names = None

    def close(self):
        self.sock.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    @classmethod
    def wait(cls, port=None, timeout=120.0):
        """Connect once the probe listens (the game may still be starting)."""
        end = time.monotonic() + timeout
        while True:
            try:
                return cls(port)
            except OSError:
                if time.monotonic() > end:
                    raise
                time.sleep(0.5)

    # ---- transport
    def _recv(self, n):
        out = bytearray()
        while len(out) < n:
            chunk = self.sock.recv(n - len(out))
            if not chunk:
                raise ProbeError("the probe closed the connection (the game exited?)")
            out += chunk
        return bytes(out)

    def request(self, cmd, **args):
        self._id += 1
        body = json.dumps({"id": self._id, "cmd": cmd, **args}).encode()
        self.sock.sendall(struct.pack("<I", len(body)) + body)
        (length,) = struct.unpack("<I", self._recv(4))
        answer = json.loads(self._recv(length))
        if not answer.get("ok"):
            raise ProbeError(f"{cmd}: {answer.get('error')}")
        return answer.get("result")

    # ---- addresses
    def info(self, fresh=True):
        if fresh or self._info is None:
            self._info = self.request("info")
        return self._info

    @property
    def base(self):
        return self.info(fresh=False)["base"]

    @property
    def names(self):
        """tools.owdbg.names.Names over this process's modules (index names when the index exists)."""
        if self._names is None:
            from tools.owdbg.core import Module
            from tools.owdbg.names import Names

            mods = [Module(m["base"], m["size"], m["path"]) for m in self.request("modules")]
            info = self.info(fresh=False)
            main = next((m for m in mods if m.base == info["base"]), None)
            if main is not None:
                # The module list has the size from the scrambled headers; the probe has the file's.
                main.size = info["image_size"]

            class _Modules:
                def __init__(self):
                    self.main = main

                def module_at(self, va):
                    return next((m for m in mods if m.base <= va < m.base + m.size), None)

            self._names = Names(_Modules())
        return self._names

    def runtime(self, va):
        """An ORPHEUS VA (base 0x7FF788F10000) in the client image -> this run's address; else as is."""
        info = self.info(fresh=False)
        if ORPHEUS_BASE <= va < ORPHEUS_BASE + info["image_size"]:
            return va - ORPHEUS_BASE + info["base"]
        return va

    def orpheus(self, va):
        """This run's address in the client image -> its ORPHEUS VA; else as is."""
        info = self.info(fresh=False)
        if info["base"] <= va < info["base"] + info["image_size"]:
            return va - info["base"] + ORPHEUS_BASE
        return va

    def addr(self, spec):
        """An address: an int or hex text (ORPHEUS VAs converted), an index/docs name (Eval_STU_x,
        GetComp4, Msg25005_Handler), 'base', or 'c59'/'ent'/'c4' (the frame sampler's local body)."""
        if isinstance(spec, int):
            return self.runtime(spec)
        text = str(spec).strip()
        if text in ("c59", "ent", "c4", "base"):
            value = self.request("eval", exprs=[text])[0]
            if value is None:
                raise ProbeError(f"{text} is not known yet (the frame sampler needs a match)")
            return value
        try:
            return self.runtime(int(text, 16) if not text.lower().startswith("0x") else int(text, 0))
        except ValueError:
            pass
        rva = self.names.lookup(text)
        if rva is None:
            raise ProbeError(f"unknown address or name {text!r}")
        return self.base + rva

    def describe(self, va):
        """'Name+0xOFF (0x7FF7...)' for a code address."""
        d = self.names.describe(va)
        return d["text"] + (f" ({d['orpheus']})" if d.get("orpheus") else "")

    # ---- memory
    def read(self, addr, length, partial=False):
        return bytes.fromhex(self.request("read", addr=self.addr(addr), len=length, partial=partial)["hex"])

    def write(self, addr, data):
        return self.request("write", addr=self.addr(addr), hex=bytes(data).hex())

    def read_u64(self, addr, count=1):
        values = struct.unpack(f"<{count}Q", self.read(addr, 8 * count))
        return values[0] if count == 1 else list(values)

    def read_u32(self, addr, count=1):
        values = struct.unpack(f"<{count}I", self.read(addr, 4 * count))
        return values[0] if count == 1 else list(values)

    def read_f32(self, addr, count=1):
        values = struct.unpack(f"<{count}f", self.read(addr, 4 * count))
        return values[0] if count == 1 else list(values)

    def eval(self, *exprs):
        """Read expressions ('[c59+0x1038]:u32', '[[rcx]+8]:f32x3' with registers only in hooks)."""
        return self.request("eval", exprs=list(exprs))

    def regions(self, start=None, end=None, committed=True):
        args = {"committed": committed}
        if start is not None:
            args["start"] = self.addr(start)
        if end is not None:
            args["end"] = self.addr(end)
        return self.request("regions", **args)

    def modules(self):
        return self.request("modules")

    def scan(self, value=None, pattern=None, start=None, end=None, max=1000):
        """Heap words equal to value, or a byte pattern ('48 8B ?? 05') in [start, end) (default .text)."""
        args = {"max": max}
        if value is not None:
            args["value"] = value
        else:
            args["pattern"] = pattern
            if start is not None:
                args["start"] = self.addr(start)
            if end is not None:
                args["end"] = self.addr(end)
        return self.request("scan", **args)

    def components(self, vtable, max=65536):
        """[{component, entity, id}] of the components with this vtable (an ORPHEUS VA is converted)."""
        return self.request("components", vtable=self.addr(vtable), max=max)

    def component(self, entity_id, kind):
        """{entity, component}: the entity with this id and its component `kind`, found as the client's own
        lookup does (None when missing; needs the frame sampler's player entity)."""
        return self.request("component", id=entity_id, kind=kind)

    def disasm(self, addr, count=16):
        return self.request("disasm", addr=self.addr(addr), count=count)

    def alloc(self, size):
        return self.request("alloc", size=size)

    def note(self, text):
        """A text record in the ring (to mark a moment); returns its sequence number."""
        return self.request("note", text=text)

    # ---- hooks
    def hook(self, addr, name=None, reads=(), ret=False, ret_reads=(), stack=0, max_hits=0, entry=True,
             set=None):
        """Install a hook; returns its description (id, ...). reads: expressions at each hit; ret: also log
        the return (entry hooks); stack: unwound callers; set: {register: value} written at each hit."""
        address = self.addr(addr)
        return self.request("hook", addr=address, name=name or self.describe(address), reads=list(reads),
                            ret=ret, ret_reads=list(ret_reads), stack=stack, max_hits=max_hits, entry=entry,
                            set=set or {})

    def unhook(self, hook_id):
        return self.request("unhook", id=hook_id)

    def hooks(self):
        return self.request("hooks")

    def fetch(self, since=0, kinds=(), hooks=(), max=5000, wait_ms=0):
        return self.request("fetch", since=since, kinds=list(kinds), hooks=list(hooks), max=max, wait_ms=wait_ms)

    def last_seq(self):
        return self.info()["records"]

    def records(self, kinds=(), hooks=(), seconds=None, since=None, until=None):
        """Yield records from now (or `since`) for `seconds`, or until `until()` is true."""
        seq = self.last_seq() if since is None else since
        end = None if seconds is None else time.monotonic() + seconds
        while True:
            if end is not None and time.monotonic() >= end:
                return
            if until is not None and until():
                return
            got = self.fetch(seq, kinds, hooks, wait_ms=100)
            if got["lost"]:
                yield {"kind": "lost", "count": got["lost"]}
            yield from got["records"]
            seq = got["next"]

    # ---- the frame sampler and input
    def sampler(self, every=None, reads=None, body=None, camera=None, scripts=None):
        """Turn the frame sampler on (every N input samples; 0 stops recording) with extra reads; body: the
        entity id to record (default 0: the one the player possesses); camera: read the camera each recorded
        frame (default on); scripts: statescript graphs to record each frame ('0D06' on the body,
        'player:0C90' on the player, '0D06:2083,11432' only those variables)."""
        args = {}
        if every is not None:
            args["every"] = every
        if reads is not None:
            args["reads"] = list(reads)
        if body is not None:
            args["body"] = body
        if camera is not None:
            args["camera"] = bool(camera)
        if scripts is not None:
            args["scripts"] = list(scripts)
        return self.request("sampler", **args)

    def frame(self):
        """{frames, last}: the latest frame sample (position, view, buttons...)."""
        return self.request("frame")

    def input(self, steps, clear=False):
        """Queue input steps (tools.owprobe.script.parse makes them from text)."""
        return self.request("input", steps=list(steps), clear=clear)

    def input_state(self):
        return self.request("input_state")

    def wait_input(self, timeout=120.0):
        end = time.monotonic() + timeout
        while self.input_state()["pending"] and time.monotonic() < end:
            time.sleep(0.05)

    def call(self, addr, *args, thread="frame", force=False, timeout_ms=5000):
        """Call a client function (integers; floats as their bit patterns). thread: 'frame' (the main
        thread, at the next input sample) or 'probe'. Refused unless addr is a function start (force)."""
        return self.request("call", addr=self.addr(addr), args=[int(a) for a in args], thread=thread,
                            force=force, timeout_ms=timeout_ms)


def f32_bits(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


def f64_bits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]
