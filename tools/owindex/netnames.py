"""Roles and index-derived names for the network tables (rows from tools/owindex/net.py, owner NetTables).

-> (roles, symbols): roles = [(rva, text)] shown by Index.addr(); symbols = [(rva, name, kind)].

Names (only when unambiguous; addr() always lists every role):
- Msg<id>_Handler: a handler rva used by one message id. A handler shared by several ids (not the no-op stub)
  is named after the lowest id, Msg<lowest>_Handler_Shared<n>; the no-op stub every dispatcher uses for
  ignored ids is Msg_IgnoredStub.
- MsgDispatch_<protocol>: a dispatcher whose rows are all of one protocol, and the only dispatcher of that
  protocol; otherwise MsgDispatch_<protocol>_<rva hex> (several protocols: joined with '_').
- Msg<id>_Info / Msg<id>_Vtbl: message info record / message class vtable used by one id.
- Proto_<protocol>_Vtbl: protocol class vtable.
- Comp<id>_Ctor / _Create / _Vtbl / _FactoryVtbl / _FactoryCtor: component addresses used by one component.
<protocol> = the protocol name, else P<crc 8 hex>. A name that would land on several rvas gets _<rva hex>.
STU-derived names win over these at the same rva; docs/analysis names win over both.
"""

from collections import defaultdict

STUB_HOW = "no-op stub"  # net.py's handler_how text for ids a dispatcher ignores


def proto_label(name, crc):
    return name or (f"P{crc:08X}" if crc is not None else "P?")


def _ids_text(ids, cap=12):
    ids = sorted(ids)
    shown = ", ".join(str(i) for i in ids[:cap])
    return shown + (f" ... ({len(ids)} ids)" if len(ids) > cap else "")


