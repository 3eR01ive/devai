//! Чтения для нового экрана: правила оценки, расход токенов и куски кода.
//!
//! Три вещи, которых нет в доменных данных, но которые нужны человеку на
//! карточке:
//!   · правила приёмки — они лежат в ПРОФИЛЕ оценки дома (YAML), а отчёт
//!     качества хранит только результаты; читаем профиль и отдаём его как
//!     данные, ничего не пересчитывая;
//!   · расход токенов — драйвер пишет `usage.json` в каталог улик прогона;
//!     читаем эти файлы и агрегируем по задаче;
//!   · кусок исходника по ссылке «путь:строки» — читаем на зафиксированном
//!     коммите через git, чтобы показать ровно тот код, о котором говорит
//!     отчёт.
//!
//! Все три — чистые чтения: ни одна не пишет в дом и не меняет состояние.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Api, ApiResult};

// ── правила оценки ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDetectView {
    pub t: String,
    pub sev: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleView {
    pub id: String,
    pub weight: Option<f64>,
    pub floor: Option<f64>,
    pub verify: Option<String>,
    pub statement: String,
    /// Шкала: «1.0» → «кандидатов нет» и т.д., дословно из профиля.
    pub anchors: BTreeMap<String, String>,
    pub detects: Vec<RuleDetectView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleGroupView {
    pub group: String,
    pub weight: f64,
    pub items: Vec<RuleView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulebookView {
    pub profile_version: Option<String>,
    pub project: Option<String>,
    pub theta: Option<f64>,
    pub floors: BTreeMap<String, Option<f64>>,
    pub blocked_max: Option<f64>,
    pub groups: Vec<RuleGroupView>,
}

fn yaml_num(v: &serde_yaml::Value) -> Option<f64> {
    v.as_f64()
}

fn yaml_str(v: &serde_yaml::Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

impl Api {
    /// Профиль оценки дома как читаемые правила: группы, веса, шкалы, полы.
    /// Источник — `<home>/config/evaluation/quality.yaml`, при его отсутствии
    /// — `base.yaml` (дом без оверлея судится базой).
    pub fn evaluation_rules(&self) -> ApiResult<RulebookView> {
        let dir = self.home_path().join("config").join("evaluation");
        let file = if dir.join("quality.yaml").exists() {
            dir.join("quality.yaml")
        } else {
            dir.join("base.yaml")
        };
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(_) => {
                return Ok(RulebookView {
                    profile_version: None,
                    project: None,
                    theta: None,
                    floors: BTreeMap::new(),
                    blocked_max: None,
                    groups: Vec::new(),
                });
            }
        };
        let doc: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| {
            crate::ApiError::refused("bad_request", format!("профиль оценки нечитаем: {e}"))
        })?;

        let mut severities: BTreeMap<String, Option<String>> = BTreeMap::new();
        if let Some(map) = doc.get("error_types").and_then(|v| v.as_mapping()) {
            for (k, v) in map {
                let sev = v.get("severity").and_then(yaml_str);
                if let Some(name) = yaml_str(k) {
                    severities.insert(name, sev);
                }
            }
        }

        let mut floors: BTreeMap<String, Option<f64>> = BTreeMap::new();
        let mut weights: BTreeMap<String, f64> = BTreeMap::new();
        let mut theta = None;
        let mut blocked_max = None;
        if let Some(scoring) = doc.get("scoring") {
            theta = scoring.get("theta").and_then(yaml_num);
            if let Some(f) = scoring.get("floors").and_then(|v| v.as_mapping()) {
                for (k, v) in f {
                    if let Some(name) = yaml_str(k) {
                        floors.insert(name, yaml_num(v));
                    }
                }
            }
            if let Some(bp) = scoring.get("blocked_policy") {
                blocked_max = bp.get("max_blocked_weight").and_then(yaml_num);
            }
            if let Some(w) = scoring.get("weights").and_then(|v| v.as_mapping()) {
                for (k, v) in w {
                    if let (Some(name), Some(num)) = (yaml_str(k), yaml_num(v)) {
                        weights.insert(name, num);
                    }
                }
            }
        }

        let mut groups: Vec<RuleGroupView> = Vec::new();
        if let Some(evals) = doc.get("evaluations").and_then(|v| v.as_sequence()) {
            for ev in evals {
                let title = ev
                    .get("title")
                    .and_then(yaml_str)
                    .or_else(|| ev.get("id").and_then(yaml_str))
                    .unwrap_or_else(|| "без группы".into());
                let mut items = Vec::new();
                if let Some(checks) = ev.get("checks").and_then(|v| v.as_sequence()) {
                    for c in checks {
                        let id = c.get("id").and_then(yaml_str).unwrap_or_default();
                        if id.is_empty() {
                            continue;
                        }
                        let mut anchors = BTreeMap::new();
                        if let Some(a) = c.get("anchors").and_then(|v| v.as_mapping()) {
                            for (k, v) in a {
                                let key = match k {
                                    serde_yaml::Value::Number(n) => n.to_string(),
                                    serde_yaml::Value::String(s) => s.clone(),
                                    _ => continue,
                                };
                                if let Some(text) = yaml_str(v) {
                                    anchors.insert(key, text);
                                }
                            }
                        }
                        let mut detects = Vec::new();
                        let mut floor: Option<f64> = None;
                        if let Some(d) = c.get("detects").and_then(|v| v.as_sequence()) {
                            for t in d {
                                if let Some(name) = yaml_str(t) {
                                    let sev = severities.get(&name).cloned().flatten();
                                    if let Some(f) =
                                        sev.as_ref().and_then(|s| floors.get(s)).and_then(|v| *v)
                                    {
                                        floor = Some(floor.map_or(f, |cur: f64| cur.max(f)));
                                    }
                                    detects.push(RuleDetectView { t: name, sev });
                                }
                            }
                        }
                        items.push(RuleView {
                            weight: weights.get(&id).copied(),
                            id,
                            floor,
                            verify: c.get("verify").and_then(yaml_str),
                            statement: c.get("statement").and_then(yaml_str).unwrap_or_default(),
                            anchors,
                            detects,
                        });
                    }
                }
                items.sort_by(|a, b| {
                    b.weight
                        .unwrap_or(0.0)
                        .partial_cmp(&a.weight.unwrap_or(0.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.id.cmp(&b.id))
                });
                let sum: f64 = items.iter().map(|i| i.weight.unwrap_or(0.0)).sum();
                groups.push(RuleGroupView {
                    group: title,
                    weight: sum,
                    items,
                });
            }
        }
        groups.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(RulebookView {
            profile_version: doc.get("version").and_then(|v| match v {
                serde_yaml::Value::Number(n) => Some(n.to_string()),
                serde_yaml::Value::String(s) => Some(s.clone()),
                _ => None,
            }),
            project: doc.get("project").and_then(yaml_str),
            theta,
            floors,
            blocked_max,
            groups,
        })
    }
}

// ── расход токенов ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRowView {
    pub execution: String,
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cached: Option<i64>,
    pub total: i64,
    pub cost: f64,
    pub calls: Option<i64>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUsageView {
    pub task: String,
    pub total: i64,
    pub cost: f64,
    /// Сколько прогонов задачи вообще было и у скольких есть отчёт о расходе:
    /// честная доля «неизвестно» вместо тихого нуля.
    pub runs: usize,
    pub measured: usize,
    pub rows: Vec<UsageRowView>,
}

/// Поток прогона как он есть: то, что роль пишет в свои потоки, пока работает.
/// Улики платформы лежат в `<home>/evidence/<task>/<execution>/role_stdout.txt`;
/// читаем ХВОСТ с потолком байт, поэтому окно показывает тот же текст, что на
/// диске, и не пересказывает его.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecTranscriptView {
    pub execution: String,
    pub task: String,
    pub role: String,
    pub status: String,
    pub running: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    /// Показан только хвост (потолок чтения) — говорим прямо, а не молчим.
    pub truncated: bool,
    /// Почему потока нет / он обрезан. `None` = поток прочитан целиком.
    pub note: Option<String>,
}

/// Потолок чтения одного потока: хвост последних 128 КиБ. Больше в окно не
/// едет — терминал всё равно не покажет, а память окна конечна.
const TRANSCRIPT_TAIL_BYTES: u64 = 128 * 1024;

/// Хвост текстового потока: (текст, полный размер файла, обрезано ли).
fn tail_text(p: &std::path::Path) -> (String, u64, bool) {
    let Ok(meta) = std::fs::metadata(p) else {
        return (String::new(), 0, false);
    };
    let size = meta.len();
    let Ok(mut f) = std::fs::File::open(p) else {
        return (String::new(), size, false);
    };
    let skip = size.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    if skip > 0 {
        use std::io::Seek;
        let _ = f.seek(std::io::SeekFrom::Start(skip));
    }
    let mut buf = Vec::new();
    {
        use std::io::Read;
        let _ = f.read_to_end(&mut buf);
    }
    let mut text = String::from_utf8_lossy(&buf).to_string();
    if skip > 0 {
        // Срез с середины строки — отдаём с целой строки, чтобы терминал не
        // начинался огрызком.
        if let Some(i) = text.find('\n') {
            text = text[i + 1..].to_string();
        }
    }
    (text, size, skip > 0)
}

/// Один файл рабочего дерева прогона: как git его видит ПРЯМО СЕЙЧАС.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveFileView {
    pub path: String,
    /// Буква состояния из `git status --porcelain` (M/A/D/R/??).
    pub status: String,
    pub added: u32,
    pub removed: u32,
}

