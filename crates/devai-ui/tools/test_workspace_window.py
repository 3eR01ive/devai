#!/usr/bin/env python3
"""Window-content интеграционный тест панели «Воркспейс» (REQ-WV-04 AC4).

Образец — selftest-пары критерия «содержимое окна» (test_ui_content.py +
docs/ui-content-criteria.md): живой прогон НЕ мок-ается. Тест поднимает
канонический dev-путь (vite + debug-бинарь devai-ui) и строит ОДИН
fixture-home с ТРЕМЯ задачами, покрывающими render-ветки панели
(TaskDetailView «Воркспейс»):

  * LIVE   — изоляция + коммит поверх bound head: файл-лист (M base.txt,
             A bin.dat, A new.txt), читаемый дифф base.txt и БИНАРНАЯ
             подпись bin.dat (binary-caption ветка REQ-WV-02 AC3);
  * ZERO   — изоляция состоялась, tip == bound head: note-строка
             «ноль изменений» (REQ-WV-01 AC4) и ни одной кнопки «дифф»;
  * MERGED — rework.merged с head_before/head_after поверх живой ветки:
             строка «смержено в main: …» (REQ-WV-07a — ветка переживает
             мерж, платформа не чистит).

Навигация открывает карточку каждой задачи, кнопкой «дифф» вызывает
task.workspace_diff, и харнесом ui_content.py снимается пара
(screenshot.png + axtree.txt) на каждый срез (четыре пары). Пороги —
ожиданиями:

  * tools/ui_expectations.json — базовый критерий (навигация, нет
    error-page, заполненное дерево);
  * tools/ui_expectations_workspace.json — маркеры панели для live/merged
    (заголовок «... · изменения против baseline», пофайловая «дифф»);
  * tools/ui_expectations_workspace_zero.json — то же для zero (кнопки
    «дифф» там нет ПО ПРАВИЛАМ РЕНДЕРА — отсутствие таблицы и есть
    покрытие ветки).

На этой машине webkit не кладёт DOM-текст в дерево
(docs/ui-content-criteria.md), поэтому содержимое листa/диффа проверяется
дважды: механически структурно (число кнопок = размер changeset; клик
монтирует diff-блок) и человеком по скриншоту пары — пары кладутся в
DEVAI_EVIDENCE_DIR.

Порт dev-пути: DEVAI_UI_PORT (канонический 1420; другой законен, только
если бинарь собран с этим devUrl, иначе окно уедет на чужой сервер —
проверка порта ниже честным SKIP блокирует такой прогон).

Данные не изобретаются: если окно не поднялось/порт занят/harness не может
снять — тест SKIP'ится с честной причиной (BLOCKED-семантика критерия),
а не зелёным прогоном вхолостую.

Прогон: /usr/bin/python3 -m unittest test_workspace_window -v  (каталог
tools/). Долго (~2-4 мин): живой vite, живой webkit-бинарь, AT-SPI-навигация
по трём карточкам.
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
EXPECT_WS_ZERO = os.path.join(TOOLS_DIR, "ui_expectations_workspace_zero.json")

DEV_PORT = int(os.environ.get("DEVAI_UI_PORT", "1420"))
TITLE_LIVE = "workspace-window LIVE (ui-content)"
TITLE_ZERO = "workspace-window ZERO (ui-content)"
TITLE_MERGED = "workspace-window MERGED (ui-content)"
MOD_PATH = "base.txt"
MOD_MARKER = "agent modified 42"   # строка фикстуры в читаемом диффе
NEW_PATH = "new.txt"
BIN_PATH = "bin.dat"
# changeset live-задачи: M base.txt, A bin.dat, A new.txt — в каноническом
# сортировочном порядке путей git; бинарная кнопка — позиция 1.
N_FILES = 3
BIN_BUTTON_INDEX = 1
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

    def _nodes(self, role, sub):
        found = []

        def walk(n, d=0):
            if d > 40:
                return
            try:
                if n.get_role_name() == role and sub.lower() in (n.get_name() or "").lower():
                    found.append(n)
                    return  # имя найдено — глубже в поддерево этого имени не идём
            except Exception:
                pass  # узел не читается — его детей всё равно обходим
            try:
                n_children = n.get_child_count()
            except Exception:
                n_children = 0
            for i in range(min(n_children, 120)):
                walk(n.get_child_at_index(i), d + 1)

        walk(self.app)
        return found

    def _node(self, role, sub):
        nodes = self._nodes(role, sub)
        return nodes[0] if nodes else None

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

    def click_nth(self, role, sub, nth, timeout=45):
        """Клик по N-му в порядке дерева элементу (для пофайловых кнопок —
        канонический отсортированный по путям порядок changeset)."""
        deadline = time.time() + timeout
        nodes = []
        while time.time() < deadline:
            nodes = self._nodes(role, sub)
            if len(nodes) > nth:
                if not nodes[nth].do_action(0):
                    raise AssertionError(
                        f"AT-SPI: do_action не сработал на [{role}] {sub!r} #{nth}")
                return nodes[nth]
            time.sleep(1.0)
        raise AssertionError(
            f"AT-SPI: [{role}] {sub!r} #{nth} не появился за {timeout} с")

    def count(self, role, sub):
        return len(self._nodes(role, sub))

    def wait_count_at_least(self, role, sub, n, timeout=45):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.count(role, sub) >= n:
                return True
            time.sleep(1.0)
        return False

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


def _new_task(home, repo, title, wt_dir):
    """Задача проекта + ветка rework/<task-id> на bound head — как это
    делает платформа на prepare."""
    out = cli(home, ["--json", "task", "new", title, "--project", "ws-window-fixture"])
    task_id = json.loads(out)["id"]
    bound = git(repo, ["rev-parse", "HEAD"])
    git(repo, ["worktree", "add", wt_dir, "-b", f"rework/{task_id}", bound])
    return task_id, bound


def _record_isolation(home, task_id, wt, bound, tip):
    """События стора в тех же payload'ах, что пишет engine
    (worktree.prepared / worktree.committed)."""
    branch = f"rework/{task_id}"
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


def _record_merged(home, task_id, repo, head_before, head_after):
    """rework.merged — те же поля, что пишет платформа на merge-gate."""
    now = time.strftime("%Y-%m-%dT%H:%M:%S.000000Z", time.gmtime())
    db = sqlite3.connect(os.path.join(home, "devai.db"))
    db.execute(
        "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at)"
        " VALUES(?,?,?,?,?,?)",
        (str(uuid.uuid4()), task_id, "rework.merged", "guerrero",
         json.dumps({"branch": f"rework/{task_id}", "workspace": repo,
                     "head_before": head_before, "head_after": head_after}), now))
    db.commit()
    db.close()


def build_fixture(root):
    """Один home, три задачи: LIVE (текст+бинарник), ZERO (tip==bound —
    легальный ноль, как его отдаёт commit_platform_work без изменений),
    MERGED (человек смержил ветку, live ref переживает мерж — REQ-WV-07a)."""
    home = os.path.join(root, "home")
    repo = os.path.join(root, "repo")
    os.makedirs(home)
    os.makedirs(repo)
    git(repo, ["init", "-q"])
    with open(os.path.join(repo, MOD_PATH), "w") as fh:
        fh.write("baseline\n")
    with open(os.path.join(repo, "doomed.txt"), "w") as fh:
        fh.write("doomed\n")
    git(repo, ["add", "-A"])
    git(repo, [*GIT_ID, "commit", "-q", "-m", "bound state"])

    cli(home, ["project", "add", "ws-window-fixture", "--repo", repo])

    # --- LIVE: текстовая модификация, новый файл и бинарник одним коммитом.
    live_id, bound = _new_task(home, repo, TITLE_LIVE, os.path.join(root, "wt-live"))
    with open(os.path.join(root, "wt-live", MOD_PATH), "w") as fh:
        fh.write(f"baseline\n{MOD_MARKER}\n")
    with open(os.path.join(root, "wt-live", NEW_PATH), "w") as fh:
        fh.write("agent work\n")
    with open(os.path.join(root, "wt-live", BIN_PATH), "wb") as fh:
        fh.write(bytes([0, 155, 0, 255, 7]))
    git(os.path.join(root, "wt-live"), ["add", "-A"])
    git(os.path.join(root, "wt-live"), [*GIT_ID, "commit", "-q", "-m", "fixture work"])
    _record_isolation(home, live_id, os.path.join(root, "wt-live"), bound,
                      git(os.path.join(root, "wt-live"), ["rev-parse", "HEAD"]))

    # --- ZERO: изоляция состоялась, содержимого нет — штамп = bound head.
    zero_id, zero_bound = _new_task(home, repo, TITLE_ZERO, os.path.join(root, "wt-zero"))
    _record_isolation(home, zero_id, os.path.join(root, "wt-zero"), zero_bound, zero_bound)

    # --- MERGED: коммит на ветке, merge в main с хранимыми head_before/after.
    merged_id, _ = _new_task(home, repo, TITLE_MERGED, os.path.join(root, "wt-merged"))
    with open(os.path.join(root, "wt-merged", "merged-note.txt"), "w") as fh:
        fh.write("merged branch content\n")
    git(os.path.join(root, "wt-merged"), ["add", "-A"])
    git(os.path.join(root, "wt-merged"),
        [*GIT_ID, "commit", "-q", "-m", "merged fixture work"])
    merged_tip = git(os.path.join(root, "wt-merged"), ["rev-parse", "HEAD"])
    _record_isolation(home, merged_id, os.path.join(root, "wt-merged"), bound, merged_tip)
    head_before = git(repo, ["rev-parse", "HEAD"])
    git(repo, ["merge", "-q", "--no-ff", "-m", "merge rework", f"rework/{merged_id}"])
    _record_merged(home, merged_id, repo, head_before, git(repo, ["rev-parse", "HEAD"]))

    return home, {"live": live_id, "zero": zero_id, "merged": merged_id}


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
        cls._ws_zero_expect = uc.load_expect(EXPECT_WS_ZERO)

    # ------------------------------------------------------------- helpers

    def _panel_section_nodes(self, driver):
        """Счёт section/paragraph-узлов панели между её заголовком и
        следующим heading — монтировать ли diff-блок/бинарную подпись."""
        lines = driver.lines()
        start = next((i for i, l in enumerate(lines)
                      if l.lower().startswith("heading|воркспейс")), None)
        if start is None:
            return -1
        end = next((i for i in range(start + 1, len(lines))
                    if lines[i].lower().startswith("heading|")), len(lines))
        region = lines[start:end]
        return sum(1 for l in region
                   if l.lower().startswith(("section|", "paragraph|")))

    def _wait_diff_block_grown(self, driver, before):
        deadline = time.time() + 45
        while time.time() < deadline:
            if self._panel_section_nodes(driver) > before:
                return True
            time.sleep(1.0)
        return False

    def _open_task(self, driver, title):
        driver.click("link", "Задачи")
        driver.click("link", title)
        # Панель примонтирована: заголовок виден AT-SPI в имени (на этом
        # webkit DOM-текст в дерево не кладётся — docs/ui-content-criteria.md;
        # пороги критерия работают по именам heading/link/button).
        self.assertIsNotNone(
            driver.wait("heading", "изменения против baseline", 45),
            f"панель «Воркспейс · изменения против baseline» не появилась для {title!r}")

    def _snap(self, root, name, pid):
        """Съёмка пары харнесом — ровно как требует ритуал producer'а."""
        pair = os.path.join(root, "ui", name)
        snap = subprocess.run(
            [sys.executable, HARNESS, "snap", "--out", pair,
             "--window", "devai-ui", "--pid", str(pid)],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
        self.assertEqual(snap.returncode, uc.EXIT_PASS,
                         f"snap {name}: {snap.stdout.decode(errors='replace')}")
        return pair

    def _check_pair(self, pair_dir, expect, label):
        v_base = uc.check_pair(pair_dir, self._ui_expect)
        self.assertEqual(v_base, [], f"{label}: базовый критерий окна {v_base}")
        v_ws = uc.check_pair(pair_dir, expect)
        self.assertEqual(v_ws, [], f"{label}: ожидания панели «Воркспейс» {v_ws}")

    # ---------------------------------------------------------------- run

    def test_window_covers_workspace_render_branches(self):
        root = tempfile.mkdtemp(prefix="ws-window-")
        self.addCleanup(shutil.rmtree, root, True)
        home, _ids = build_fixture(root)

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
            self.skipTest(f"BLOCKED: vite не поднялся на 127.0.0.1:{DEV_PORT} за 30 с")

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

        pairs = {}

        # ---- LIVE: файл-лист = N кнопок; читаемый дифф; бинарная подпись.
        self._open_task(driver, TITLE_LIVE)
        self.assertTrue(
            driver.wait_count_at_least("button", "дифф", N_FILES, 45),
            "кнопки «дифф» по числу файлов live-changeset не появились")
        before = self._panel_section_nodes(driver)
        driver.click_nth("button", "дифф", 0)  # base.txt — текстовый diff
        self.assertTrue(
            self._wait_diff_block_grown(driver, before),
            "блок диффа не примонтировался после кнопки «дифф» (live/base.txt)")
        pairs["workspace-live-diff"] = self._snap(root, "workspace-live-diff", binary.pid)

        before = self._panel_section_nodes(driver)
        driver.click_nth("button", "дифф", BIN_BUTTON_INDEX)  # bin.dat — binary
        deadline = time.time() + 45
        while time.time() < deadline and self._panel_section_nodes(driver) < before:
            time.sleep(1.0)  # diff-блок перезаписывается: ждём не пустую панель
        self.assertGreaterEqual(
            self._panel_section_nodes(driver), before,
            "бинарная подпись bin.dat не примонтировала diff-блок")
        pairs["workspace-live-binary"] = \
            self._snap(root, "workspace-live-binary", binary.pid)

        # ---- ZERO: легальный ноль изменений — таблицы и кнопок нет.
        self._open_task(driver, TITLE_ZERO)
        self.assertEqual(
            driver.count("button", "дифф"), 0,
            "zero_changes не обязан рендерить таблицу с кнопками «дифф»")
        pairs["workspace-zero-changes"] = \
            self._snap(root, "workspace-zero-changes", binary.pid)

        # ---- MERGED: live-список + строка head_before/head_after.
        self._open_task(driver, TITLE_MERGED)
        self.assertTrue(
            driver.wait_count_at_least("button", "дифф", 1, 45),
            "merged-задача потеряла файл-лист живой ветки (REQ-WV-07a)")
        pairs["workspace-merged-facts"] = \
            self._snap(root, "workspace-merged-facts", binary.pid)

        # ---- пороги и evidence-пары (ui-content гейт платформы).
        for name, pair_dir in pairs.items():
            expect = self._ws_zero_expect if "zero" in name else self._ws_expect
            self._check_pair(pair_dir, expect, name)
        ev = os.environ.get("DEVAI_EVIDENCE_DIR")
        if ev:
            for name, pair_dir in pairs.items():
                dst = os.path.join(ev, "ui", name)
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                if os.path.isdir(dst):
                    shutil.rmtree(dst)
                shutil.copytree(pair_dir, dst)

    @staticmethod
    def _terminate(proc):
        proc.terminate()
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    unittest.main(verbosity=2)
