# -*- coding: utf-8 -*-
"""
Merge roads from Terrain Builder .tv4p projects (DayZ/Arma).

Commands:
  python tv4p_merge_roads.py export base.tv4p roads.json
  python tv4p_merge_roads.py merge base.tv4p donor.tv4p output.tv4p
  python tv4p_merge_roads.py types base.tv4p
  python tv4p_merge_roads.py roundtrip base.tv4p

Both projects must use the same Road Tool road types and junction definitions
(blocks 0x88 and 0x89). Non-road data comes from the base project (A); only new
roads from the donor project (B) are added. Always back up and verify the result
in Terrain Builder. This tool is based on reverse engineering; the file format
is not officially documented.
"""
import json
import struct
import sys

MAGIC_ENTRY = b"\x06\x00\x0d"
TAG_ROAD_TYPES = 0x88
TAG_XDEF = 0x89
TAG_ROADS = 0x8A

ROAD_STRIDE = 992
PART_STRIDE = 128


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def u16(b, o):
    return struct.unpack_from("<H", b, o)[0]


def f64(b, o):
    return struct.unpack_from("<d", b, o)[0]


def parse_entries(data, pos, entries_len, count):
    """Parse a 06 00 0D entry list and return entry/body offsets, length, type and ID."""
    out = []
    end = pos + entries_len
    for _ in range(count):
        if pos + 7 > end or data[pos:pos + 3] != MAGIC_ENTRY:
            return None
        blen = u32(data, pos + 3)
        bs = pos + 7
        be = bs + blen
        if be > end or blen < 6:
            return None
        out.append((pos, bs, blen, u16(data, bs), u32(data, bs + 2)))
        pos = be
    return out


def find_list(data, tag, validate=None):
    """Find a tag 00 0C block and return its start, length, count and entries."""
    pat = bytes([tag, 0x00, 0x0C])
    idx = 0
    cands = []
    while True:
        i = data.find(pat, idx)
        if i < 0:
            break
        if i + 11 <= len(data):
            ln = u32(data, i + 3)
            cnt = u32(data, i + 7)
            if 4 <= ln and i + 7 + ln <= len(data) and cnt <= 20000:
                ent = parse_entries(data, i + 11, ln - 4, cnt)
                if ent is not None and (validate is None or validate(data, ent)):
                    cands.append((i, ln, cnt, i + 11, ent))
        idx = i + 1
    if validate is not None:
        cands = [c for c in cands if validate(data, c[4])]
    return cands


def is_roads_block(data, entries):
    return len(entries) > 0 and all(e[3] == 0x1A for e in entries)


def parse_fields(body):
    """Parse entry fields into (tag, type, value) tuples."""
    flds = []
    q = 6
    n = len(body)
    while q + 3 <= n:
        tag, typ = body[q], body[q + 2]
        q += 3
        if typ == 0x0B:
            ln = u16(body, q)
            q += 2
            flds.append((tag, typ, body[q:q + ln]))
            q += ln
        elif typ in (0x05, 0x0D):
            flds.append((tag, typ, body[q:q + 4]))
            q += 4
        elif typ == 0x09:
            flds.append((tag, typ, body[q:q + 1]))
            q += 1
        elif typ == 0x08:
            flds.append((tag, typ, body[q:q + 4]))
            q += 4
        elif typ == 0x14:
            flds.append((tag, typ, body[q:q + 8]))
            q += 8
        elif typ == 0x15:
            cnt = body[q]
            flds.append((tag, typ, body[q:q + 1 + cnt * 8]))
            q += 1 + cnt * 8
        elif typ == 0x20:
            flds.append((tag, typ, body[q:q + 3]))
            q += 3
        elif typ == 0x0C:
            ll = u32(body, q)
            cc = u32(body, q + 4)
            flds.append((tag, typ, (ll, cc, q + 8)))
            q += 8 + (ll - 4)
        else:
            raise ValueError("Unknown field type 0x%02x" % typ)
    return flds


