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
            known-bad — FAIL, edge-sparse (бедный живой UI) — PASS и
            отличается от known-bad; иначе харнесс сломан и ему нельзя верить.
  auto      применимость (у задачи есть UI-поверхность?) + поиск пар в
            evidence-директории задачи + check каждой.
  gate      selftest, затем auto — точка входа платформенного гейта.

Коды выхода: 0 PASS / N/A; 1 FAIL — вердикт по содержимому; 2 BLOCKED
продюсера съёмки («не по чему оценивать»: нет живого окна, gi/Atspi или
инструмента захвата) — различим от FAIL (R1); 127 BLOCKED платформенной
пробы — невыполнимая проба (битый/невалидный expectations-файл, неполная
QA-база образцов, провалившийся самоконтроль гейта): shell-семантика
126/127 → BLOCKED, не FAIL (config.rs:124-141, evaluation.rs:111-128).
usage-ошибка argparse тоже выходит 2 — различима от FAIL (1) по выводу.
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
from typing import NoReturn

warnings.filterwarnings("ignore", category=DeprecationWarning)

EXIT_PASS = 0
EXIT_FAIL = 1
EXIT_BLOCKED = 2
EXIT_BLOCKED_PROBE = 127

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
REQUIRED_EXPECT_KEYS = ("error_texts", "min_ax_nodes", "required_elements")


def die(msg, code=EXIT_FAIL) -> NoReturn:
    print(msg)
    sys.exit(code)


# ---------------------------------------------------------------------------
# ожидания (пороги) — файл поверхности обязан быть полными данными (R2)
# ---------------------------------------------------------------------------

def validate_expectations(raw):
    """Чистая проверка expectations-файла. Возвращает список ошибок (пусто =
    годен). Дефолтов нет: потерянный ключ = отсутствие обязательного ключа,
    молча degrade порог запрещён (fail-closed)."""
    if not isinstance(raw, dict):
        return ["ожидания: корень JSON должен быть объектом"]
    errors = []
    for key in REQUIRED_EXPECT_KEYS:
        if key not in raw:
            errors.append(f"нет обязательного ключа {key!r}")
    if errors:
        return errors
    error_texts = raw["error_texts"]
    if not isinstance(error_texts, list) or not all(
            isinstance(t, str) and t for t in error_texts):
        errors.append("error_texts должен быть списком непустых строк "
                      "(пустой список — сознательная декларация «текстов нет»)")
    min_nodes = raw["min_ax_nodes"]
    if isinstance(min_nodes, bool) or not isinstance(min_nodes, int) \
            or min_nodes < 1:
        errors.append("min_ax_nodes должен быть целым >= 1")
    elements = raw["required_elements"]
    if not isinstance(elements, list) or not elements:
        errors.append("required_elements должен быть непустым списком "
                      "(обязательные элементы — данные поверхности)")
    else:
        for i, req in enumerate(elements):
            if not isinstance(req, dict):
                errors.append(f"required_elements[{i}] должен быть объектом")
                continue
            if not isinstance(req.get("id"), str) or not req.get("id"):
                errors.append(f"required_elements[{i}]: нет строки id")
            alts = req.get("any_of")
            if not isinstance(alts, list) or not alts or not all(
                    isinstance(a, str) and a for a in alts):
                errors.append(f"required_elements[{i}] "
                              f"({req.get('id', '?')}): any_of должен быть "
                              "непустым списком непустых строк")
    return errors


def load_expect(path):
    """Загрузка + валидация ожиданий; любая поломка — loud-ошибка загрузки,
    BLOCKED платформенной пробы (127), не FAIL содержимого и не молчаливый
    пропуск (R2; семантика config.rs:124-141)."""
    try:
        with open(path, encoding="utf-8") as fh:
            raw = json.load(fh)
    except OSError as exc:
        die(f"expectations {path}: не читается — {exc} (BLOCKED, дефолтов нет)",
            EXIT_BLOCKED_PROBE)
    except ValueError as exc:
        die(f"expectations {path}: не JSON — {exc} (BLOCKED, дефолтов нет)",
            EXIT_BLOCKED_PROBE)
    errors = validate_expectations(raw)
    if errors:
        die(f"expectations {path}: невалиден — порог не деградирует молча "
            f"(BLOCKED): " + "; ".join(errors), EXIT_BLOCKED_PROBE)
    return raw


# ---------------------------------------------------------------------------
# snap
# ---------------------------------------------------------------------------

def _atspi_text(node):
    try:
        return node.get_text(0, -1) or ""
    except Exception:
        return ""