def net_names(net):
    messages = (net or {}).get("messages") or []
    components = (net or {}).get("components") or []
    protocols = (net or {}).get("protocols") or []
    roles, symbols = [], []

    def proto(m):
        return proto_label(m.get("protocol_name"), m.get("protocol_crc"))

    # --- handlers ---
    by_handler = defaultdict(list)
    for m in messages:
        if m.get("handler_rva"):
            by_handler[m["handler_rva"]].append(m)
    for rva, rows in by_handler.items():
        ids = {m["id"] for m in rows}
        stub = all(STUB_HOW in (m.get("handler_how") or "") for m in rows)
        disp = {m.get("dispatcher_rva") for m in rows if m.get("dispatcher_rva")}
        if stub:
            roles.append((rva, f"ignored-message stub (returns 0): {len(ids)} message ids across {len(disp)} "
                               f"dispatchers ({_ids_text(ids)})"))  # fmt: skip
            symbols.append((rva, "Msg_IgnoredStub", "func"))
            continue
        for m in sorted(rows, key=lambda m: (m["id"], m.get("dispatcher_rva") or 0))[:8]:
            ch = m.get("channel")
            where = f"{proto(m)}" + (f", channel {ch}" if ch is not None else "")
            d = m.get("dispatcher_rva")
            how = ", ".join(x for x in (m.get("dispatch_kind"), m.get("handler_how")) if x)
            text = f"handler of message {m['id']} ({where})"
            if d:
                text += f" under dispatcher {d:#x}" + (f" ({how})" if how else "")
            roles.append((rva, text))
        if len(rows) > 8:
            roles.append((rva, f"handler of {len(rows) - 8} more message rows (ids {_ids_text(ids)})"))
        low = min(ids)
        symbols.append((rva, f"Msg{low}_Handler" if len(ids) == 1 else f"Msg{low}_Handler_Shared{len(ids)}", "func"))

    # --- dispatchers ---
    by_disp = defaultdict(list)
    for m in messages:
        if m.get("dispatcher_rva"):
            by_disp[m["dispatcher_rva"]].append(m)
    disp_of_proto = defaultdict(set)
    for rva, rows in by_disp.items():
        for m in rows:
            disp_of_proto[proto(m)].add(rva)
    for rva, rows in sorted(by_disp.items()):
        protos = sorted({proto(m) for m in rows})
        chans = sorted({m.get("channel") for m in rows if m.get("channel") is not None})
        ids = {m["id"] for m in rows}
        ignored = sum(1 for m in rows if STUB_HOW in (m.get("handler_how") or ""))
        kinds = sorted({m.get("dispatch_kind") for m in rows if m.get("dispatch_kind")})
        text = (f"message dispatcher of {', '.join(protos)}" + (f" (channel {', '.join(map(str, chans))})" if chans else "")
                + f": {len(ids)} ids {min(ids)}-{max(ids)}" + (f", {ignored} ignored" if ignored else "")
                + (f" ({', '.join(kinds)})" if kinds else ""))  # fmt: skip
        roles.append((rva, text))
        unique = len(protos) == 1 and len(disp_of_proto[protos[0]]) == 1
        symbols.append((rva, f"MsgDispatch_{'_'.join(protos)}" + ("" if unique else f"_{rva:X}"), "func"))

    # --- message info records and class vtables ---
    for key, what, suffix in (("info_rva", "message info record", "Info"), ("class_vtable_rva", "message class vtable", "Vtbl")):
        users = defaultdict(set)
        protos = {}
        for m in messages:
            if m.get(key):
                users[m[key]].add(m["id"])
                protos[m[key]] = proto(m)
        for rva, ids in users.items():
            roles.append((rva, f"{what} of message {_ids_text(ids)} ({protos[rva]})"))
            if len(ids) == 1:
                symbols.append((rva, f"Msg{min(ids)}_{suffix}", "data"))

    # --- protocols ---
    vt_protos = defaultdict(list)
    for p in protocols:
        if p.get("vtable_rva"):
            vt_protos[p["vtable_rva"]].append(p)
    for rva, ps in vt_protos.items():
        for p in ps:
            label = proto_label(p.get("name"), p.get("crc"))
            ch = p.get("channel")
            roles.append((rva, f"protocol class vtable of {label}: ids {p.get('first_id')}-{p.get('end_id')}"
                               + (f", channel {ch}" if ch is not None else "")))  # fmt: skip
        if len(ps) == 1:
            symbols.append((rva, f"Proto_{proto_label(ps[0].get('name'), ps[0].get('crc'))}_Vtbl", "data"))

    # --- components ---
    comp_roles = (
        ("construct_rva", "constructor", "Ctor", "func"),
        ("create_rva", "create", "Create", "func"),
        ("vtable_rva", "vtable", "Vtbl", "data"),
        ("factory_vtable_rva", "factory vtable", "FactoryVtbl", "data"),
        ("factory_ctor_rva", "factory constructor", "FactoryCtor", "func"),
    )
    for key, what, suffix, kind in comp_roles:
        users = defaultdict(list)
        for c in components:
            if c.get(key):
                users[c[key]].append(c)
        for rva, cs in users.items():
            for c in cs:
                extra = [x for x in (c.get("name"), c.get("label")) if x]
                if c.get("size"):
                    extra.append(f"size {c['size']:#x}")
                if c.get("class_hash") is not None:
                    extra.append(f"STU_{c['class_hash']:08X}")
                text = f"component {c['id']} {what}" + (f" ({'; '.join(extra)})" if extra else "")
                roles.append((rva, text))
            if len(cs) == 1:
                symbols.append((rva, f"Comp{cs[0]['id']}_{suffix}", kind))
    # the same name on several rvas (e.g. one id with a handler under each of two dispatchers): add the rva
    count = defaultdict(int)
    for _, name, _ in symbols:
        count[name] += 1
    symbols = [(rva, name if count[name] == 1 else f"{name}_{rva:X}", kind) for rva, name, kind in symbols]
    return roles, symbols
