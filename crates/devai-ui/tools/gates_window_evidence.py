#!/usr/bin/env python3
"""Window-evidence прогон секции ожидания человека (задача 01a1b0ea).

Поднимает канонический dev-путь (vite на 127.0.0.1:1420 + debug-бинарь
devai-ui, тот же, что снимают образцы ui-content), открывает вкладку
«Ожидание» (#/inbox) и снимает пару screenshot.png + axtree.txt харнесом
критерия «содержимое окна» (ui_content.py snap). Ожидания — базовый
ui_expectations.json поверхности (навигация, отсутствие error-page,
min_ax_nodes).

Данные не изобретаются: дом берётся из DEVAI_HOME (или --home) как есть;
если в нём нет ни одной задачи на гейте платформы (APPROVAL /
WAITING_FOR_INPUT / QA / FAILED) и ни одной остановленной оператором
(PAUSED / BLOCKED), прогон честно SKIP'ится как BLOCKED — пустой экран
ничего не доказывает.

Пара кладётся в <DEVAI_EVIDENCE_DIR>/ui/gates-section/ (producer-ритуал).

Запуск:
  DEVAI_EVIDENCE_DIR=... DEVAI_HOME=~/.devai \\
    /usr/bin/python3 gates_window_evidence.py
"""

import argparse
import glob
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ui_content as uc  # noqa: E402

TOOLS_DIR = os.path.dirname(os.path.abspath(__file__))
CRATE_DIR = os.path.dirname(TOOLS_DIR)
WORKSPACE_ROOT = os.path.dirname(os.path.dirname(CRATE_DIR))
BIN_UI = os.path.join(WORKSPACE_ROOT, "target", "debug", "devai-ui")
BIN_CLI = os.path.join(WORKSPACE_ROOT, "target", "debug", "devai")
FRONTEND = os.path.join(CRATE_DIR, "frontend")
DEV_PORT = int(os.environ.get("DEVAI_UI_PORT", "1420"))
HARNESS = os.path.join(TOOLS_DIR, "ui_content.py")
EXPECT = os.path.join(TOOLS_DIR, "ui_expectations.json")


def config_dir(home):
    """The home's config directory, resolved like the engine (REQ-HP-04)."""
    return os.environ.get("DEVAI_CONFIG") or os.path.join(home, "config")


def wait_states(home):
    """The state tokens of the wait section, read from the DEMAND TABLE.

    The table is platform data (`config/workflow/inbox-waits.yaml`, loaded by
    `devai_store::config::load_inbox_wait_policy` with the merge-scan idiom:
    files without the `inbox_waits:` key are inert, the LAST section wins).
    The tool keeps no copy of the state set: a state
    that the table assigns to a group other than `none` is a wait state.
    """
    try:
        import yaml
    except ImportError as e:  # fail loudly — a silently empty set would SKIP
        raise RuntimeError(f"нужен PyYAML для чтения таблицы ожиданий: {e}")
    states = {}
    root = config_dir(home)
    wf = os.path.join(root, "workflow")
    paths = sorted(glob.glob(os.path.join(wf, "*.yaml")) + glob.glob(os.path.join(wf, "*.yml")))
    for path in paths:
        with open(path, encoding="utf-8") as fh:
            doc = yaml.safe_load(fh) or {}
        if isinstance(doc, dict) and "inbox_waits" in doc:
            states = (doc["inbox_waits"] or {}).get("states") or {}
    if not states:
        # Factory table: the same bytes the loader embeds.
        with open(os.path.join(WORKSPACE_ROOT, "config", "workflow", "inbox-waits.yaml"),
                  encoding="utf-8") as fh:
            states = (yaml.safe_load(fh) or {}).get("inbox_waits", {}).get("states", {})
    return {
        token
        for token, row in states.items()
        if (row or {}).get("group", "none") != "none"
    }


def port_open(host, port):
    with socket.socket() as s:
        s.settimeout(1.0)
        return s.connect_ex((host, port)) == 0


def waiting_states(home):
    env = {**os.environ, "DEVAI_HOME": home}
    res = subprocess.run([BIN_CLI, "task", "list", "--json"],
                         env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                         timeout=120)
    if res.returncode != 0:
        raise RuntimeError(res.stdout.decode(errors="replace"))
    tasks = json.loads(res.stdout.decode())
    waits = wait_states(home)
    return sorted({t["state"] for t in tasks if t["state"] in waits})


