#!/usr/bin/env python3
"""Юнит-прогон чистого ядра критерия «содержимое окна» (R6 задачи 01a1acec).

Покрыты side-effect-free функции ui_content.py на фикстурных дампах
qa-samples + синтетических входах: ядро порогов evaluate_pair, классификация
неуспеха съёмки classify_capture, fail-closed валидация ожиданий
validate_expectations. Граница самой съёмки (AT-SPI/scrot) не мок-ается
осознанно — регресс R1 (snap без окна → rc=2 BLOCKED) проверяется живым
субпроцессом на заведомо несуществующем окне, без mock'ов.

Прогон: /usr/bin/python3 -m unittest test_ui_content -v  (каталог tools/).
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ui_content as uc  # noqa: E402

TOOLS_DIR = os.path.dirname(os.path.abspath(__file__))
CRATE_DIR = os.path.dirname(TOOLS_DIR)
SAMPLES = os.path.join(CRATE_DIR, "qa-samples")
SCRIPT = os.path.join(TOOLS_DIR, "ui_content.py")


def expectations():
    with open(os.path.join(TOOLS_DIR, "ui_expectations.json"),
              encoding="utf-8") as fh:
        return json.load(fh)


def ax_dump(name):
    with open(os.path.join(SAMPLES, name, "axtree.txt"),
              encoding="utf-8") as fh:
        return fh.read()


VALID_SHOT = {"exists": True, "size": 248273, "magic_ok": True}
NO_SHOT = {"exists": False, "size": 0, "magic_ok": False}


class TestEvaluatePair(unittest.TestCase):
    """Ядро порогов на фикстурных дампах (R6)."""

    def test_known_good_no_violations(self):
        self.assertEqual(
            uc.evaluate_pair(ax_dump("known-good"), VALID_SHOT, expectations()),
            [])

    def test_edge_sparse_passes_distinguishably_from_known_bad(self):
        # R3: бедный живой UI (19 узлов) проходит порог и отличается от bad.
        edge = uc.evaluate_pair(ax_dump("edge-sparse"), VALID_SHOT,
                                expectations())
        bad = uc.evaluate_pair(ax_dump("known-bad"), VALID_SHOT, expectations())
        self.assertEqual(edge, [])
        self.assertTrue(bad)

    def test_known_bad_flags_error_text_nodes_and_nav(self):
        violations = uc.evaluate_pair(ax_dump("known-bad"), VALID_SHOT,
                                      expectations())
        joined = " ".join(violations)
        self.assertIn("error-page текст", joined)
        self.assertIn("AX-дерево не заполнено", joined)
        self.assertIn("навигационный элемент отсутствует", joined)

    def test_missing_ax_and_shot_reported(self):
        violations = uc.evaluate_pair(None, NO_SHOT, expectations())
        joined = " ".join(violations)
        self.assertIn(f"отсутствует {uc.AXTREE}", joined)
        self.assertIn(f"отсутствует {uc.SHOT}", joined)

    def test_corrupt_shot_reported(self):
        tiny = {"exists": True, "size": 42, "magic_ok": False}
        joined = " ".join(uc.evaluate_pair(ax_dump("known-good"), tiny,
                                           expectations()))
        self.assertIn("битый/тривиальный", joined)

    def test_node_count_ignores_headers(self):
        self.assertEqual(uc._node_count(ax_dump("edge-sparse")), 19)


class TestClassifyCapture(unittest.TestCase):
    """Чистая классификация неуспеха съёмки (R6): blocked/invalid/ok."""

    def test_blocked_when_nothing_captured(self):
        kind, why = uc.classify_capture(False, NO_SHOT)
        self.assertEqual(kind, "blocked")
        self.assertIn("BLOCKED", why)

    def test_blocked_when_tool_returned_without_file(self):
        kind, _ = uc.classify_capture(True, NO_SHOT)
        self.assertEqual(kind, "blocked")

    def test_invalid_when_corrupt(self):
        kind, why = uc.classify_capture(True, {"exists": True, "size": 10,
                                               "magic_ok": False})
        self.assertEqual(kind, "invalid")
        self.assertIn("invalid", why)

    def test_ok_returns_size(self):
        self.assertEqual(uc.classify_capture(True, VALID_SHOT),
                         ("ok", 248273))


class TestValidateExpectations(unittest.TestCase):
    """Fail-closed загрузка ожиданий (R2): потеря ключа — ошибка, не дефолт."""

    def test_repo_expectations_valid(self):
        self.assertEqual(uc.validate_expectations(expectations()), [])

    def test_missing_each_required_key_is_loud(self):
        base = expectations()
        for key in uc.REQUIRED_EXPECT_KEYS:
            raw = dict(base)
            del raw[key]
            errors = uc.validate_expectations(raw)
            self.assertTrue(errors, f"ключ {key} потерярован молча")
            self.assertIn(key, " ".join(errors))

    def test_min_ax_nodes_type_enforced(self):
        raw = expectations()
        raw["min_ax_nodes"] = "12"
        self.assertTrue(uc.validate_expectations(raw))

    def test_required_elements_not_empty_enforced(self):
        raw = expectations()
        raw["required_elements"] = []
        self.assertTrue(uc.validate_expectations(raw))
        raw["required_elements"] = [{"id": "x", "any_of": []}]
        self.assertTrue(uc.validate_expectations(raw))


class TestSnapNoWindowBlocked(unittest.TestCase):
    """Регресс R1: snap без живого окна → rc=2 (BLOCKED), текст без обмана."""

    def run_snap(self, python):
        with tempfile.TemporaryDirectory() as td:
            res = subprocess.run(
                [python, SCRIPT, "snap", "--out", os.path.join(td, "x"),
                 "--window", "no-such-window-01a1acec-zzz", "--pid", "4294967"],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=60,
                env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
            return res

    def test_snap_without_window_exits_blocked_not_fail(self):
        for python in (sys.executable, "/usr/bin/python3"):
            if not os.path.exists(python):
                continue
            res = self.run_snap(python)
            out = res.stdout.decode()
            self.assertEqual(res.returncode, uc.EXIT_BLOCKED,
                             f"{python}: snap без окна обязан давать rc=2, "
                             f"получено {res.returncode}: {out!r}")
            self.assertNotEqual(res.returncode, uc.EXIT_FAIL)
            self.assertIn("BLOCKED", out)


if __name__ == "__main__":
    unittest.main(verbosity=2)