/// Что происходит в рабочем дереве задачи во время прогона. Отчёты и диффы
/// показывают КОММИТНУТОЕ (baseline..tip), а пока роль работает, её правки ещё
/// не закоммичены — и окно их не видело вовсе. Здесь — фактическое состояние
/// `worktrees/<задача>`: ветка, HEAD, изменённые файлы с числом строк.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveWorktreeView {
    pub task: String,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub files: Vec<LiveFileView>,
    pub dirty: usize,
    /// Список обрезан: правок больше, чем окно показывает.
    pub truncated: bool,
    /// Прогон задачи идёт прямо сейчас — значит дерево ещё будет меняться.
    pub running: bool,
    pub note: Option<String>,
}

/// Дифф одного файла живого дерева (незакоммиченные правки).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveDiffView {
    pub path: String,
    pub diff: String,
    pub note: Option<String>,
}

/// Потолок списка файлов: остальное — счётчиком, а не молчанием.
const LIVE_FILES_MAX: usize = 200;

fn num(v: &serde_json::Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_i64())
}

impl Api {
    /// Расход токенов по задаче: файлы `usage.json` в каталоге улик прогонов.
    /// Прогоны без отчёта считаются, но в сумму не входят — «неизвестно»
    /// остаётся видимым (measured из runs).
    pub fn task_usage(&self, id_prefix: &str) -> ApiResult<TaskUsageView> {
        let task = self.store.resolve_task(id_prefix)?;
        let runs = self.store.list_executions(Some(task.id))?;
        let dir = self.home_path().join("evidence").join(task.id.to_string());
        let mut rows = Vec::new();
        let mut total = 0i64;
        let mut cost = 0.0f64;
        let mut measured = 0usize;
        for e in &runs {
            let p = dir.join(e.id.to_string()).join("usage.json");
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let t = num(&v, "total_tokens").unwrap_or(0);
            let c = v
                .get("estimated_cost_usd")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0);
            total += t;
            cost += c;
            measured += 1;
            rows.push(UsageRowView {
                execution: e.id.to_string(),
                input: num(&v, "input_tokens"),
                output: num(&v, "output_tokens"),
                cached: num(&v, "cache_read_tokens"),
                total: t,
                cost: c,
                calls: num(&v, "api_calls"),
                model: v.get("model").and_then(|x| x.as_str()).map(str::to_string),
            });
        }
        Ok(TaskUsageView {
            task: task.id.to_string(),
            total,
            cost,
            runs: runs.len(),
            measured,
            rows,
        })
    }

    /// Живое рабочее дерево задачи: то, что роль правит ПРЯМО СЕЙЧАС.
    /// `workspace_changes` показывает закоммиченный changeset baseline..tip —
    /// у идущего прогона он ещё пуст, хотя файлы уже меняются.
    pub fn workspace_live(&self, id_prefix: &str) -> ApiResult<LiveWorktreeView> {
        let task = self.store.resolve_task(id_prefix)?;
        let dir = self.home_path().join("worktrees").join(task.id.to_string());
        let runs = self.store.list_executions(Some(task.id))?;
        let running = runs.iter().any(|e| e.finished_at.is_none());
        if !dir.is_dir() {
            return Ok(LiveWorktreeView {
                task: task.id.to_string(),
                worktree: None,
                branch: None,
                head: None,
                files: vec![],
                dirty: 0,
                truncated: false,
                running,
                note: Some(format!(
                    "изолированного дерева у задачи нет ({} не существует) — роль правила выложенное",
                    dir.display()
                )),
            });
        }
        let git = |args: &[&str]| -> Option<String> {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        };
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string());
        let head = git(&["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
        // Счётчики строк по файлам: рабочее (unstaged) + индекс (staged).
        let mut counts: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
        for args in [
            vec!["diff", "--numstat", "HEAD"],
            vec!["diff", "--cached", "--numstat"],
        ] {
            if let Some(out) = git(&args) {
                for line in out.lines() {
                    let mut it = line.split('\t');
                    let (Some(a), Some(r), Some(p)) = (it.next(), it.next(), it.next()) else {
                        continue;
                    };
                    let entry = counts.entry(p.to_string()).or_default();
                    entry.0 += a.parse::<u32>().unwrap_or(0);
                    entry.1 += r.parse::<u32>().unwrap_or(0);
                }
            }
        }
        let mut files = Vec::new();
        if let Some(out) = git(&["status", "--porcelain"]) {
            for line in out.lines() {
                if line.len() < 4 {
                    continue;
                }
                let code = line[..2].trim().to_string();
                let path = line[3..].trim().to_string();
                let (added, removed) = counts.get(&path).copied().unwrap_or((0, 0));
                files.push(LiveFileView {
                    status: if code == "??" { "??".to_string() } else { code },
                    path,
                    added,
                    removed,
                });
            }
        }
        let dirty = files.len();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let truncated = dirty > LIVE_FILES_MAX;
        files.truncate(LIVE_FILES_MAX);
        Ok(LiveWorktreeView {
            task: task.id.to_string(),
            worktree: Some(dir.display().to_string()),
            branch,
            head,
            files,
            dirty,
            truncated,
            running,
            note: if dirty == 0 {
                Some("в рабочем дереве чисто: роль ещё ничего не поменяла (или уже закоммитила)".to_string())
            } else {
                None
            },
        })
    }

    /// Дифф ОДНОГО файла живого дерева: правки, которых ещё нет в коммите.
    /// Тем же `git diff` по рабочему каталогу задачи, что и список.
    pub fn workspace_live_diff(
        &self,
        id_prefix: &str,
        path: &str,
    ) -> ApiResult<LiveDiffView> {
        let task = self.store.resolve_task(id_prefix)?;
        let dir = self.home_path().join("worktrees").join(task.id.to_string());
        if !dir.is_dir() {
            return Err(crate::ApiError::refused(
                "no_worktree",
                "у задачи нет изолированного рабочего дерева",
            ));
        }
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["diff", "HEAD", "--", path])
            .output()
            .map_err(|e| crate::ApiError::refused("git_failed", format!("git: {e}")))?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let note = if text.trim().is_empty() {
            Some("в этом файле нет незакоммиченных правок (или он не менялся)".to_string())
        } else {
            None
        };
        Ok(LiveDiffView {
            path: path.to_string(),
            diff: text,
            note,
        })
    }

    /// Консоль прогона: потоки роли прямо сейчас. Читаются те же улики, что
    /// пишет исполнение (`role_stdout.txt` / `role_stderr.txt` в каталоге
    /// прогона), поэтому живой прогон виден по мере записи, а завершённый —
    /// своим хвостом. Пустой поток и обрезка названы, а не выданы за тишину.
    pub fn exec_transcript(
        &self,
        id_prefix: &str,
        execution: &str,
    ) -> ApiResult<ExecTranscriptView> {
        let task = self.store.resolve_task(id_prefix)?;
        let runs = self.store.list_executions(Some(task.id))?;
        let e = runs
            .into_iter()
            .find(|r| r.id.to_string() == execution || r.id.to_string().starts_with(execution))
            .ok_or_else(|| {
                crate::ApiError::refused(
                    "unknown_execution",
                    format!("у задачи нет прогона `{execution}`"),
                )
            })?;
        let dir = self
            .home_path()
            .join("evidence")
            .join(task.id.to_string())
            .join(e.id.to_string());
        let (stdout, stdout_bytes, so_cut) = tail_text(&dir.join("role_stdout.txt"));
        let (stderr, stderr_bytes, se_cut) = tail_text(&dir.join("role_stderr.txt"));
        let note = if stdout_bytes == 0 && stderr_bytes == 0 {
            Some(
                "роль ничего не написала в свои потоки (или прогон шёл без файла улик)"
                    .to_string(),
            )
        } else if so_cut || se_cut {
            Some(format!(
                "показан хвост: потолок {} КиБ на поток",
                TRANSCRIPT_TAIL_BYTES / 1024
            ))
        } else {
            None
        };
        Ok(ExecTranscriptView {
            execution: e.id.to_string(),
            task: task.id.to_string(),
            role: e.role_slug.clone(),
            status: e.status.as_str().to_string(),
            running: e.finished_at.is_none(),
            started_at: e.started_at.to_rfc3339(),
            finished_at: e.finished_at.map(|t| t.to_rfc3339()),
            stdout,
            stderr,
            stdout_bytes,
            stderr_bytes,
            truncated: so_cut || se_cut,
            note,
        })
    }
}

