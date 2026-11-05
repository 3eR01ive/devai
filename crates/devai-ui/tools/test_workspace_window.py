#!/usr/bin/env python3
"""Window-content интеграционный тест панели «Воркспейс» (REQ-WV-04 AC4).

Образец — selftest-пары критерия «содержимое окна» (test_ui_content.py +
docs/ui-content-criteria.md): живой прогон НЕ мок-ается. Тест поднимает
канонический dev-путь (vite 127.0.0.1:1420 + debug-бинарь devai-ui),
строит настоящую фикстуру изоляции (git-репозиторий + worktree + ветка
rework/<task-id> с коммитом поверх bound head + хранимые события
worktree.prepared/committed в отдельном DEVAI_HOME), переходом по навигации
открывает карточку задачи, кнопкой «дифф» вызывает task.workspace_diff —
и харнесом ui_content.py снимает пару (screenshot.png + axtree.txt) с живого
окна. Пороги проверяются ДВУМЯ ожиданиями:

  * tools/ui_expectations.json — базовый критерий (навигация, нет
    error-page, заполненное дерево);
  * tools/ui_expectations_workspace.json — отдельное ожидание этой
    поверхности по форме docs: механические маркеры панели на уровне
    имён AX (заголовок «... · изменения против baseline», пофайловая
    кнопка «дифф»). На этой машине webkit не кладёт DOM-текст в дерево
    (docs/ui-content-criteria.md), поэтому содержимое листa/диффа
    проверяется структурно в самом тесте (число кнопок = размер
    changeset фикстуры; клик монтирует diff-блок) и человеком по
    скриншоту пары.

Данные не изобретаются: если окно не поднялось/порт занят/harness не может
снять — тест SKIP'ится с честной причиной (BLOCKED-семантика критерия),
а не зелёным прогоном вхолостую.

Прогон: /usr/bin/python3 -m unittest test_workspace_window -v  (каталог
tools/). Долго (~1-2 мин): живой vite, живой webkit-бинарь, AT-SPI-навигация.
"""

import json
import os
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ui_content as uc  # noqa: E402

TOOLS_DIR = os.path.dirname(os.path.abspath(__file__))
CRATE_DIR = os.path.dirname(TOOLS_DIR)            # crates/devai-ui
WORKSPACE_ROOT = os.path.dirname(os.path.dirname(CRATE_DIR))  # repo root
BIN_UI = os.path.join(WORKSPACE_ROOT, "target", "debug", "devai-ui")
BIN_CLI = os.path.join(WORKSPACE_ROOT, "target", "debug", "devai")
FRONTEND = os.path.join(CRATE_DIR, "frontend")
VITE_BIN = os.path.join(FRONTEND, "node_modules", ".bin", "vite")
HARNESS = os.path.join(TOOLS_DIR, "ui_content.py")
EXPECT_BASE = os.path.join(TOOLS_DIR, "ui_expectations.json")
EXPECT_WS = os.path.join(TOOLS_DIR, "ui_expectations_workspace.json")

DEV_PORT = 1420
TASK_TITLE = "workspace-window fixture (ui-content)"
MOD_PATH = "base.txt"
MOD_MARKER = "agent modified 42"   # строка фикстуры в диффе (видно на скриншоте)
NEW_PATH = "new.txt"
N_FILES = 2                        # M base.txt + A new.txt — ожидаемый changeset
GIT_ID = ["-c", "user.email=devai-test@local", "-c", "user.name=devai-test"]


def have(cmd):
    return shutil.which(cmd) is not None


def port_open(host, port):
    with socket.socket() as s:
        s.settimeout(1.0)
        return s.connect_ex((host, port)) == 0


def git(dir_, args, **kw):
    res = subprocess.run(["git", *args], cwd=dir_, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, timeout=60, **kw)
    if res.returncode != 0:
        raise RuntimeError(f"git {args}: {res.stdout.decode(errors='replace')}")
    return res.stdout.decode().strip()


