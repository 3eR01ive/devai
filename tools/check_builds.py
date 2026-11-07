#!/usr/bin/env python3
"""Generic "significant builds" gate for the base quality profile.

Проект объявляет свои сборки ДАННЫМИ (config/workflow/builds.yaml), а не кодом
профиля: у одного проекта это cargo, у другого cargo+frontend, у третьего
pandoc/typst. Гейт ничего не знает про язык — он проходит по списку и требует
exit 0 от каждой значимой сборки.

Формат config/workflow/builds.yaml (merge-scan, как остальные policy-файлы):

    builds:
      - name: rust
        cmd: cargo build --workspace
        cwd: .            # необязательно, относительно корня проекта
      - name: frontend
        cmd: npm ci --silent && node node_modules/vite/bin/vite.js build
        cwd: crates/devai-ui/frontend

Коды выхода: 0 — все сборки прошли; 1 — хотя бы одна упала (печатается, какая);
2 — сборки не объявлены вовсе (BLOCKED: «не по чему оценивать», не FAIL).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys

CONFIG = os.path.join("config", "workflow", "builds.yaml")


def load_builds(path: str) -> list[dict]:
    """Минимальный парсер нужного подмножества YAML (stdlib-only окружение)."""
    if not os.path.exists(path):
        return []
    builds: list[dict] = []
    cur: dict | None = None
    in_builds = False
    for raw in open(path, encoding="utf-8"):
        line = raw.rstrip("\n")
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped == "builds:":
            in_builds = True
            continue
        if not in_builds:
            continue
        if stripped.startswith("- "):
            if cur:
                builds.append(cur)
            cur = {}
            stripped = stripped[2:].strip()
        if cur is None:
            continue
        if ":" in stripped:
            key, _, value = stripped.partition(":")
            key, value = key.strip(), value.strip()
            if value.startswith(("'", '"')) and value.endswith(("'", '"')):
                value = value[1:-1]
            if key in {"name", "cmd", "cwd"} and value:
                cur[key] = value
    if cur:
        builds.append(cur)
    return [b for b in builds if b.get("cmd")]


def main() -> int:
    builds = load_builds(CONFIG)
    if not builds:
        print(f"BLOCKED: {CONFIG} не объявляет ни одной сборки — гейт не по чему применять")
        return 2

    failures = []
    for b in builds:
        name = b.get("name") or b["cmd"][:40]
        cwd = b.get("cwd") or "."
        print(f"== build: {name} (cwd={cwd})")
        proc = subprocess.run(b["cmd"], shell=True, cwd=cwd, capture_output=True, text=True)
        tail = (proc.stdout or "")[-2000:] + (proc.stderr or "")[-2000:]
        if proc.returncode != 0:
            failures.append(name)
            print(f"FAIL {name} rc={proc.returncode}")
            print(tail.strip()[-1200:])
        else:
            print(f"ok   {name}")

    if failures:
        print("\nСБОРКИ НЕ ПРОШЛИ: " + ", ".join(failures))
        return 1
    print(f"\nвсе значимые сборки прошли ({len(builds)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