// ── кусок исходника по ссылке «путь:строки» ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSliceView {
    pub path: String,
    pub rev: String,
    pub from: u32,
    pub to: u32,
    /// Сколько строк в файле на этом коммите — чтобы окно могло развернуть
    /// остальное, не угадывая конец файла запросами наугад.
    pub total_lines: u32,
    pub text: String,
    /// Файл с таким хвостом есть в нескольких местах дерева: вместо догадки
    /// отдаём варианты, и человек выбирает сам.
    #[serde(default)]
    pub candidates: Vec<String>,
    /// Чем кончилось разрешение ревизии: ссылка из отчёта может указывать на
    /// коммит, которого в этом репозитории уже нет (ветку убрали, ворктри
    /// удалили). Тогда честно читаем ближайшую доступную ревизию и говорим,
    /// какую — вместо «код недоступен» на каждой ссылке.
    #[serde(default)]
    pub rev_note: Option<String>,
}

/// Чем кончилось разрешение сокращённого пути.
enum Resolved {
    Hit(String),
    /// Несколько файлов заканчиваются так же — гадать запрещено.
    Ambiguous(Vec<String>),
    Missing,
}

/// Путь файла в коммите: точный или единственный по хвосту.
/// Отчёты пишут ссылки сокращённо, а `git show` требует полный путь.
fn resolve_path(repo: &Path, rev: &str, path: &str) -> Result<Resolved, crate::ApiError> {
    let exact = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e", &format!("{rev}:{path}")])
        .status();
    if matches!(exact, Ok(s) if s.success()) {
        return Ok(Resolved::Hit(path.to_string()));
    }
    let listing = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-tree", "-r", "--name-only", rev])
        .output()
        .map_err(|e| crate::ApiError::refused("no_source", format!("git недоступен: {e}")))?;
    let listed = String::from_utf8_lossy(&listing.stdout).to_string();
    let files: Vec<&str> = listed.lines().collect();
    let tail = format!("/{path}");
    let hits: Vec<&str> = files
        .iter()
        .copied()
        .filter(|f| *f == path || f.ends_with(&tail))
        .collect();
    match hits.len() {
        1 => Ok(Resolved::Hit(hits[0].to_string())),
        0 => Ok(Resolved::Missing),
        _ => Ok(Resolved::Ambiguous(
            hits.iter().take(20).map(|s| s.to_string()).collect(),
        )),
    }
}

