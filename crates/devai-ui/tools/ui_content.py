#!/usr/bin/env python3
"""Критерий «содержимое окна» (REQ-QA-01..03 задачи 01a1acec).

Пара артефактов на прогон = скриншот webview (screenshot.png) + текстовый
дамп accessibility-дерева (axtree.txt). Прогион «окно жив» без пары или с
error-page/белым содержимым не может пройти QA.

Режимы:
  snap      снять пару с ЖИВОГО окна (AT-SPI + scrot); режим запуска
            (dev через vite / standalone) не важен — окно то же.
  check     проверить пару артефактов против порогов из ui_expectations.json.
  selftest  регрессионные образцы qa-samples: known-good даёт PASS,
            known-bad — FAIL; иначе харнесс сломан и ему нельзя верить.
  auto      применимость (у задачи есть UI-поверхность?) + поиск пар в
            evidence-директории задачи + check каждой.
  gate      selftest, затем auto — точка входа платформенного гейта.

Коды выхода: 0 PASS / N/A, 1 FAIL, 2 ошибка использования,
127 харнесс недоступен (нет python-gi/Atspi, scrot и т.п.) — платформенная
семантика shell 126/127 превращает это в BLOCKED, не в FAIL (config.rs:124-141).
Зависимости: только stdlib; snap дополнительно gi.Atspi + scrot + DISPLAY.
"""

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import warnings

warnings.filterwarnings("ignore", category=DeprecationWarning)

EXIT_PASS = 0
EXIT_FAIL = 1
EXIT_USAGE = 2
EXIT_BLOCKED = 127

AXTREE = "axtree.txt"
SHOT = "screenshot.png"
# Аудит-артефакты producer пишет под <evidence>/ui/**; остальное evidence
# (воспроизведения, логи) в поиск пар не попадает.
UI_SUBDIR = "ui"
SURFACE_MARKERS = ("tauri.conf.json", ".ui-audit-surface.json")
SKIP_DIRS = {".git", "node_modules", "target", "dist", "frontend/dist"}
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
SHOT_MIN_BYTES = 1000
MAX_NODES = 4000
TEXT_CAP = 240
TOOLS_DIR = os.path.dirname(os.path.abspath(__file__))
CRATE_DIR = os.path.dirname(TOOLS_DIR)
DEFAULT_EXPECT = os.path.join(TOOLS_DIR, "ui_expectations.json")
SAMPLES_DIR = os.path.join(CRATE_DIR, "qa-samples")


def load_expect(path):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def die(msg, code=EXIT_FAIL):
    print(msg)
    sys.exit(code)


# ---------------------------------------------------------------------------
# snap
# ---------------------------------------------------------------------------

def _atspi_text(node):
    try:
        return node.get_text(0, -1) or ""
    except Exception:
        return ""


def _atspi_links(node):
    """URIs гиперссылок ноды (если Text-интерфейс их отдаёт)."""
    uris = []
    try:
        for i in range(node.get_link_count()):
            hl = node.get_link(i)
            if hl is not None:
                uris.append(hl.get_uri(i) if hasattr(hl, "get_uri") else "")
    except Exception:
        pass
    return [u for u in uris if u]


def _walk(node, depth, lines):
    if len(lines) >= MAX_NODES:
        return
    try:
        name = node.get_name() or ""
        role = node.get_role_name() or "?"
    except Exception as exc:  # дерево может «таять» под ногами
        lines.append("  " * depth + f"[broken] {exc}")
        return
    text = _atspi_text(node)
    uris = _atspi_links(node)
    extra = ""
    if text and text != name:
        extra += f" text={text[:TEXT_CAP]!r}"
    for u in uris:
        extra += f" uri={u!r}"
    lines.append("  " * depth + f"[{role}] {name[:TEXT_CAP]!r}{extra}")
    for i in range(node.get_child_count()):
        _walk(node.get_child_at_index(i), depth + 1, lines)