def road_signature(body):
    """Return a road signature for deduplication, excluding IDs."""
    parts = []
    head = []
    for tag, typ, val in parse_fields(body):
        if typ == 0x0C:
            ll, cc, sub = val
            items = []
            sp = sub
            for _ in range(cc):
                sl = u32(body, sp + 3)
                sb = sp + 7
                sub_b = body[sb:sb + sl]
                d = {}
                for t2, y2, v2 in parse_fields(sub_b):
                    d[t2] = bytes(v2) if isinstance(v2, (bytes, bytearray)) else v2
                items.append((d.get(0x7F, b""), d.get(0x6C, b""), d.get(0x33, b"")))
                sp += 7 + sl
            parts.append((tag, tuple(items)))
        else:
            head.append((tag, bytes(val)))
    return (tuple(head), tuple(parts))


def road_info(body):
    """Return readable road details: start, length, model and part counts."""
    info = {"length": None, "start": None, "model": None, "part_counts": {}}
    for tag, typ, val in parse_fields(body):
        if tag == 0x8C and typ == 0x14:
            info["length"] = round(f64(val, 0), 2)
        elif tag == 0x8E and typ == 0x15:
            cnt = val[0]
            info["start"] = [round(f64(val, 1 + j * 8), 2) for j in range(cnt)]
        elif tag == 0x91 and typ == 0x0B:
            info["model"] = bytes(val).decode("utf-8", "replace")
        elif typ == 0x0C:
            ll, cc, _ = val
            info["part_counts"]["0x%02x" % tag] = cc
    info["total_parts"] = sum(info["part_counts"].values())
    return info


def collect_ids(data):
    used = set()
    pos = 0
    while True:
        i = data.find(MAGIC_ENTRY, pos)
        if i < 0 or i + 7 + 6 > len(data):
            break
        blen = u32(data, i + 3)
        bs = i + 7
        if blen >= 6 and bs + blen <= len(data):
            rid = u32(data, bs + 2)
            if rid:
                used.add(rid)
        pos = i + 1
    return used


def set_entry_id(buf, body_off, new_id):
    struct.pack_into("<I", buf, body_off + 2, new_id)


def reid_body(body, new_road_id, alloc_part_id):
    """Assign unique IDs to a road entry and its parts, returning a new body."""
    body = bytearray(body)
    set_entry_id(body, 0, new_road_id)
    q = 6
    while q + 3 <= len(body):
        tag, typ = body[q], body[q + 2]
        q += 3
        if typ == 0x0B:
            q += 2 + u16(body, q)
        elif typ in (0x05, 0x0D):
            q += 4
        elif typ == 0x09:
            q += 1
        elif typ == 0x08:
            q += 4
        elif typ == 0x14:
            q += 8
        elif typ == 0x15:
            q += 1 + body[q] * 8
        elif typ == 0x20:
            q += 3
        elif typ == 0x0C:
            ll = u32(body, q)
            cc = u32(body, q + 4)
            q += 8
            for _ in range(cc):
                assert body[q:q + 3] == MAGIC_ENTRY
                sl = u32(body, q + 3)
                sb = q + 7
                nid = alloc_part_id()
                set_entry_id(body, sb, nid)
                q += 7 + sl
        else:
            raise ValueError("Unknown field type")
    return body


def zero_ids_recursive(body):
    """Copy a body with its entry ID and all nested entry IDs set to zero."""
    body = bytearray(body)
    body[2:6] = b"\x00\x00\x00\x00"
    q = 6
    while q + 3 <= len(body):
        tag, typ = body[q], body[q + 2]
        q += 3
        if typ == 0x0B:
            q += 2 + u16(body, q)
        elif typ in (0x05, 0x0D):
            q += 4
        elif typ == 0x09:
            q += 1
        elif typ == 0x08:
            q += 4
        elif typ == 0x14:
            q += 8
        elif typ == 0x15:
            q += 1 + body[q] * 8
        elif typ == 0x20:
            q += 3
        elif typ == 0x0C:
            ll = u32(body, q)
            q += 8
            end = q + (ll - 4)
            while q + 7 <= end:
                assert body[q:q + 3] == MAGIC_ENTRY, "Unexpected list structure"
                sl = u32(body, q + 3)
                sb = q + 7
                sub = zero_ids_recursive(bytes(body[sb:sb + sl]))
                body[sb:sb + sl] = sub
                q += 7 + sl
            assert q == end, "List length mismatch"
        else:
            raise ValueError("Unknown field type 0x%02x" % typ)
    return bytes(body)