def cli(home, args):
    env = {**os.environ, "DEVAI_HOME": home, "DEVAI_USER": "ws-window-test"}
    res = subprocess.run([BIN_CLI, *args], env=env, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, timeout=120)
    if res.returncode != 0:
        raise RuntimeError(f"devai {args}: {res.stdout.decode(errors='replace')}")
    return res.stdout.decode()


class AtspiDriver:
    """Минимальный драйвер окна через AT-SPI (роль link/button + do_action).

    Тот же интерфейс, что у харнеса съёмки: никакого git/путей — только
    наблюдаемое содержимое окна (догма REQ-WV-04 AC1 для теста-оператора)."""

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

    def _node(self, role, sub):
        found = []

        def walk(n, d=0):
            if found or d > 40:
                return
            try:
                if n.get_role_name() == role and sub.lower() in (n.get_name() or "").lower():
                    found.append(n)
                    return
            except Exception:
                pass  # узел не читается — его детей всё равно обходим
            try:
                n_children = n.get_child_count()
            except Exception:
                n_children = 0
            for i in range(min(n_children, 120)):
                walk(n.get_child_at_index(i), d + 1)

        walk(self.app)
        return found[0] if found else None

    def wait(self, role, sub, timeout=45):
        deadline = time.time() + timeout
        while time.time() < deadline:
            n = self._node(role, sub)
            if n:
                return n
            time.sleep(1.0)
        return None

    def click(self, role, sub, timeout=45):
        n = self.wait(role, sub, timeout)
        if n is None:
            raise AssertionError(f"AT-SPI: элемент [{role}] {sub!r} не найден в дереве")
        if not n.do_action(0):
            raise AssertionError(f"AT-SPI: do_action не сработал на [{role}] {sub!r}")
        return n

    def count(self, role, sub):
        found = []

        def walk(n, d=0):
            if d > 40:
                return
            try:
                if n.get_role_name() == role and sub.lower() in (n.get_name() or "").lower():
                    found.append(1)
            except Exception:
                pass
            try:
                n_children = n.get_child_count()
            except Exception:
                n_children = 0
            for i in range(min(n_children, 120)):
                walk(n.get_child_at_index(i), d + 1)

        walk(self.app)
        return len(found)

    def lines(self):
        """Дерево списком 'role|name' — структурная сигнатура окна."""
        out = []

        def walk(n, d=0):
            if d > 40 or len(out) > 4000:
                return
            try:
                out.append(f"{n.get_role_name()}|{n.get_name() or ''}")
            except Exception:
                return
            try:
                n_children = n.get_child_count()
            except Exception:
                n_children = 0
            for i in range(min(n_children, 120)):
                walk(n.get_child_at_index(i), d + 1)

        walk(self.app)
        return out