def _atspi_links(node):
    """URIs цели гиперссылки ноды (если Text-интерфейс их отдаёт)."""
    uris = []
    try:
        for i in range(node.get_link_count()):
            link_target = node.get_link(i)
            if link_target is not None:
                uris.append(link_target.get_uri(i)
                            if hasattr(link_target, "get_uri") else "")
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
        die(f"snap: AT-SPI bindings unavailable ({exc}) — install gi/Atspi "
            "typelib (BLOCKED, не FAIL)", EXIT_BLOCKED)
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


def shot_state(path):
    """Факт о снимке без суждений: {exists, size, magic_ok}."""
    if not os.path.exists(path):
        return {"exists": False, "size": 0, "magic_ok": False}
    size = os.path.getsize(path)
    with open(path, "rb") as fh:
        magic_ok = fh.read(8) == PNG_MAGIC
    return {"exists": True, "size": size, "magic_ok": magic_ok}


def classify_capture(capture_ok, state):
    """Чистое ядро решения snap-съёмки (R6): по результату запуска инструментов
    и факту о файле возвращает (класс, намерение) без side-effects:
      ("blocked", причина) — ни один инструмент не снял кадр (нет харнесса);
      ("invalid", причина) — кадр есть, но битый/тривиальный;
      ("ok", размер)      — кадр годен."""
    if not capture_ok or not state["exists"]:
        return ("blocked", "snap: no screenshot tool (import/scrot missing?) "
                "— cannot capture (BLOCKED, не FAIL)")
    if state["size"] < SHOT_MIN_BYTES or not state["magic_ok"]:
        return ("invalid", f"snap: screenshot invalid (size {state['size']}, "
                f"png={state['magic_ok']})")
    return ("ok", state["size"])


def _focus_and_shoot(out_dir, pid):
    """Снимает ОКНО целевого процесса (import -window <id> — не требует
    фокуса, в отличие от scrot -u); root/scrot — только fallback, иначе кадр
    может описывать не проверяемое окно. Решение о классе съёмки — в чистом
    classify_capture; здесь только запуск инструментов (граница съёмки
    не мок-ается осознанно)."""
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
    capture_ok = False
    for cmd in attempts:
        try:
            capture_ok = subprocess.call(cmd, stdout=subprocess.DEVNULL,
                                         stderr=subprocess.DEVNULL) == 0
        except FileNotFoundError:
            capture_ok = False
        if capture_ok and os.path.exists(shot):
            break
    verdict, payload = classify_capture(capture_ok, shot_state(shot))
    if verdict == "blocked":
        die(payload, EXIT_BLOCKED)
    if verdict == "invalid":
        die(payload, EXIT_FAIL)
    return payload