/// Ревизия, которую реально можно прочитать: названная, иначе её полный
/// коммит, иначе текущая HEAD. Возвращает пару (ревизия, объяснение подмены).
fn resolve_rev(repo: &Path, rev: &str) -> (String, Option<String>) {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    if git(&["cat-file", "-e", &format!("{rev}^{{commit}}")]).is_some() {
        return (rev.to_string(), None);
    }
    if let Some(sha) = git(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")]) {
        return (sha, None);
    }
    if let Some(head) = git(&["rev-parse", "HEAD"]) {
        return (
            head,
            Some(format!(
                "ревизии {rev} в этом репозитории нет — код читается с текущей HEAD"
            )),
        );
    }
    (rev.to_string(), None)
}

fn repo_of(facts_repo: Option<PathBuf>) -> Option<PathBuf> {
    facts_repo
}

impl Api {
    /// Кусок файла на зафиксированном коммите: ровно то, о чём говорит
    /// ссылка отчёта. Читается `git show <rev>:<path>` из репозитория задачи;
    /// вне диапазона — тихий отказ, а не выдуманный код.
    pub fn source_slice(
        &self,
        id_prefix: &str,
        rev: &str,
        path: &str,
        from: u32,
        to: u32,
    ) -> ApiResult<SourceSliceView> {
        let f = self.workspace_facts(id_prefix)?;
        let repo = repo_of(f.repo).ok_or_else(|| {
            crate::ApiError::refused("no_workspace", "у задачи нет известного репозитория")
        })?;
        // Отчёты ссылаются на файлы сокращённо («signature/types.rs» вместо
        // «crates/devai-api/src/signature/types.rs»). Путь разрешаем по дереву
        // коммита: точное совпадение, иначе единственное совпадение по хвосту.
        // Неоднозначность — отказ со списком, а не догадка.
        // Ссылка из отчёта может называть коммит, которого уже нет: читаем
        // ближайшую доступную ревизию и говорим, какую именно.
        let (rev_owned, rev_note) = resolve_rev(&repo, rev);
        let rev = rev_owned.as_str();
        let path = match resolve_path(&repo, rev, path)? {
            Resolved::Hit(p) => p,
            Resolved::Ambiguous(candidates) => {
                return Ok(SourceSliceView {
                    path: path.to_string(),
                    rev: rev.to_string(),
                    from,
                    to,
                    total_lines: 0,
                    text: String::new(),
                    candidates,
                    rev_note,
                });
            }
            Resolved::Missing => {
                return Err(crate::ApiError::refused(
                    "no_source",
                    format!("в коммите {rev} нет файла {path}"),
                ));
            }
        };
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["show", &format!("{rev}:{}", path)])
            .output();
        let text = match out {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => {
                return Err(crate::ApiError::refused(
                    "no_source",
                    format!("в коммите {rev} нет файла {path}"),
                ));
            }
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let start = (from.max(1) - 1) as usize;
        let end = if to >= from { to as usize } else { start + 12 };
        let end = end.min(lines.len());
        if start >= lines.len() {
            return Err(crate::ApiError::refused(
                "no_source",
                format!("в {path} меньше {from} строк"),
            ));
        }
        // Разбор по переводу строки даёт пустой хвост у файла, который
        // заканчивается переводом строки: строкой он не считается.
        let total = if text.ends_with('\n') {
            lines.len().saturating_sub(1)
        } else {
            lines.len()
        };
        Ok(SourceSliceView {
            path: path.to_string(),
            rev: rev.to_string(),
            from: (start + 1) as u32,
            to: end as u32,
            total_lines: total as u32,
            text: lines[start..end].join("\n"),
            candidates: Vec::new(),
            rev_note,
        })
    }
}