class AtspiDriver:
    def __init__(self, pid):
        import gi
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
        self.A = Atspi
        self.app = self._find_app(pid)

    def _find_app(self, pid):
        desk = self.A.get_desktop(0)
        for i in range(desk.get_child_count()):
            a = desk.get_child_at_index(i)
            try:
                if a.get_process_id() == pid:
                    return a
            except Exception:
                continue
        raise AssertionError(f"AT-SPI: приложение pid={pid} не зарегистрировано")

    def _nodes(self, role, sub):
        found = []

        def walk(n, d=0):
            if d > 40:
                return
            try:
                if n.get_role_name() == role and sub.lower() in (n.get_name() or "").lower():
                    found.append(n)
                    return
            except Exception:
                pass
            try:
                count = n.get_child_count()
            except Exception:
                count = 0
            for i in range(min(count, 120)):
                walk(n.get_child_at_index(i), d + 1)

        walk(self.app)
        return found

    def click(self, role, sub, timeout=45):
        deadline = time.time() + timeout
        while time.time() < deadline:
            nodes = self._nodes(role, sub)
            if nodes:
                if not nodes[0].do_action(0):
                    raise AssertionError(f"AT-SPI: do_action не сработал на [{role}] {sub!r}")
                return
            time.sleep(1.0)
        raise AssertionError(f"AT-SPI: элемент [{role}] {sub!r} не найден")

    def count(self, role, sub):
        return len(self._nodes(role, sub))


def terminate(proc):
    proc.terminate()
    try:
        proc.wait(10)
    except subprocess.TimeoutExpired:
        proc.kill()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--home", default=os.environ.get("DEVAI_HOME"))
    ap.add_argument("--out", default=None, help="каталог пары (по умолчанию temp)")
    args = ap.parse_args()
    home = os.path.expanduser(args.home) if args.home else os.path.expanduser("~/.devai")
    if not os.path.isfile(os.path.join(home, "devai.db")):
        print(f"BLOCKED: в доме {home} нет devai.db", file=sys.stderr)
        return 2
    states = waiting_states(home)
    if not states:
        print(f"BLOCKED: в доме {home} нет ни одной ожидающей задачи — секции нечего показать",
              file=sys.stderr)
        return 2
    print(f"home={home} waiting-state-tokens={states}")

    vite_bin = os.path.join(FRONTEND, "node_modules", ".bin", "vite")
    vite = subprocess.Popen([vite_bin, "--host", "127.0.0.1", "--port", str(DEV_PORT),
                             "--strictPort"],
                            cwd=FRONTEND, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    binary = None
    try:
        for _ in range(60):
            if port_open("127.0.0.1", DEV_PORT):
                break
            time.sleep(0.5)
        else:
            print("BLOCKED: vite не поднялся за 30 с", file=sys.stderr)
            return 2

        binary = subprocess.Popen([BIN_UI], env={**os.environ, "DEVAI_HOME": home,
                                                 "DEVAI_USER": "gates-window-evidence"})
        driver = None
        for _ in range(45):
            time.sleep(1.0)
            try:
                driver = AtspiDriver(binary.pid)
                break
            except Exception:
                continue
        if driver is None:
            print("BLOCKED: окно devai-ui не зарегистрировалось в AT-SPI", file=sys.stderr)
            return 2

        # Вкладка ожидания — адрес секции, он же навигационный токен.
        driver.click("link", "Ожидание", timeout=60)
        time.sleep(4.0)  # первый load + поллинг
        headings = driver.count("heading", "")
        toggles = driver.count("button", "")
        print(f"ax: headings={headings} buttons={toggles}")

        root = args.out or tempfile.mkdtemp(prefix="gates-window-")
        pair = os.path.join(root, "ui", "gates-section")
        snap = subprocess.run([sys.executable, HARNESS, "snap", "--out", pair,
                               "--window", "devai-ui", "--pid", str(binary.pid)],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
        if snap.returncode != uc.EXIT_PASS:
            print("BLOCKED: snap: " + snap.stdout.decode(errors="replace"), file=sys.stderr)
            return 2

        expect = json.load(open(EXPECT))
        verdict = uc.check_pair(pair, expect)
        if verdict:
            print("FAIL: " + "; ".join(verdict), file=sys.stderr)
            return 1

        ev = os.environ.get("DEVAI_EVIDENCE_DIR")
        if ev:
            dst = os.path.join(ev, "ui", "gates-section")
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            if os.path.isdir(dst):
                shutil.rmtree(dst)
            shutil.copytree(pair, dst)
            print(f"evidence: {dst}")
        print(f"PASS: {pair}")
        return 0
    finally:
        if binary is not None:
            terminate(binary)
        terminate(vite)


if __name__ == "__main__":
    sys.exit(main())
