#!/usr/bin/env python3
"""QA-run: загрузчик профиля оценки + вердикт-функция + рендер отчёта по §2.6.

Парсит YAML профиля без внешних зависимостей (PyYAML может отсутствовать) —
ровно нужные блоки. Запуск:

    python3 docs/qa-run.py <run.json> [profile.yaml]

run.json — вывод оценщика (баллы + претензии), НЕ код проекта:
{
  "gates_ok": true,
  "scores": { "<check-id>": 1.0 },
  "findings": [ {"check": "<id>", "text": "что + file:line + чем кончится"} ],
  "blocked_reasons": { "<check-id>": "почему проба не выполнена" },
  "pending_human": ["<check-id>"]
}

Вердикт детерминирован (схема §2.5): PASS = gates ∧ floors ∧ S>=θ ∧ нет
pending_human; FAIL = breach пола или S<θ; INCONCLUSIVE = blocked-вес > cap.
"""
import json, re, sys

PROF = sys.argv[2] if len(sys.argv) > 2 else \
    __file__.rsplit("/", 1)[0] + "/profiles/devai.yaml"
P = open(PROF).read()

theta = float(re.findall(r"\n  theta: ([0-9.]+)", P)[0])
cap = float(re.findall(r"\n    max_blocked_weight: ([0-9.]+)", P)[0])
floors_decl = {k: (None if v == "null" else float(v))
               for k, v in re.findall(r"\n    (catastrophic|major|minor): (null|[0-9.]+)", P)}
weights = {k: float(v) for k, v in
           re.findall(r"\n    ([a-z_]+/[a-z0-9-]+): ([0-9.]+)",
                      P.split("\n  weights:")[1])}

et_block = P.split("error_types:")[1].split("gates:")[0]
sev, cur = {}, None
for line in et_block.splitlines():
    m = re.match(r"  ([a-z-]+/[a-z-]+):", line)
    if m:
        cur = m.group(1)
    m = re.match(r"    severity: (\w+)", line)
    if m and cur:
        sev[cur] = m.group(1)

detects = {}
for m in re.finditer(r"- id: ([a-z_]+/[a-z0-9-]+)\s*\n(?:.*?\n)*?\s+detects: \[([^\]]+)\]", P):
    detects[m.group(1)] = [d.strip() for d in m.group(2).split(",")]

rank = {"cosmetic": -1, "minor": 0, "major": 1, "catastrophic": 2}
worst_sev, floors = {}, {}
for c, dets in detects.items():
    worst_sev[c] = max((sev[e] for e in dets), key=lambda s: rank[s])
    floors[c] = floors_decl[worst_sev[c]]

# инварианты загрузки (V7/V8 схемы)
assert abs(sum(weights.values()) - 1.0) < 1e-9, "Σ весов != 1.0"
assert set(weights) == set(detects), f"веса/проверки разошлись: {set(weights) ^ set(detects)}"

run = json.load(open(sys.argv[1]))
sc, fnd = run["scores"], run["findings"]
blocked_r, pending = run["blocked_reasons"], run["pending_human"]
for c in sc:
    assert c in weights, f"в отчёте неизвестный check {c}"
unknown = [c for c in weights if c not in sc and c not in blocked_r]
assert not unknown, f"нет баллов/блок-причин для: {unknown}"

blocked_w = sum(weights[c] for c in blocked_r)
raw = sum(weights[c] * s for c, s in sc.items())
S = raw / (1 - blocked_w) if blocked_w < 1 else 0.0
breach = [c for c, s in sc.items() if floors.get(c) is not None and s < floors[c]]

if not run.get("gates_ok", True):
    verdict, sub = "НЕ ПРОШЁЛ", "шлюз не пройден"
elif blocked_w > cap:
    verdict, sub = "ВЕРДИКТ НЕ ГОТОВ", f"невыполненных проб на вес {blocked_w:.2f} > {cap}"
elif breach:
    verdict, sub = "НЕ ПРОШЁЛ", "красные линии: " + \
        ", ".join(f"{b}={sc[b]}<{floors[b]}" for b in breach)
elif S < theta:
    verdict, sub = "НЕ ПРОШЁЛ", f"S<{theta}"
elif pending:
    verdict, sub = "ВЕРДИКТ НЕ ГОТОВ", "нет человеческой подписи"
else:
    verdict, sub = "ПРОШЁЛ", "красных линий нет"

# ── рендер по §2.6: число-шапка, только претензии, worst-first ───────────────
lines = [f"ВЕРДИКТ: {verdict} (S={S:.3f}/1.00, порог {theta}, {sub})", ""]
ordered = sorted(fnd, key=lambda f: -rank[worst_sev.get(f["check"], "cosmetic")])
if ordered:
    lines.append("ЧТО ПЛОХО:")
    lines += [f"{i}. [{f['check']}] {f['text']}" for i, f in enumerate(ordered, 1)]
    lines.append("")
if blocked_r:
    lines.append("НЕ ПРОВЕРЕНО: " + "; ".join(f"{c} — {r}" for c, r in blocked_r.items()))
    lines.append("")
if pending:
    lines.append("ЖДЁТ ЧЕЛОВЕКА: " + ", ".join(pending))
print("\n".join(lines))