def normalized_block(data, start, list_len):
    """Canonicalize a block for logical comparison, ignoring nested IDs.

    Terrain Builder rewrites entry IDs when saving, so files with identical
    Road Tool settings may differ only in their IDs.
    """
    norm = []
    pos = start + 11
    end = start + 7 + list_len
    while pos + 7 <= end:
        assert data[pos:pos + 3] == MAGIC_ENTRY
        blen = u32(data, pos + 3)
        bs = pos + 7
        body = bytes(data[bs:bs + blen])
        tid = u16(data, bs)
        norm.append((tid, zero_ids_recursive(body)))
        pos = bs + blen
    return norm


def build_list_field(tag, entry_bodies):
    payload = bytearray()
    for b in entry_bodies:
        payload += MAGIC_ENTRY + struct.pack("<I", len(b)) + b
    field = (bytes([tag, 0x00, 0x0C]) + struct.pack("<I", len(payload) + 4)
             + struct.pack("<I", len(entry_bodies)) + payload)
    return field


def find_single(data, tag, typ):
    pat = bytes([tag, 0x00, typ])
    occ = []
    pos = 0
    while True:
        i = data.find(pat, pos)
        if i < 0:
            break
        occ.append(i)
        pos = i + 1
    if len(occ) != 1:
        raise ValueError("Tag 0x%02x: expected one occurrence, found %d" % (tag, len(occ)))
    return occ[0]


def cmd_export(path, out_json):
    data = open(path, "rb").read()
    roads = find_list(data, TAG_ROADS, is_roads_block)
    if len(roads) != 1:
        raise SystemExit("Could not find a unique road block 0x8A (candidates: %d)" % len(roads))
    _, ln, cnt, estart, entries = roads[0]
    recs = []
    for i, (_, bs, blen, tid, rid) in enumerate(entries):
        body = data[bs:bs + blen]
        rec = {"index": i, "id": rid}
        rec.update(road_info(body))
        recs.append(rec)
    total_parts = sum(r["total_parts"] for r in recs)
    xs = [r["start"][0] for r in recs if r["start"]]
    ys = [r["start"][1] for r in recs if r["start"]]
    summary = {"file": path, "roads": len(recs), "total_parts": total_parts,
               "bbox_E": [min(xs), max(xs)] if xs else None,
               "bbox_N": [min(ys), max(ys)] if ys else None}
    open(out_json, "w", encoding="utf-8").write(
        json.dumps({"summary": summary, "roads": recs}, ensure_ascii=False, indent=1))
    print("Roads: %d, total parts: %d" % (len(recs), total_parts))
    print("Bounding box E: %s  N: %s" % (summary["bbox_E"], summary["bbox_N"]))
    print("Saved: %s" % out_json)


def cmd_roundtrip(path):
    data = open(path, "rb").read()
    roads = find_list(data, TAG_ROADS, is_roads_block)
    assert len(roads) == 1
    start, ln, cnt, estart, entries = roads[0]
    bodies = [data[bs:bs + blen] for (_, bs, blen, _, _) in entries]
    rebuilt = build_list_field(TAG_ROADS, bodies)
    orig = data[start:start + 7 + ln]
    print("Original: %d B, rebuilt: %d B, identical: %s"
          % (len(orig), len(rebuilt), orig == rebuilt))
    return orig == rebuilt


