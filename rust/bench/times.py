"""Response times per stage from _logs/conversations.jsonl, written the same way by both brains.
Usage: python times.py conversations.jsonl FIRST_LINE [LAST_LINE]  ->  medians (and max) per path: shortcut or llm.
`stt` is what the brain was handed: wake word scan + Whisper (both lines measure it the same way)."""
import json, statistics, sys
from collections import defaultdict
by = defaultdict(lambda: defaultdict(list))
for n, line in enumerate(open(sys.argv[1], encoding="utf-8"), 1):
    if n < int(sys.argv[2]) or (len(sys.argv) > 3 and n > int(sys.argv[3])):
        continue
    try:
        d = json.loads(line)
    except ValueError:
        continue
    t, via = d.get("timing") or {}, (d.get("intent") or {}).get("via", "?")
    for k in ("stt", "shortcut", "llm", "action"):
        if isinstance(t.get(k), (int, float)):
            by[via][k].append(t[k])
    # end to end, comparable across lines: the brains' own `total` starts at different points (Node: after Whisper)
    by[via]["end_to_end"].append(sum(t.get(k) or 0 for k in ("stt", "shortcut", "llm", "action")))
for via, ks in by.items():
    print(via, " ".join(f"{k} med {statistics.median(v):.0f} max {max(v):.0f} (n={len(v)})" for k, v in ks.items()))