def build_fixture(root):
    """Реальная фикстура изоляции: repo + bound commit + worktree-ветка с
    коммитом + проект/задача через CLI + события стора (те же payload'ы,
    что пишет engine: worktree.prepared / worktree.committed)."""
    home = os.path.join(root, "home")
    repo = os.path.join(root, "repo")
    wt = os.path.join(root, "wt")
    os.makedirs(home)
    os.makedirs(repo)
    git(repo, ["init", "-q"])
    with open(os.path.join(repo, MOD_PATH), "w") as fh:
        fh.write("baseline\n")
    with open(os.path.join(repo, "doomed.txt"), "w") as fh:
        fh.write("doomed\n")
    git(repo, ["add", "-A"])
    git(repo, [*GIT_ID, "commit", "-q", "-m", "bound state"])
    bound = git(repo, ["rev-parse", "HEAD"])

    cli(home, ["project", "add", "ws-window-fixture", "--repo", repo])
    out = cli(home, ["--json", "task", "new", TASK_TITLE,
                     "--project", "ws-window-fixture"])
    task_id = json.loads(out)["id"]

    branch = f"rework/{task_id}"
    git(repo, ["worktree", "add", wt, "-b", branch, bound])
    with open(os.path.join(wt, MOD_PATH), "w") as fh:
        fh.write(f"baseline\n{MOD_MARKER}\n")
    with open(os.path.join(wt, NEW_PATH), "w") as fh:
        fh.write("agent work\n")
    git(wt, ["add", "-A"])
    git(wt, [*GIT_ID, "commit", "-q", "-m", "fixture work"])
    tip = git(wt, ["rev-parse", "HEAD"])

    exec_id = str(uuid.uuid4())
    now = time.strftime("%Y-%m-%dT%H:%M:%S.000000Z", time.gmtime())
    db = sqlite3.connect(os.path.join(home, "devai.db"))
    for kind, payload in (
        ("worktree.prepared", {"execution": exec_id, "branch": branch,
                               "worktree": wt, "bound_head": bound}),
        ("worktree.committed", {"execution": exec_id, "branch": branch,
                                "worktree": wt, "commit": tip}),
    ):
        db.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at)"
            " VALUES(?,?,?,?,?,?)",
            (str(uuid.uuid4()), task_id, kind, "implementation-agent",
             json.dumps(payload), now))
    db.commit()
    db.close()
    return home, task_id, bound, tip


@unittest.skipIf(not os.path.exists(BIN_UI),
                 f"BLOCKED: нет debug-бинаря {BIN_UI} — соберите cargo build -p devai-ui")
@unittest.skipIf(not os.path.exists(BIN_CLI),
                 f"BLOCKED: нет CLI {BIN_CLI} — соберите cargo build -p devai-cli")
@unittest.skipIf(not os.path.exists(VITE_BIN),
                 f"BLOCKED: нет {VITE_BIN} — npm install в crates/devai-ui/frontend")
@unittest.skipIf(not os.environ.get("DISPLAY"),
                 "BLOCKED: нет DISPLAY — окну webkit неоткуда быть")
@unittest.skipIf(not (have("xdotool") and (have("scrot") or have("import"))),
                 "BLOCKED: нет xdotool/scrot|import — съёмка окна невозможна")