def cmd_merge(a_path, b_path, out_path):
    a = open(a_path, "rb").read()
    b = open(b_path, "rb").read()

    ra = find_list(a, TAG_ROADS, is_roads_block)
    rb = find_list(b, TAG_ROADS, is_roads_block)
    if len(ra) != 1 or len(rb) != 1:
        raise SystemExit("Ambiguous road block 0x8A")
    sa, lna, cnta, esa, enta = ra[0]
    sb, lnb, cntb, esb, entb = rb[0]

    # Require matching Road Tool settings (road types and junctions).
    # Compare logical contents; Terrain Builder may rewrite internal IDs on save.
    for tag in (TAG_ROAD_TYPES, TAG_XDEF):
        la = find_list(a, tag)
        lb = find_list(b, tag)
        if len(la) != 1 or len(lb) != 1:
            raise SystemExit("Ambiguous block 0x%02x" % tag)
        na = normalized_block(a, la[0][0], la[0][1])
        nb = normalized_block(b, lb[0][0], lb[0][1])
        if na != nb:
            raise SystemExit(
                "MISMATCHED block 0x%02x -- projects use different Road Tool settings "
                "(road types, order or parts). Synchronize them in Terrain Builder or "
                "with tv4p-road-tool (extract from the canonical project, then patch "
                "the other project); otherwise the merge may mix up type indexes." % tag)
        raw_a = a[la[0][0]:la[0][0] + 7 + la[0][1]]
        raw_b = b[lb[0][0]:lb[0][0] + 7 + lb[0][1]]
        if raw_a != raw_b:
            print("  0x%02x: logically compatible; only internal IDs differ "
                  "(Terrain Builder rewrites them on save). Merging." % tag)
        else:
            print("  0x%02x: identyczne." % tag)
    print("Road Tool settings match; projects can be merged.")

    bodies_a = [a[bs:bs + blen] for (_, bs, blen, _, _) in enta]
    bodies_b = [b[bs:bs + blen] for (_, bs, blen, _, _) in entb]
    sigs = {}
    for body in bodies_a:
        sigs.setdefault(road_signature(body), []).append(body)
    new_roads, dup = [], 0
    for body in bodies_b:
        if road_signature(body) in sigs:
            dup += 1
        else:
            new_roads.append(body)
    print("A=%d  B=%d  shared=%d  new from B=%d" % (len(bodies_a), len(bodies_b), dup, len(new_roads)))
    if not new_roads:
        print("No new roads to add.")
        return

    used = collect_ids(a) | collect_ids(b)
    # Continue the road ID sequence (last ID from A + 992).
    last_road = max(e[4] for e in enta)
    next_road = last_road + ROAD_STRIDE
    while next_road in used:
        next_road += ROAD_STRIDE
    # Part IDs are globally unique and start at max ID + 128.
    part_state = [max(used) + PART_STRIDE]

    def alloc_part():
        while part_state[0] in used:
            part_state[0] += PART_STRIDE
        used.add(part_state[0])
        nid = part_state[0]
        part_state[0] += PART_STRIDE
        return nid

    merged = list(bodies_a)
    for body in new_roads:
        while next_road in used:
            next_road += ROAD_STRIDE
        used.add(next_road)
        merged.append(reid_body(body, next_road, alloc_part))
        next_road += ROAD_STRIDE

    field = build_list_field(TAG_ROADS, merged)
    old_start, old_ln = sa, lna
    old_end = old_start + 7 + old_ln
    delta = len(field) - (old_end - old_start)
    out = bytearray(a[:old_start] + field + a[old_end:])

    # Correct offsets after the insertion point:
    #  - 0x3F/0x0D (metadata before 0x8A; size includes the 0x8A block) += delta
    #  - 0x18/0x0D (offset after the block)                             += delta
    #  - Do not change 0x3E/0x0D, 0x19/0x20 or 0x40 (header fields).
    shift = - (len(a) - len(out))  # Equal to delta, computed from the final buffer.
    p3f = find_single(out, 0x3F, 0x0D)
    struct.pack_into("<I", out, p3f + 3, (u32(out, p3f + 3) + delta) & 0xFFFFFFFF)
    p18 = find_single(out, 0x18, 0x0D)
    struct.pack_into("<I", out, p18 + 3, (u32(out, p18 + 3) + delta) & 0xFFFFFFFF)
    assert shift == delta, (shift, delta)

    open(out_path, "wb").write(out)
    print("Road block 0x8A size change: %+d B (0x3F and 0x18 corrected)" % delta)
    print("Saved: %s (%d roads)" % (out_path, len(merged)))
    print("VERIFY: open in Terrain Builder and check Road Tool, layers and rasters.")