def _pick_app(substr, pick, want_pid):
    try:
        import gi

        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
    except Exception as exc:
        die(f"snap: AT-SPI bindings unavailable ({exc}) — install gi/Atspi typelib",
            EXIT_BLOCKED)
    matches = []
    desk = Atspi.get_desktop(0)
    for i in range(desk.get_child_count()):
        app = desk.get_child_at_index(i)
        try:
            name = app.get_name() or ""
        except Exception:
            continue
        if substr.casefold() not in name.casefold():
            continue
        pid = None
        try:
            pid = app.get_process_id()
        except Exception:
            pass
        if want_pid is not None and pid != want_pid:
            continue
        matches.append(app)
    if not matches:
        return None
    return matches[-1] if pick == "last" else matches[0]


def _focus_and_shoot(out_dir, pid):
    """Снимает ОКНО целевого процесса (import -window <id> — не требует
    фокуса, в отличие от scrot -u); root/scrot — только fallback, иначе кадр
    может описывать не проверяемое окно."""
    shot = os.path.join(out_dir, SHOT)
    win = None
    try:
        res = subprocess.run(["xdotool", "search", "--pid", str(pid)],
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             timeout=10)
        ids = res.stdout.decode().split()
        if ids:
            win = ids[-1]
            subprocess.run(["xdotool", "windowactivate", "--sync", win],
                           stderr=subprocess.DEVNULL, timeout=10)
    except Exception:
        win = None
    attempts = ([["import", "-window", win, shot]] if win else []) + \
               [["import", "-window", "root", shot], ["scrot", shot]]
    ok = False
    for cmd in attempts:
        try:
            ok = subprocess.call(cmd, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL) == 0
        except FileNotFoundError:
            ok = False
        if ok and os.path.exists(shot):
            break
    if not ok or not os.path.exists(shot):
        die("snap: no screenshot tool (import/scrot missing?) — cannot capture",
            EXIT_BLOCKED)
    size = os.path.getsize(shot)
    with open(shot, "rb") as fh:
        magic = fh.read(8)
    if size < SHOT_MIN_BYTES or magic != PNG_MAGIC:
        die(f"snap: screenshot invalid (size {size}, png={magic == PNG_MAGIC})")
    return size


def cmd_snap(args):
    app = _pick_app(args.window, args.pick, args.pid)
    if app is None:
        die(f"snap: no live window matching {args.window!r}"
            + (f" pid={args.pid}" if args.pid else "")
            + " — criterion needs a RUNNING UI (BLOCKED, не FAIL)")
    os.makedirs(args.out, exist_ok=True)
    lines = []
    _walk(app, 0, lines)
    stamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
    header = [
        "# devai-ui-content-snapshot v1",
        f"# captured_at: {stamp}",
        f"# window_filter: {args.window}  picked: {args.pick}  pid: {app.get_process_id()}",
        f"# nodes: {len(lines)}",
        "# --- axtree ---",
    ]
    ax_path = os.path.join(args.out, AXTREE)
    with open(ax_path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(header + lines) + "\n")
    size = _focus_and_shoot(args.out, app.get_process_id())
    print(f"snap: {ax_path} ({len(lines)} nodes), "
          f"{os.path.join(args.out, SHOT)} ({size} bytes)")
    return EXIT_PASS


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------

def _ax_lines(pair_dir):
    path = os.path.join(pair_dir, AXTREE)
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8", errors="replace") as fh:
        return fh.read()


def _node_count(ax):
    return len([l for l in ax.splitlines() if re.match(r"^\s*\[", l)])


def check_pair(pair_dir, exp):
    """Пороги критерия по одной паре. Возвращает список нарушений (пусто = PASS)."""
    v = []
    ax = _ax_lines(pair_dir)
    if ax is None:
        v.append(f"отсутствует {AXTREE} — пары артефактов нет")
        ax = ""
    shot = os.path.join(pair_dir, SHOT)
    if not os.path.exists(shot):
        v.append(f"отсутствует {SHOT} — пары артефактов нет")
    else:
        with open(shot, "rb") as fh:
            if fh.read(8) != PNG_MAGIC or os.path.getsize(shot) < SHOT_MIN_BYTES:
                v.append(f"{SHOT} битый/тривиальный (не PNG или < {SHOT_MIN_BYTES} байт)")
    blob = ax.casefold()
    for err in exp.get("error_texts", []):
        if err.casefold() in blob:
            v.append(f"error-page текст в артефактах: {err!r}")
    nodes = _node_count(ax)
    if nodes < exp.get("min_ax_nodes", 1):
        v.append(f"AX-дерево не заполнено: узлов {nodes} < минимума "
                 f"{exp.get('min_ax_nodes')} (белый экран?)")
    for req in exp.get("required_elements", []):
        alts = req.get("any_of", [])
        if not any(a.casefold() in blob for a in alts):
            v.append(f"обязательный навигационный элемент отсутствует: {req['id']} "
                     f"(искали {' | '.join(alts)})")
    return v