def cmd_snap(args):
    app = _pick_app(args.window, args.pick, args.pid)
    if app is None:
        die(f"snap: no live window matching {args.window!r}"
            + (f" pid={args.pid}" if args.pid else "")
            + " — criterion needs a RUNNING UI (BLOCKED, не FAIL)",
            EXIT_BLOCKED)
    os.makedirs(args.out, exist_ok=True)
    lines = []
    _walk(app, 0, lines)
    stamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
    header = [
        "# devai-ui-content-snapshot v1",
        f"# captured_at: {stamp}",
        f"# window_filter: {args.window}  picked: {args.pick}  "
        f"pid: {app.get_process_id()}",
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


def evaluate_pair(ax_text, shot, expectations):
    """Чистое ядро порогов критерия (R6): по тексту AX-дампа (None = файла
    нет), факту о снимке (shot_state) и валидированным ожиданиям — список
    нарушений (пусто = PASS). Side-effects нет; пороги читаются прямыми
    ключами — валидация при загрузке гарантировала их (R2)."""
    violations = []
    if ax_text is None:
        violations.append(f"отсутствует {AXTREE} — пары артефактов нет")
        ax_text = ""
    if not shot["exists"]:
        violations.append(f"отсутствует {SHOT} — пары артефактов нет")
    elif shot["size"] < SHOT_MIN_BYTES or not shot["magic_ok"]:
        violations.append(f"{SHOT} битый/тривиальный (не PNG или "
                          f"< {SHOT_MIN_BYTES} байт)")
    blob = ax_text.casefold()
    for err in expectations["error_texts"]:
        if err.casefold() in blob:
            violations.append(f"error-page текст в артефактах: {err!r}")
    nodes = _node_count(ax_text)
    if nodes < expectations["min_ax_nodes"]:
        violations.append(f"AX-дерево не заполнено: узлов {nodes} < минимума "
                          f"{expectations['min_ax_nodes']} (белый экран?)")
    for req in expectations["required_elements"]:
        alts = req["any_of"]
        if not any(a.casefold() in blob for a in alts):
            violations.append(f"обязательный навигационный элемент отсутствует: "
                              f"{req['id']} (искали {' | '.join(alts)})")
    return violations


def check_pair(pair_dir, expectations):
    """Пороги критерия по одной паре артефактов (IO-обёртка над
    evaluate_pair). Возвращает список нарушений (пусто = PASS)."""
    ax_text = _ax_lines(pair_dir)
    shot = shot_state(os.path.join(pair_dir, SHOT))
    return evaluate_pair(ax_text, shot, expectations)


def cmd_check(args):
    expectations = load_expect(args.expect)
    violations = check_pair(args.dir, expectations)
    for line in violations:
        print(f"VIOLATION: {line}")
    if violations:
        print(f"check {args.dir}: FAIL ({len(violations)} нарушений)")
        return EXIT_FAIL
    print(f"check {args.dir}: PASS")
    return EXIT_PASS


# ---------------------------------------------------------------------------
# selftest — регрессионные образцы REQ-QA-03 (+ край запаса R3)
# ---------------------------------------------------------------------------

def cmd_selftest(args):
    expectations = load_expect(args.expect)
    samples = {
        "known-good": os.path.join(SAMPLES_DIR, "known-good"),
        "known-bad": os.path.join(SAMPLES_DIR, "known-bad"),
        "edge-sparse": os.path.join(SAMPLES_DIR, "edge-sparse"),
    }
    for name, d in samples.items():
        if not os.path.isdir(d):
            die(f"selftest: образец {d} отсутствует — QA-база неполна "
                "(BLOCKED, не FAIL)", EXIT_BLOCKED_PROBE)
    verdicts = {name: check_pair(d, expectations)
                for name, d in samples.items()}
    nodes = {name: _node_count(_ax_lines(d) or "")
             for name, d in samples.items()}
    threshold = expectations["min_ax_nodes"]
    ok = True
    if verdicts["known-good"]:
        print(f"selftest: known-good даёт нарушения ({verdicts['known-good']}) "
              "— харнесс сломан")
        ok = False
    if not verdicts["known-bad"]:
        print("selftest: known-bad проходит как good — порог не работает, "
              "белый экран снова может пройти QA")
        ok = False
    if verdicts["edge-sparse"]:
        print(f"selftest: edge-sparse (бедный живой UI, узлов {nodes['edge-sparse']} "
              f"против порога {threshold}) даёт FAIL — порог отсекает "
              "легитимный тонкий интерфейс; критерий не различает край и "
              "known-bad")
        ok = False
    if ok:
        print(f"selftest: PASS (known-good → PASS, {nodes['known-good']} узлов; "
              f"edge-sparse → PASS, {nodes['edge-sparse']} узлов при пороге "
              f"{threshold}, маржа +{nodes['edge-sparse'] - threshold}; "
              f"known-bad → FAIL, {nodes['known-bad']} узлов: "
              f"{verdicts['known-bad'][0]})")
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


def touched_surfaces(ws):
    """Eligibility (R5): поверхности-маркеры в дереве + затронутые diff'ом."""
    roots = surface_roots(ws)
    if not roots:
        return roots, []
    changed = changed_paths(ws)
    touched = []
    for root in roots:
        rel = os.path.relpath(root, ws)
        if any(p == rel or p.startswith(rel + "/") for p in changed):
            touched.append(root)
    return roots, touched


def surface_expectation_files(selected, ws):
    """Ожидания для каждой выбранной поверхности; незадекларированные —
    FAIL пробы (набор элементов — данные, не угадывание)."""
    expectation_paths = []
    for root in selected:
        expect_path = expectations_for(root)
        if expect_path is None:
            die(f"auto: для UI-поверхности {root} не задекларированы ожидания "
                "(tools/ui_expectations.json) — набор обязательных элементов "
                "должен быть данными, не угадыванием")
        expectation_paths.append(expect_path)
    return expectation_paths


def check_pairs(pairs, expectation_paths, ws):
    """Проверка всех пар против всех ожиданий затронутых поверхностей."""
    rc = EXIT_PASS
    for expect_path in expectation_paths:
        expectations = load_expect(expect_path)
        for pair_dir in pairs:
            violations = check_pair(pair_dir, expectations)
            for line in violations:
                print(f"VIOLATION[{os.path.relpath(expect_path, ws)}] "
                      f"{pair_dir}: {line}")
            if violations:
                rc = EXIT_FAIL
    return rc


def cmd_auto(args):
    ws = os.path.abspath(args.ws)
    roots, touched = touched_surfaces(ws)
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
    expectation_paths = surface_expectation_files(touched or roots, ws)
    rc = check_pairs(pairs, expectation_paths, ws)
    if rc == EXIT_PASS:
        print(f"auto: PASS — пар проверено {len(pairs)}, ожиданий "
              f"{len(expectation_paths)}, нарушений нет")
    return rc


def cmd_gate(args):
    rc = cmd_selftest(args)
    if rc != EXIT_PASS:
        die("gate: самоконтроль харнесса не прошёл — гейт не может судить "
            "(BLOCKED, не FAIL)", EXIT_BLOCKED_PROBE)
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