def cmd_types(path):
    """Print comparable lists of road types (0x88) and junction definitions (0x89)."""
    data = open(path, "rb").read()
    print("== %s ==" % path)
    for tag, label in ((TAG_ROAD_TYPES, "road types 0x88"), (TAG_XDEF, "junctions 0x89")):
        bl = find_list(data, tag)
        if len(bl) != 1:
            print("  %s: AMBIGUOUS (%d candidates)" % (label, len(bl)))
            continue
        _, ln, cnt, _, entries = bl[0]
        print("  %s: %d wpisow" % (label, cnt))
        for i, (_, bs, blen, tid, rid) in enumerate(entries):
            body = data[bs:bs + blen]
            name, model, sub = None, None, {}
            for t2, y2, v2 in parse_fields(body):
                if y2 == 0x0B and t2 in (0x33,):
                    name = bytes(v2).decode("utf-8", "replace")
                elif y2 == 0x0B and t2 in (0x7C, 0x91):
                    model = bytes(v2).decode("utf-8", "replace")
                elif y2 == 0x0C:
                    sub["0x%02x" % t2] = v2[1]
            extra = (" model=%s" % model) if model else ""
            extra += (" " + " ".join("%s=%d" % kv for kv in sorted(sub.items()))) if sub else ""
            print("    [%d] id=%d name=%s%s" % (i, rid, name, extra))


