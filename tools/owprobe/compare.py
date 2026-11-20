"""The server's movement against the client's prediction, command frame by command frame.

Server: the `orpheus::pose` debug trace (`RUST_LOG=info,orpheus::pose=debug`; one `owner pose` line per command
frame a body ran: frame, position (feet), velocity, movement flags, the states that began and ended). Client: an
owprobe trace (`trace --corrections`): each frame sample's mover state (component 31 +0xD40: the frame it was
simulated to, feet, velocity, flags) and the hits of the correction hooks.

    python -m tools.owprobe compare --server play.log --trace trace.jsonl [--body A0000101] [--out diff.jsonl]

Per command frame: the client's last sample at that frame against the server's pose, the distance, both flag words,
and the marks: C = the client's movement correction gate took a server record of that frame (return 1), R = a
statescript rollback ran while the client was at that frame, A = an owner chunk was applied.
"""
import json
import math
import re
from collections import defaultdict

ANSI = re.compile(r"\x1b\[[0-9;]*m")
POSE = re.compile(r"owner pose body=(?P<body>[0-9A-Fa-f]+) frame=(?P<frame>\d+) position=\[(?P<pos>[^\]]*)\] "
                  r"velocity=\[(?P<vel>[^\]]*)\] flags=(?P<flags>0x[0-9a-fA-F]+) began=\[(?P<began>[^\]]*)\] "
                  r"ended=\[(?P<ended>[^\]]*)\]")


def _floats(text):
    return [float(v) for v in text.split(",") if v.strip()]


def _names(text):
    return [v.strip().strip('"') for v in text.split(",") if v.strip()]


def server_poses(path, body=None):
    """{frame: {position, velocity, flags, began, ended}} of one body (the first body seen when not given)."""
    poses = {}
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            if "owner pose" not in line:
                continue
            m = POSE.search(ANSI.sub("", line))
            if not m:
                continue
            if body is None:
                body = m["body"].upper()
            if m["body"].upper() != body:
                continue
            poses[int(m["frame"])] = {"position": _floats(m["pos"]), "velocity": _floats(m["vel"]),
                                      "flags": int(m["flags"], 16), "began": _names(m["began"]),
                                      "ended": _names(m["ended"])}
    return body, poses


def client_trace(path):
    """(frames, marks): {mover frame: the last sample at it}; {frame: set of marks} from the hook records."""
    records = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
    hooks = {}
    for r in records:
        if r.get("kind") == "text" and r.get("text", "").startswith("hook "):
            # "hook <id> <role>" notes written by `trace --corrections`.
            _, ident, role = r["text"].split(maxsplit=2)
            hooks[int(ident)] = role
    frames, marks = {}, defaultdict(set)
    current = None
    # The gate's hit holds the record's frame; its return (on the same thread, next) says whether it corrected.
    gate = {}
    for r in records:
        kind = r.get("kind")
        if kind == "frame":
            mover = r.get("mover")
            if mover:
                frames[mover["frame"]] = r
                current = mover["frame"]
        elif kind in ("hit", "return") and current is not None:
            role = hooks.get(r.get("hook"))
            if role == "correction" and kind == "hit":
                frame = (r.get("reads") or [None])[0]
                gate[r["thread"]] = frame if isinstance(frame, int) else current
            elif role == "correction" and kind == "return":
                frame = gate.pop(r["thread"], current)
                if r.get("rax", 0) & 0xFF == 1:
                    marks[frame].add("C")
            elif role == "rollback" and kind == "hit":
                marks[current].add("R")
            elif role == "applier" and kind == "hit":
                marks[current].add("A")
    return frames, marks


def distance(a, b):
    return math.sqrt(sum((x - y) ** 2 for x, y in zip(a, b)))


def compare(server, trace, body=None, offset=0):
    """Rows per command frame both sides have (client frame + offset = server frame). The body: the given id,
    else the one the trace recorded."""
    frames, marks = client_trace(trace)
    if body is None:
        body = next((f"{r['body']:08X}" for r in frames.values() if r.get("body")), None)
    body, poses = server_poses(server, body.upper() if body else None)
    rows = []
    for frame in sorted(frames):
        sample = frames[frame]
        pose = poses.get(frame + offset)
        if pose is None:
            continue
        mover = sample["mover"]
        rows.append({"frame": frame, "client": mover["position"], "server": pose["position"],
                     "distance": distance(mover["position"], pose["position"]),
                     "client_velocity": mover["velocity"], "server_velocity": pose["velocity"],
                     "client_flags": mover["flags"], "server_flags": pose["flags"],
                     "marks": "".join(sorted(marks.get(frame, ()))),
                     "began": pose["began"], "ended": pose["ended"], "t": sample["t"]})
    return body, rows, marks


def best_offset(server, trace, body=None, span=3):
    """The client/server frame offset with the smallest mean distance (0 when the frames line up)."""
    scores = {}
    for offset in range(-span, span + 1):
        _, rows, _ = compare(server, trace, body, offset)
        if rows:
            scores[offset] = (sum(r["distance"] for r in rows) / len(rows), len(rows))
    return scores


def report(server, trace, body=None, threshold=0.01, out=None, log=print):
    body, rows, marks = compare(server, trace, body)
    if not rows:
        log(f"no common frames (server body {body}); check the server log has orpheus::pose lines")
        return rows
    scores = best_offset(server, trace, body)
    log(f"body {body}: {len(rows)} common command frames {rows[0]['frame']}..{rows[-1]['frame']}; mean distance "
        f"by client->server frame offset: " + ", ".join(f"{o:+d}: {m:.4f} m ({n})" for o, (m, n) in scores.items()))
    worst = max(rows, key=lambda r: r["distance"])
    log(f"max distance {worst['distance']:.4f} m at frame {worst['frame']}; "
        f"{sum(r['distance'] > threshold for r in rows)} frames over {threshold} m; "
        f"corrections {sum('C' in m for m in marks.values())}, frames with rollbacks "
        f"{sum('R' in m for m in marks.values())}")
    log(f"{'frame':>7} {'dist':>8}  {'client feet':>27}  {'server feet':>27}  flags c/s     marks states")
    for r in rows:
        if (r["distance"] > threshold or "C" in r["marks"] or r["began"] or r["ended"]
                or r["client_flags"] != r["server_flags"]):
            c = "(" + ", ".join(f"{v:7.3f}" for v in r["client"]) + ")"
            s = "(" + ", ".join(f"{v:7.3f}" for v in r["server"]) + ")"
            states = " ".join(["+" + b for b in r["began"]] + ["-" + e for e in r["ended"]])
            log(f"{r['frame']:>7} {r['distance']:8.4f}  {c:>27}  {s:>27}  {r['client_flags']:#x}/{r['server_flags']:#x}"
                f"  {r['marks']:5} {states}")
    if out:
        with open(out, "w", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
        log(f"-> {out}")
    return rows
