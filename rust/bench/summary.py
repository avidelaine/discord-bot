"""Summarize sample.ps1 CSVs: per process name, mean CPU % (of one core), mean and peak RAM and GPU memory.
Usage: python summary.py run.csv [from_ms to_ms]"""
import csv, sys
from collections import defaultdict
rows = list(csv.DictReader(open(sys.argv[1])))
lo, hi = (int(sys.argv[2]), int(sys.argv[3])) if len(sys.argv) > 3 else (0, 1 << 62)
by = defaultdict(list)
for r in rows:
    if lo <= int(r["ts"]) <= hi:
        by[(r["name"], r["pid"])].append((int(r["ts"]), float(r["cpu_ms"]), float(r["ws_mb"]), float(r["vram_mb"])))
agg = defaultdict(lambda: {"cpu": 0.0, "ws": [], "vram": [], "span": 0})
for (name, _), s in by.items():
    a = agg[name]
    if len(s) > 1:
        a["cpu"] += (s[-1][1] - s[0][1]); a["span"] = max(a["span"], s[-1][0] - s[0][0])
    a["ws"].append([x[2] for x in s]); a["vram"].append([x[3] for x in s])
print(f"{'process':14} {'cpu %':>6} {'ram MB avg':>10} {'ram MB max':>10} {'vram MB avg':>11} {'vram MB max':>11}")
tot = [0, 0, 0, 0, 0]
for name, a in sorted(agg.items()):
    # sum the processes of one name sample by sample (python can be several), approximated by summing per-pid means
    ws_avg = sum(sum(w) / len(w) for w in a["ws"]); ws_max = sum(max(w) for w in a["ws"])
    v_avg = sum(sum(v) / len(v) for v in a["vram"]); v_max = sum(max(v) for v in a["vram"])
    cpu = 100 * a["cpu"] / a["span"] if a["span"] else 0
    for i, x in enumerate((cpu, ws_avg, ws_max, v_avg, v_max)): tot[i] += x
    print(f"{name:14} {cpu:6.1f} {ws_avg:10.0f} {ws_max:10.0f} {v_avg:11.0f} {v_max:11.0f}")
print(f"{'TOTAL':14} {tot[0]:6.1f} {tot[1]:10.0f} {tot[2]:10.0f} {tot[3]:11.0f} {tot[4]:11.0f}")