def gui_main():
    """Simple graphical interface using only the standard library and Tkinter."""
    import os
    import threading
    bundle_dir = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    if getattr(sys, "frozen", False):
        for variable, folder in (("TCL_LIBRARY", "_tcl_data"), ("TK_LIBRARY", "_tk_data")):
            runtime_path = os.path.join(bundle_dir, folder)
            if os.path.isdir(runtime_path):
                os.environ[variable] = runtime_path
    import tkinter as tk
    from tkinter import filedialog, messagebox, ttk

    root = tk.Tk()
    root.title("tv4p_merge_roads — Terrain Builder Road Merger (.tv4p)")
    icon_path = os.path.join(bundle_dir, "app_icon.ico")
    if os.path.isfile(icon_path):
        try:
            root.iconbitmap(default=icon_path)
        except tk.TclError:
            pass
    root.geometry("680x560")
    root.minsize(600, 500)

    pad = {"padx": 6, "pady": 3}

    # --- variables ---
    var_a = tk.StringVar()
    var_b = tk.StringVar()
    var_out = tk.StringVar(value="output.tv4p")
    var_exp_in = tk.StringVar()
    var_exp_out = tk.StringVar(value="roads.json")
    var_tool_in = tk.StringVar()

    # --- log ---
    log_frame = ttk.LabelFrame(root, text="Log")
    log_text = tk.Text(log_frame, height=12, wrap="word", state="disabled")
    log_scroll = ttk.Scrollbar(log_frame, command=log_text.yview)
    log_text.configure(yscrollcommand=log_scroll.set)

    def log(msg=""):
        log_text.configure(state="normal")
        log_text.insert("end", str(msg) + "\n")
        log_text.see("end")
        log_text.configure(state="disabled")
        root.update_idletasks()

    class GuiStream:
        def write(self, s):
            s = str(s)
            if s.strip():
                # print() may call write() with just a newline; ignore empty lines.
                for line in s.splitlines():
                    if line.strip():
                        log(line)
        def flush(self):
            pass

    def run_worker(fn, done_msg=None):
        """Run fn in the background and redirect print() output to the log."""
        import sys as _sys
        def target():
            old = _sys.stdout
            _sys.stdout = GuiStream()
            try:
                fn()
                if done_msg:
                    log(done_msg)
            except SystemExit as e:
                log("ERROR: %s" % e)
                if str(e) and str(e) != "0":
                    messagebox.showerror("Error", str(e))
            except Exception as e:  # noqa: BLE001
                log("ERROR: %r" % e)
                messagebox.showerror("Error", "%r" % e)
            finally:
                _sys.stdout = old
                btn_merge.configure(state="normal")
                btn_export.configure(state="normal")
                btn_types.configure(state="normal")
                btn_roundtrip.configure(state="normal")
        for b in (btn_merge, btn_export, btn_types, btn_roundtrip):
            b.configure(state="disabled")
        threading.Thread(target=target, daemon=True).start()

    def pick(var, title="Select a .tv4p file", save=False, defext=".tv4p"):
        if save:
            p = filedialog.asksaveasfilename(title=title, defaultextension=defext,
                                             filetypes=[("Terrain Builder project", "*.tv4p"),
                                                        ("All files", "*.*")])
        else:
            p = filedialog.askopenfilename(title=title,
                                           filetypes=[("Terrain Builder project", "*.tv4p"),
                                                      ("All files", "*.*")])
        if p:
            var.set(p)

    def pick_json_save(var):
        p = filedialog.asksaveasfilename(title="Save JSON as…", defaultextension=".json",
                                         filetypes=[("JSON", "*.json"), ("All files", "*.*")])
        if p:
            var.set(p)

    # --- tabs ---
    nb = ttk.Notebook(root)
    tab_merge = ttk.Frame(nb)
    tab_tools = ttk.Frame(nb)
    nb.add(tab_merge, text="Merge")
    nb.add(tab_tools, text="Export / Types / Integrity")

    # --- Merge tab ---
    r = 0
    ttk.Label(tab_merge, text="Project A (base — current world):").grid(row=r, column=0, sticky="w", **pad)
    r += 1
    ttk.Entry(tab_merge, textvariable=var_a, width=60).grid(row=r, column=0, sticky="ew", **pad)
    ttk.Button(tab_merge, text="Browse…", command=lambda: pick(var_a)).grid(row=r, column=1, **pad)
    r += 1
    ttk.Label(tab_merge, text="Project B (road donor):").grid(row=r, column=0, sticky="w", **pad)
    r += 1
    ttk.Entry(tab_merge, textvariable=var_b, width=60).grid(row=r, column=0, sticky="ew", **pad)
    ttk.Button(tab_merge, text="Browse…", command=lambda: pick(var_b)).grid(row=r, column=1, **pad)
    r += 1
    ttk.Label(tab_merge, text="Output project:").grid(row=r, column=0, sticky="w", **pad)
    r += 1
    ttk.Entry(tab_merge, textvariable=var_out, width=60).grid(row=r, column=0, sticky="ew", **pad)
    ttk.Button(tab_merge, text="Save as…",
               command=lambda: pick(var_out, title="Save output as…", save=True)).grid(row=r, column=1, **pad)
    r += 1
    btn_merge = ttk.Button(tab_merge, text="Merge roads",
                           command=lambda: run_worker(
                               lambda: cmd_merge(var_a.get(), var_b.get(), var_out.get()),
                               "Done. Verify the result in Terrain Builder."))
    btn_merge.grid(row=r, column=0, columnspan=2, pady=10, sticky="ew")
    r += 1
    ttk.Label(tab_merge, text="A = base (world to keep), B = road donor.\n"
                              "After merging: make a backup and verify in Terrain Builder.",
              foreground="gray").grid(row=r, column=0, columnspan=2, sticky="w", **pad)
    tab_merge.columnconfigure(0, weight=1)

    # --- Tools tab ---
    r = 0
    ttk.Label(tab_tools, text="Export road list to JSON:").grid(row=r, column=0, sticky="w", **pad)
    r += 1
    ttk.Entry(tab_tools, textvariable=var_exp_in, width=60).grid(row=r, column=0, sticky="ew", **pad)
    ttk.Button(tab_tools, text="Browse…", command=lambda: pick(var_exp_in, title="Select a .tv4p file to export")).grid(row=r, column=1, **pad)
    r += 1
    ttk.Entry(tab_tools, textvariable=var_exp_out, width=60).grid(row=r, column=0, sticky="ew", **pad)
    ttk.Button(tab_tools, text="JSON as…", command=lambda: pick_json_save(var_exp_out)).grid(row=r, column=1, **pad)
    r += 1
    btn_export = ttk.Button(tab_tools, text="Export to JSON",
                            command=lambda: run_worker(
                                lambda: cmd_export(var_exp_in.get(), var_exp_out.get()),
                                "JSON saved."))
    btn_export.grid(row=r, column=0, columnspan=2, sticky="ew", **pad)
    r += 1
    ttk.Separator(tab_tools, orient="horizontal").grid(row=r, column=0, columnspan=2, sticky="ew", pady=8)
    r += 1
    ttk.Label(tab_tools, text="Road types / integrity check:").grid(row=r, column=0, sticky="w", **pad)
    r += 1
    ttk.Entry(tab_tools, textvariable=var_tool_in, width=60).grid(row=r, column=0, sticky="ew", **pad)
    ttk.Button(tab_tools, text="Browse…", command=lambda: pick(var_tool_in)).grid(row=r, column=1, **pad)
    r += 1
    btn_row = ttk.Frame(tab_tools)
    btn_row.grid(row=r, column=0, columnspan=2, sticky="ew")
    btn_types = ttk.Button(btn_row, text="Show road types",
                           command=lambda: run_worker(lambda: cmd_types(var_tool_in.get())))
    btn_types.pack(side="left", expand=True, fill="x", padx=(0, 3))
    btn_roundtrip = ttk.Button(btn_row, text="Test roundtrip",
                               command=lambda: run_worker(
                                   lambda: log("Roundtrip identical: %s"
                                               % cmd_roundtrip(var_tool_in.get()))))
    btn_roundtrip.pack(side="left", expand=True, fill="x", padx=(3, 0))
    tab_tools.columnconfigure(0, weight=1)

    nb.pack(fill="both", expand=False, padx=6, pady=6)
    log_frame.pack(fill="both", expand=True, padx=6, pady=(0, 6))
    log_text.pack(side="left", fill="both", expand=True)
    log_scroll.pack(side="right", fill="y")

    # Convenience: use A as the default input for the other tools.
    def _sync_exp(*_):
        if var_a.get() and not var_exp_in.get():
            var_exp_in.set(var_a.get())
        if var_a.get() and not var_tool_in.get():
            var_tool_in.set(var_a.get())
    var_a.trace_add("write", _sync_exp)

    log("Ready. Select the projects and click Merge roads.")
    log("A = base, B = road donor. Verify the result in Terrain Builder.")
    root.mainloop()


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] in ("gui", "--gui"):
        if len(sys.argv) >= 3:
            print(__doc__)
            sys.exit(1)
        try:
            gui_main()
        except ImportError as e:
            print("Tkinter is unavailable (%s). Use the CLI instead:\n" % e)
            print(__doc__)
            sys.exit(1)
    else:
        cmd = sys.argv[1]
        if cmd == "export":
            cmd_export(sys.argv[2], sys.argv[3])
        elif cmd == "merge":
            cmd_merge(sys.argv[2], sys.argv[3], sys.argv[4])
        elif cmd == "roundtrip":
            ok = cmd_roundtrip(sys.argv[2])
            sys.exit(0 if ok else 2)
        elif cmd == "types":
            cmd_types(sys.argv[2])
        else:
            raise SystemExit("Unknown command: %s" % cmd)