def cmd_check(args):
    exp = load_expect(args.expect)
    v = check_pair(args.dir, exp)
    for line in v:
        print(f"VIOLATION: {line}")
    if v:
        print(f"check {args.dir}: FAIL ({len(v)} нарушений)")
        return EXIT_FAIL
    print(f"check {args.dir}: PASS")
    return EXIT_PASS


# ---------------------------------------------------------------------------
# selftest — регрессионные образцы REQ-QA-03
# ---------------------------------------------------------------------------

def cmd_selftest(args):
    exp = load_expect(args.expect)
    good = os.path.join(SAMPLES_DIR, "known-good")
    bad = os.path.join(SAMPLES_DIR, "known-bad")
    for d in (good, bad):
        if not os.path.isdir(d):
            die(f"selftest: образец {d} отсутствует — QA-база неполна")
    v_good = check_pair(good, exp)
    v_bad = check_pair(bad, exp)
    ok = True
    if v_good:
        print(f"selftest: known-good даёт нарушения ({v_good}) — харнесс сломан")
        ok = False
    if not v_bad:
        print("selftest: known-bad проходит как good — порог не работает, "
              "белый экран снова может пройти QA")
        ok = False
    if ok:
        print(f"selftest: PASS (known-good → PASS, known-bad → FAIL: {v_bad[0]})")
        return EXIT_PASS
    return EXIT_FAIL


# ---------------------------------------------------------------------------
# auto — применимость + пары в evidence
# ---------------------------------------------------------------------------

def surface_roots(ws):
    """UI-поверхности: каталоги с маркером (tauri.conf.json / явный
    .ui-audit-surface.json). Маркер-данные, не имя проекта."""
    roots = []
    for dirpath, dirnames, filenames in os.walk(ws):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS
                       and not (dirpath.replace(os.sep, "/").endswith("/frontend")
                               and d == "dist")]
        for marker in SURFACE_MARKERS:
            if marker in filenames:
                roots.append(dirpath)
                break
    return sorted(set(roots))


def expectations_for(root):
    for cand in (os.path.join(root, "tools", "ui_expectations.json"),
                 os.path.join(root, "ui_expectations.json")):
        if os.path.exists(cand):
            return cand
    return None


def changed_paths(ws):
    """Пути, затронутые задачей: закоммиченное на rework-ветке относительно
    main + грязное дерево."""
    out = set()
    base = _git(ws, ["merge-base", "HEAD", "main"]) or _git(ws, ["rev-parse", "HEAD"])
    diff = (_git(ws, ["diff", "--name-only", f"{base}..HEAD"]) or "") if base else ""
    out.update(l for l in diff.splitlines() if l)
    status = _git(ws, ["status", "--porcelain"]) or ""
    for line in status.splitlines():
        out.add(line[3:].strip().split(" -> ")[-1])
    return {p for p in out if p}


def _git(ws, args):
    try:
        res = subprocess.run(["git", *args], cwd=ws,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             timeout=30)
        return res.stdout.decode(errors="replace") if res.returncode == 0 else None
    except Exception:
        return None


def task_id_from(ws):
    branch = (_git(ws, ["rev-parse", "--abbrev-ref", "HEAD"]) or "").strip()
    m = re.match(r"^rework/(.+)$", branch)
    if m:
        return m.group(1)
    name = os.path.basename(ws.rstrip(os.sep))
    return name if re.match(r"^[0-9a-f-]{12,}$", name) else None


