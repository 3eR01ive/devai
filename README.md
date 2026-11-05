# devai

Local-first мультиагентная платформа разработки с workflow-ядром: задачи живут в
декларативном цикле (DRAFT → ANALYSIS → REVIEW → APPROVAL → EXECUTE → QA → ACCEPTED),
роли-агенты исполняют стадии, оценка кода — декларативный профиль с machine-гейтами.

## Сборка и тесты

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
```

Rust stable, edition 2021, без внешних сетевых зависимостей в product-коде
(субпроцесс-адаптеры запускают локальные CLI-агентов).

## CLI

Бинарь `devai` (debug-сборка — `target/debug/devai`). Данные — в `$DEVAI_HOME`
(по умолчанию `~/.devai`): SQLite-стор задач/событий/артефактов, worktree-и, конфиг.

```sh
devai init                       # засеять конфиг/роли/профили в home
devai task new "<название>" --description-file d.txt   # DRAFT
devai task start <id>            # ANALYSIS
devai task run <id> --agent claude|hermes             # прогон цикла ролей
devai task report <id>           # рендер QUALITY_REPORT (§2.6)
devai task accept <id> --rationale "..."              # человеческий гейт
devai task merge <id>            # интеграция ветки в main
devai task active | exec list    # снимок движения
```

## Конфигурация оценки

- Грамматика профилей: `docs/evaluation-schema.md`
- Активный QA-профиль (платформенный binding): `config/evaluation/quality.yaml`
- Абстрактная база (фабричный вид без конкретики языка/проекта):
  `docs/internal/criteria-generic-draft.yaml`
- Продуктовые инварианты (workflow-переходы, append-only, locking) проверяются
  платформенным блоком того же профиля; gates и mechanical-пробы исполняет
  платформа, rubric-баллы выставляет аудитор с обязательным evidence.

## Как устроено (коротко)

`crates/devai-core` — переходы workflow и доменные типы; `crates/devai-store` —
SQLite + append-only события + артефакты (blobs по sha256); `crates/devai-api` —
единственная публичная граница к данным (типизированные Req/Outcome/ErrorCode);
`crates/devai-runtime` — engine прогонов ролей, gates, аудит-сборка;
`crates/devai-cli` — только ввод/вывод поверх API.

`docs/` — схема оценки, профили, снапшоты CLI-контрактов; `docs/internal/` —
черновики и прогоны калибровки.