/// Путь дома — нужен чтениям выше; держим тут, чтобы API не отдавал наружу
/// собственный корень.
trait HomePath {
    fn home_path(&self) -> &Path;
}

impl HomePath for Api {
    fn home_path(&self) -> &Path {
        self.home_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::{Resolved, resolve_path};
    use std::path::Path;
    use std::process::Command;

    /// Временный git-репозиторий: два уровня каталогов, чтобы проверять хвост.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        let sh = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        sh(&["init", "-q"]);
        std::fs::create_dir_all(dir.path().join("crates/api/src/signature")).unwrap();
        std::fs::write(
            dir.path().join("crates/api/src/signature/types.rs"),
            "fn a() {}\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("crates/api/src/lib.rs"), "fn b() {}\n").unwrap();
        sh(&["add", "-A"]);
        sh(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ]);
        dir
    }

    fn head(dir: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn exact_path_wins() {
        let dir = repo();
        let rev = head(dir.path());
        match resolve_path(dir.path(), &rev, "crates/api/src/lib.rs").unwrap() {
            Resolved::Hit(p) => assert_eq!(p, "crates/api/src/lib.rs"),
            _ => panic!("должно быть точное совпадение"),
        }
    }

    #[test]
    fn short_path_resolves_to_the_only_tail_match() {
        // Отчёт писал «signature/types.rs» — полный путь ищется по хвосту.
        let dir = repo();
        let rev = head(dir.path());
        match resolve_path(dir.path(), &rev, "signature/types.rs").unwrap() {
            Resolved::Hit(p) => assert_eq!(p, "crates/api/src/signature/types.rs"),
            _ => panic!("должен найтись единственный хвост"),
        }
    }

    #[test]
    fn unknown_rev_reads_head_and_says_so() {
        // Ссылка может называть коммит, которого в репозитории уже нет
        // (ветку убрали, ворктри удалили): читаем HEAD и объясняем подмену.
        let dir = repo();
        let (rev, note) = super::resolve_rev(dir.path(), "2519218");
        assert_eq!(rev, head(dir.path()));
        assert!(note.is_some(), "подмена должна быть объяснена");
    }

    #[test]
    fn known_rev_is_used_as_named() {
        let dir = repo();
        let h = head(dir.path());
        let (rev, note) = super::resolve_rev(dir.path(), &h);
        assert_eq!(rev, h);
        assert!(note.is_none());
    }

    #[test]
    fn same_tail_in_two_places_gives_candidates_not_a_guess() {
        // «types.rs» встречается и в api, и в core — выбрать за человека нельзя.
        let dir = repo();
        std::fs::create_dir_all(dir.path().join("crates/core/src")).unwrap();
        std::fs::write(dir.path().join("crates/core/src/types.rs"), "fn c() {}\n").unwrap();
        let sh = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(dir.path())
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        sh(&["add", "-A"]);
        sh(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "two",
        ]);
        let rev = head(dir.path());
        match resolve_path(dir.path(), &rev, "types.rs").unwrap() {
            Resolved::Ambiguous(c) => {
                assert_eq!(c.len(), 2, "{c:?}");
                assert!(c.iter().any(|p| p.ends_with("signature/types.rs")), "{c:?}");
                assert!(c.iter().any(|p| p.ends_with("core/src/types.rs")), "{c:?}");
            }
            _ => panic!("ожидались варианты"),
        }
    }

    #[test]
    fn missing_file_is_a_typed_refusal_not_a_guess() {
        let dir = repo();
        let rev = head(dir.path());
        assert!(matches!(
            resolve_path(dir.path(), &rev, "nope/absent.rs").unwrap(),
            Resolved::Missing
        ));
    }
}