class TestWorkspaceWindowContent(unittest.TestCase):

    @classmethod
    def setUpClass(cls):
        if port_open("127.0.0.1", DEV_PORT):
            raise unittest.SkipTest(
                f"BLOCKED: порт {DEV_PORT} занят живым dev-сервером — "
                "остановите чужой vite и повторите прогон")
        try:
            import gi  # noqa: F401
            gi.require_version("Atspi", "2.0")
        except Exception as exc:
            raise unittest.SkipTest(f"BLOCKED: gi/Atspi недоступен ({exc})")
        cls._ui_expect = uc.load_expect(EXPECT_BASE)
        cls._ws_expect = uc.load_expect(EXPECT_WS)

    def test_live_window_shows_changeset_and_readable_diff(self):
        root = tempfile.mkdtemp(prefix="ws-window-")
        self.addCleanup(shutil.rmtree, root, True)
        home, task_id, bound, tip = build_fixture(root)

        vite = subprocess.Popen(
            [VITE_BIN, "--host", "127.0.0.1", "--port", str(DEV_PORT),
             "--strictPort"],
            cwd=FRONTEND, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
        self.addCleanup(self._terminate, vite)
        for _ in range(60):
            if port_open("127.0.0.1", DEV_PORT):
                break
            time.sleep(0.5)
        else:
            self.skipTest("BLOCKED: vite не поднялся на 127.0.0.1:1420 за 30 с")

        binary = subprocess.Popen([BIN_UI],
                                  env={**os.environ, "DEVAI_HOME": home,
                                       "DEVAI_USER": "ws-window-test"})
        self.addCleanup(self._terminate, binary)
        driver = None
        for _ in range(40):
            time.sleep(1.0)
            try:
                driver = AtspiDriver(binary.pid)
                break
            except Exception:
                continue
        if driver is None:
            self.fail("BLOCKED: окно devai-ui не зарегистрировалось в AT-SPI")

        # Навигация к карточке задачи: только наблюдаемые элементы окна.
        driver.click("link", "Задачи")
        driver.click("link", TASK_TITLE)
        # Панель примонтирована: заголовок виден AT-SPI в имени (на этом
        # webkit DOM-текст в дерево не кладётся — docs/ui-content-criteria.md
        # «текст в дерево уже не кладёт»; прецедент: пороги критерия работают
        # по именам heading/link/button). Читаемая часть диффа — скриншот
        # пары; механическая — заголовок + пофайловые кнопки «дифф» +
        # структурный монтаж блока диффа по клику.
        self.assertIsNotNone(
            driver.wait("heading", "изменения против baseline", 45),
            "панель «Воркспейс · изменения против baseline» не появилась на живом окне")

        # Changeset непустой: по строке файла с кнопкой «дифф»
        # (table v-if="ws.files.length" — в no_isolation/zero_changes/
        # unavailable кнопок «дифф» на панели нет).
        deadline = time.time() + 45
        while time.time() < deadline and driver.count("button", "дифф") < N_FILES:
            time.sleep(1.0)
        self.assertGreaterEqual(
            driver.count("button", "дифф"), N_FILES,
            "кнопки «дифф» по числу файлов changeset не появились")

        # Пофайловый дифф по требованию (REQ-WV-02/05): клик по «дифф»
        # монтирует блок diff — внутри панели растёт структура (абзац
        # «diff: path · baseline..tip» + monospace-секция с телом).
        def panel_section_nodes():
            lines = driver.lines()
            start = next((i for i, l in enumerate(lines)
                          if l.lower().startswith("heading|воркспейс")), None)
            if start is None:
                return -1, len(lines)
            end = next((i for i in range(start + 1, len(lines))
                        if lines[i].lower().startswith("heading|")), len(lines))
            region = lines[start:end]
            return sum(1 for l in region if l.lower().startswith(("section|", "paragraph|"))), len(region)

        sect_before, _ = panel_section_nodes()
        driver.click("button", "дифф")
        grown = False
        deadline = time.time() + 45
        while time.time() < deadline:
            sect_now, _ = panel_section_nodes()
            if sect_now > sect_before:
                grown = True
                break
            time.sleep(1.0)
        self.assertTrue(grown, "блок диффа не примонтировался после кнопки «дифф»")

        # Съёмка пары харнесом — ровно как требует ритуал producer'а.
        pair = os.path.join(root, "ui", "workspace-panel-window")
        snap = subprocess.run(
            [sys.executable, HARNESS, "snap", "--out", pair,
             "--window", "devai-ui", "--pid", str(binary.pid)],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
        self.assertEqual(snap.returncode, uc.EXIT_PASS,
                         f"snap: {snap.stdout.decode(errors='replace')}")

        # Пороги базового критерия на карточке с панелью.
        v_base = uc.check_pair(pair, self._ui_expect)
        self.assertEqual(v_base, [], f"базовый критерий окна: {v_base}")
        # Отдельное ожидание панели — форма docs, обязательные элементы.
        v_ws = uc.check_pair(pair, self._ws_expect)
        self.assertEqual(v_ws, [], f"ожидания панели «Воркспейс»: {v_ws}")

        # Evidence-пара для платформенного гейта ui-content.
        ev = os.environ.get("DEVAI_EVIDENCE_DIR")
        if ev:
            dst = os.path.join(ev, "ui", "workspace-panel-window")
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            if os.path.isdir(dst):
                shutil.rmtree(dst)
            shutil.copytree(pair, dst)

    @staticmethod
    def _terminate(proc):
        proc.terminate()
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    unittest.main(verbosity=2)