def discover_pairs(ws):
    """Пары артефактов прогона: <evidence>/ui/** задачи. DEVAI_EVIDENCE_DIR
    (текущий execution) плюс все execution задачи в <home>/evidence/<task>/."""
    scan = []
    ev = os.environ.get("DEVAI_EVIDENCE_DIR")
    if ev:
        scan.append(os.path.join(ev, UI_SUBDIR))
    home = os.environ.get("DEVAI_HOME") or os.path.expanduser("~/.devai")
    tid = task_id_from(ws)
    if tid:
        task_root = os.path.join(home, "evidence", tid)
        if os.path.isdir(task_root):
            for ex in sorted(os.listdir(task_root)):
                scan.append(os.path.join(task_root, ex, UI_SUBDIR))
    found = {}
    for root in scan:
        if not os.path.isdir(root):
            continue
        for dirpath, _dirnames, filenames in os.walk(root):
            if AXTREE in filenames or SHOT in filenames:
                found[os.path.abspath(dirpath)] = True
    return sorted(found)


def cmd_auto(args):
    ws = os.path.abspath(args.ws)
    roots = surface_roots(ws)
    touched = []
    if roots:
        changed = changed_paths(ws)
        for r in roots:
            rel = os.path.relpath(r, ws)
            if any(p == rel or p.startswith(rel + "/") for p in changed):
                touched.append(r)
    pairs = discover_pairs(ws)
    if not touched and not pairs:
        print(f"auto: N/A — у задачи нет UI-поверхности "
              f"(поверхностей в дереве: {len(roots)}, пар в evidence: 0)")
        return EXIT_PASS
    if not pairs:
        die("auto: UI-поверхность затронута "
            f"({', '.join(os.path.relpath(r, ws) for r in touched)}), "
            f"но пара артефактов (снимок+{AXTREE}) в evidence не найдена — "
            "«окно живо» без съёмки не принимается")
    sel = touched or roots
    expect_files = []
    for r in sel:
        ef = expectations_for(r)
        if ef is None:
            die(f"auto: для UI-поверхности {r} не задекларированы ожидания "
                "(tools/ui_expectations.json) — набор обязательных элементов "
                "должен быть данными, не угадыванием")
        expect_files.append(ef)
    rc = EXIT_PASS
    for ef in expect_files:
        exp = load_expect(ef)
        for d in pairs:
            v = check_pair(d, exp)
            for line in v:
                print(f"VIOLATION[{os.path.relpath(ef, ws)}] {d}: {line}")
            if v:
                rc = EXIT_FAIL
    if rc == EXIT_PASS:
        print(f"auto: PASS — пар проверено {len(pairs)}, ожиданий "
              f"{len(expect_files)}, нарушений нет")
    return rc


def cmd_gate(args):
    rc = cmd_selftest(args)
    if rc != EXIT_PASS:
        die("gate: самоконтроль харнесса не прошёл — гейт не может судить")
    return cmd_auto(args)


def main():
    p = argparse.ArgumentParser(prog="ui_content.py", description=__doc__.splitlines()[0])
    p.add_argument("--expect", default=DEFAULT_EXPECT,
                   help="файл ожиданий (пороги/набор элементов)")
    sub = p.add_subparsers(dest="mode", required=True)

    s = sub.add_parser("snap", help="снять пару с живого окна")
    s.add_argument("--out", required=True)
    s.add_argument("--window", default="devai-ui")
    s.add_argument("--pick", choices=["first", "last"], default="first")
    s.add_argument("--pid", type=int, default=None,
                   help="точечная привязка к процессу окна")

    c = sub.add_parser("check", help="проверить пару артефактов")
    c.add_argument("dir")

    sub.add_parser("selftest", help="проверить порог на регрессионных образцах")

    a = sub.add_parser("auto", help="применимость + evidence-пары задачи")
    a.add_argument("--ws", default=os.getcwd())
    g = sub.add_parser("gate", help="selftest + auto (точка входа платформенного гейта)")
    g.add_argument("--ws", default=os.getcwd())

    args = p.parse_args()
    fn = {"snap": cmd_snap, "check": cmd_check, "selftest": cmd_selftest,
          "auto": cmd_auto, "gate": cmd_gate}[args.mode]
    sys.exit(fn(args) or EXIT_PASS)


if __name__ == "__main__":
    main()
