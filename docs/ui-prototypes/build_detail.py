#!/usr/bin/env python3
"""Сборщик данных для прототипа ЭКРАНА ЗАДАЧИ из живого дома.

    python3 build_detail.py 01a1b5e1-dea0-7495-a293-d486e3678144

Ничего не пишет в дом: только читает прогресс, артефакты, вопросы, расход
и куски кода по ссылкам «путь:строки» из отчётов. Результат — detail.json.
"""
import json
import os
import re
import subprocess
from datetime import datetime
import sys

HOME = os.path.expanduser('~/.devai')
RULES = {}
_rp = '/tmp/proto/rules.json'
if os.path.exists(_rp):
    RULES = json.load(open(_rp, encoding='utf-8'))
REPO = os.path.expanduser('~/dev/devai')
BIN = os.path.join(REPO, 'bin', 'devai')
TASK = sys.argv[1] if len(sys.argv) > 1 else '01a1b5e1-dea0-7495-a293-d486e3678144'


def cli(*args):
    p = subprocess.run([BIN, '--home', HOME, *args], capture_output=True, text=True)
    return p.stdout


def jcli(*args):
    out = cli('--json', *args)
    return json.loads(out, strict=False) if out.strip() else None


head_ref = ['']  # реальный head известен позже; runs_of читает его на лету

prog = jcli('task', 'progress', TASK)
task = prog['task']
arts = jcli('artifact', 'list', TASK) or []
questions = jcli('question', 'list', TASK) or []

# ── расход по прогонам: usage.json в каталоге улик ──────────────────────────
evid = os.path.join(HOME, 'evidence', TASK)


def usage_of(exec_id):
    p = os.path.join(evid, exec_id, 'usage.json')
    if not os.path.exists(p):
        return None
    u = json.load(open(p))
    return {'in': u.get('input_tokens'), 'out': u.get('output_tokens'),
            'cached': u.get('cache_read_tokens'), 'total': u.get('total_tokens'),
            'cost': u.get('estimated_cost_usd'), 'calls': u.get('api_calls'),
            'model': u.get('model')}


# ── код по ссылкам «путь:строки» ───────────────────────────────────────────
REF = re.compile(r'([\w./-]+\.(?:rs|ts|vue|js|py|yaml|yml|json|toml|sh|sql))'
                 r'((?::\d+(?:-\d+)?)(?:,\s?\d+(?:-\d+)?)*)?')
TREE = {}


def tree_paths(head):
    key = head or 'HEAD'
    if key in TREE:
        return TREE[key]
    try:
        out = subprocess.run(['git', '-C', REPO, 'ls-tree', '-r', '--name-only', key],
                             capture_output=True, text=True, timeout=30).stdout
        TREE[key] = [p for p in out.split('\n') if p]
    except Exception:
        TREE[key] = []
    return TREE[key]


def resolve_path(path, head):
    """Ссылки в отчётах часто короткие («graph.rs:1466») — находим полный путь
    по дереву зафиксированного коммита, а не угадываем."""
    if os.path.exists(os.path.join(REPO, path)):
        return path
    files = tree_paths(head)
    exact = [f for f in files if f == path]
    if exact:
        return exact[0]
    tail = [f for f in files if f.endswith('/' + path)]
    if len(tail) == 1:
        return tail[0]
    if tail:
        return tail[0]
    base = path.split('/')[-1]
    byname = [f for f in files if f.endswith('/' + base)]
    if len(byname) == 1:
        return byname[0]
    return None
_cache = {}


def code_for(path, a, b, head):
    """Вставка кода по пути и диапазону строк на зафиксированном коммите."""
    key = (path, a, b, head)
    if key in _cache:
        return _cache[key]
    text = None
    if head:
        try:
            text = subprocess.run(['git', '-C', REPO, 'show', f'{head}:{path}'],
                                  capture_output=True, text=True, timeout=20).stdout
        except Exception:
            text = None
    if not text:
        full = os.path.join(REPO, path)
        if os.path.exists(full):
            text = open(full, encoding='utf-8', errors='replace').read()
    if not text:
        _cache[key] = None
        return None
    lines = text.split('\n')
    if a:
        start = max(0, int(a) - 1)
        end = int(b) if b else int(a) + 11
        end = min(len(lines), max(end, start + 1))
        if end - start > 40:
            end = start + 40
        snippet = '\n'.join(lines[start:end])
        first = start + 1
    else:
        snippet = '\n'.join(lines[:14])
        first = 1
    res = {'path': path, 'line': first, 'code': snippet}
    _cache[key] = res
    return res


# По каждому виду берём ПОСЛЕДНЮЮ версию: у живой задачи «текущей» может
# не быть (отчёт устарел после правки), но человеку нужен именно он.
latest = {}
for a in arts:
    cur = latest.get(a['kind'])
    if not cur or a['seq'] >= cur['seq'] or (a['status'] == 'CURRENT' and cur['status'] != 'CURRENT'):
        latest[a['kind']] = a
    if a['status'] == 'CURRENT':
        latest[a['kind']] = a
out_arts = {}
for kind, a in latest.items():
    body = cli('artifact', 'cat', a['id'])
    try:
        out_arts[kind] = {'meta': a, 'body': json.loads(body, strict=False)}
    except Exception:
        out_arts[kind] = {'meta': a, 'text': body[:20000]}

head = (out_arts.get('IMPLEMENTATION_REPORT', {}).get('body') or {}).get('commit') \
    or (out_arts.get('QUALITY_REPORT', {}).get('body') or {}).get('workspace_revision', {}).get('git_head')
head_ref[0] = head or ''

# ── сходимость: раунды по стадиям ──────────────────────────────────────────
def convergence_state():
    """Сходимость как её отдаёт платформа: раунды стадии с баллом, порогом,
    дельтой, объёмом требований и режимом. Неизмеренный раунд остаётся
    неизмеренным — окно не дорисовывает ему балл."""
    c = prog.get('convergence') or {}
    out = []
    for st in c.get('stages') or []:
        rounds = []
        for r in st.get('series') or []:
            rounds.append({
                'round': r.get('round'),
                'score': (r['score_milli'] / 1000) if r.get('score_milli') is not None else None,
                'theta': (r['theta_milli'] / 1000) if r.get('theta_milli') is not None else None,
                'delta': (r['delta_score_milli'] / 1000) if r.get('delta_score_milli') is not None else None,
                'reqs': r.get('requirements_count'),
                'oq': r.get('oq_count'),
                'verdict': r.get('verdict'),
                'mode': r.get('mode'),
                'measured': bool(r.get('measured')),
            })
        out.append({'stage': st.get('stage'), 'rounds': rounds})
    return out


CONV = convergence_state()

# ── worktree: файлы изменения и их diff (шаг «реализация») ─────────────────
def full_diff(worktree, base, tip, path):
    """Diff ВСЕГО файла: весь контекст, чтобы окно могло развернуть скрытые
    строки без второго чтения. Тот же диапазон база..tip, что у платформы."""
    if not worktree or not base or not tip:
        return None
    try:
        out = subprocess.run(
            ['git', '-C', worktree, 'diff', '-U100000', base, tip, '--', path],
            capture_output=True, text=True, timeout=30)
        return out.stdout if out.returncode == 0 else None
    except Exception:
        return None


def worktree_state():
    """Снимок изменения задачи: ветка, база, tip, файлы со статусами.
    Один read платформы; diff тянем по файлу только когда он есть."""
    raw = cli('--json', 'task', 'workspace', TASK)
    try:
        ws = json.loads(raw, strict=False)
    except Exception:
        return None
    files = ws.get('files') or []
    out = {'state': ws.get('state'), 'branch': ws.get('branch'),
           'baseline': ws.get('bound_head'), 'tip': ws.get('tip'),
           'commits': ws.get('commits') or [], 'files': []}
    # diff по каждому файлу — только для реально изменённых, максимум 25
    for f in files[:25]:
        d = cli('--json', 'task', 'workspace-diff', TASK, f['path'])
        try:
            j = json.loads(d, strict=False)
        except Exception:
            continue
        full = full_diff(ws.get('worktree'), ws.get('bound_head'), ws.get('tip'), f['path'])
        out['files'].append({'status': f.get('status'), 'path': f['path'],
                             'binary': bool(j.get('binary')),
                             'diff': j.get('diff') or '',
                             'full': full or j.get('diff') or ''})
    return out


WORKTREE = worktree_state()

# ── стадии ─────────────────────────────────────────────────────────────────
stages = []
for s in prog['graph']['stages']:
    stages.append({'state': s['state'], 'label': s['label'], 'executor': s.get('executor'),
                   'gate': bool(s.get('gate')), 'status': s['status'],
                   'word': s.get('status_word', '')})

nodes = {n['state']: n for n in prog['graph'].get('nodes', []) if n.get('kind') == 'role'}
for st in stages:
    n = nodes.get(st['state'])
    st['role_slug'] = (n or {}).get('role_slug')
    st['role_version'] = (n or {}).get('role_version')
    st['in_contract'] = (n or {}).get('input_contract')
    st['out_contract'] = (n or {}).get('output_contract')
    execs = (n or {}).get('executions') or []
    st['executions'] = [{'id': e['id'], 'status': e['status'], 'attempt': e.get('attempt'),
                         'started': e.get('started_at'), 'finished': e.get('finished_at'),
                         'summary': e.get('summary'), 'error': e.get('error'),
                         'usage': usage_of(e['id'])} for e in execs]
    st['attempts'] = len(execs)
    st['running'] = [e['id'] for e in execs if str(e.get('status')) == 'RUNNING']

# артефакты, произведённые прогонами стадии
for st in stages:
    ids = {e['id'] for e in st['executions']}
    st['produced'] = [a for a in arts if a.get('execution_id') in ids]

# ── данные ВЫБРАННОГО прогона: тело артефакта, который он отдал ────────────
# Вкладки шага раньше всегда показывали последний артефакт. Здесь готовим
# тела по прогонам, чтобы клик по засечке менял и данные, а не только статус.
def req_oq(spec):
    """Вопросы, привязанные к пунктам требования (по коду REQ-… в тексте)."""
    reqs = spec.get('requirements') or []
    oqs = spec.get('open_questions') or []
    by_req, common = {}, []
    ids = [r.get('id') for r in reqs]
    for q in oqs:
        hit = [i for i in ids if i and i in q]
        if hit:
            by_req.setdefault(hit[0], []).append(q)
        else:
            common.append(q)
    return by_req, common


def _trim(kind, body):
    """Срезаем служебное, что не рисуется: адреса кода, историю правок,
    счётчики. Оставляем то, что видно во вкладках."""
    if not isinstance(body, dict):
        return body
    if kind == 'REQUIREMENTS_SPEC':
        return {
            'task_name': body.get('task_name'), 'version': body.get('version'),
            'human_paragraph': body.get('human_paragraph'),
            'requirements': [{'id': r.get('id'), 'statement': r.get('statement'),
                              'acceptance_criteria': r.get('acceptance_criteria') or []}
                             for r in body.get('requirements') or []],
            'open_questions': body.get('open_questions') or [],
        }
    if kind == 'REVIEW_REPORT':
        return {
            'verdict': body.get('verdict'), 'summary': body.get('summary'),
            'human_paragraph': body.get('human_paragraph'),
            'spec_seq': body.get('spec_seq'),
            'findings': [{'severity': f.get('severity'), 'description': f.get('description'),
                          'requirement_ref': f.get('requirement_ref')}
                         for f in body.get('findings') or []],
        }
    if kind == 'IMPLEMENTATION_REPORT':
        return {'branch': body.get('branch'), 'commit': body.get('commit'),
                'human_paragraph': body.get('human_paragraph'), 'notes': body.get('notes')}
    return body  # QUALITY_REPORT — целиком: по нему строится вкладка качества


_BODY_CACHE = {}


def artifact_body(aid):
    if aid not in _BODY_CACHE:
        raw = cli('artifact', 'cat', aid)
        try:
            _BODY_CACHE[aid] = json.loads(raw, strict=False)
        except Exception:
            _BODY_CACHE[aid] = None
    return _BODY_CACHE[aid]


BY_EXEC = {}
for a in arts:
    if a.get('execution_id'):
        BY_EXEC.setdefault(a['execution_id'], []).append(a)


def runs_of(stage):
    """Прогоны шага с артефактом каждого: засечка → её данные."""
    out = []
    for e in stage.get('executions') or []:
        items = BY_EXEC.get(e['id']) or []
        art = None
        if items:
            a = items[-1]
            bod = artifact_body(a['id'])
            art = {'kind': a['kind'], 'seq': a['seq'], 'display': a.get('display'),
                   'status': a['status'], 'body': _trim(a['kind'], bod)}
            if a['kind'] == 'QUALITY_REPORT' and isinstance(bod, dict):
                rows, totals = qa_pack(bod, head_ref[0])
                art['checks'] = rows
                art['qa_totals'] = totals
            if a['kind'] == 'REQUIREMENTS_SPEC' and isinstance(bod, dict):
                by_req, common = req_oq(bod)
                art['spec'] = {
                    'paragraph': bod.get('human_paragraph') or '',
                    'requirements': [{'id': r.get('id'), 'statement': r.get('statement') or '',
                                      'ac': r.get('acceptance_criteria') or [],
                                      'oq': [{'text': q} for q in by_req.get(r.get('id'), [])]}
                                     for r in bod.get('requirements') or []],
                    'common': common,
                    'assumptions': bod.get('assumptions') or [],
                    'unknowns': bod.get('unknowns') or [],
                }
            if a['kind'] == 'REVIEW_REPORT' and isinstance(bod, dict):
                art['review'] = {
                    'verdict': bod.get('verdict'),
                    'paragraph': bod.get('human_paragraph') or '',
                    'spec_seq': bod.get('spec_seq'),
                    'findings': [{'req': f.get('requirement_ref'), 'severity': f.get('severity'),
                                  'text': f.get('description') or ''}
                                 for f in (bod.get('findings') or [])][:14],
                }
            if a['kind'] == 'IMPLEMENTATION_REPORT' and isinstance(bod, dict):
                art['impl'] = {'paragraph': bod.get('human_paragraph') or '',
                               'branch': bod.get('branch'),
                               'commit': (bod.get('commit') or '')[:10],
                               'notes': (bod.get('notes') or '')[:1200]}
        out.append({
            'id': e['id'], 'status': e['status'], 'attempt': e.get('attempt'),
            'started': e.get('started'), 'finished': e.get('finished'),
            'summary': e.get('summary'), 'error': (e.get('error') or '')[:400],
            'usage': e.get('usage'), 'art': art,
        })
    return out


# вопросы платформы и вопросы спеки
for q in questions:
    for st in stages:
        if q.get('execution_id') in {e['id'] for e in st['executions']}:
            st.setdefault('platform_questions', []).append(q)

spec = out_arts.get('REQUIREMENTS_SPEC', {}).get('body') or {}
for st in stages:
    if st['state'] == 'ANALYSIS':
        st['spec'] = spec

rev = out_arts.get('REVIEW_REPORT', {}).get('body') or {}
impl = out_arts.get('IMPLEMENTATION_REPORT', {}).get('body') or {}
# УЛИКИ прогона: то, что роль реально складывала рядом с отчётом. Отчёт говорит
# «exit 0», улика показывает, что команда шла и чем кончилась — это разные вещи,
# и окно показывает их раздельно, не выдавая одно за другое.
def evidence_of(state):
    st = [x for x in stages if x['state'] == state]
    if not st or not st[0]['executions']:
        return []
    ex = st[0]['executions'][-1]
    d = os.path.join(os.path.expanduser('~/.devai/evidence'), TASK, ex['id'])
    if not os.path.isdir(d):
        return []
    rows = []
    for name in sorted(os.listdir(d)):
        fp = os.path.join(d, name)
        if not os.path.isfile(fp):
            continue
        try:
            sz = os.path.getsize(fp)
            mt = datetime.fromtimestamp(os.path.getmtime(fp)).strftime('%d.%m %H:%M')
            txt = open(fp, encoding='utf-8', errors='replace').read()
        except Exception:
            continue
        tail = '\n'.join(txt.splitlines()[-14:])
        rows.append({'name': name, 'size': sz, 'at': mt, 'tail': tail, 'lines': len(txt.splitlines())})
    return rows

EVIDENCE = []
qual = out_arts.get('QUALITY_REPORT', {}).get('body') or {}

for st in stages:
    if st['state'] == 'REVIEW':
        st['review'] = rev
    if st['state'] == 'IMPLEMENTATION':
        st['impl'] = impl
        st['evidence'] = evidence_of('IMPLEMENTATION')
        st['worktree'] = WORKTREE
    if st['state'] == 'QA':
        st['quality'] = qual

# куски кода по ссылкам отчётов
def inline_pieces(text, head):
    """Текст свидетельства, разложенный на куски: проза и вставки кода РОВНО
    там, где стоит ссылка. Все ссылки; одно упоминание может нести несколько
    диапазонов («path:15-24, 29-36») — каждый становится своей вставкой."""
    if not text:
        return []
    pieces, last = [], 0
    for m in REF.finditer(text):
        if m.start() < last:
            continue
        raw_path, ranges = m.group(1), m.group(2) or ''
        path = resolve_path(raw_path, head)
        if not path:
            continue
        spans = []
        for part in [r.strip() for r in ranges.lstrip(':').split(',') if r.strip()]:
            if '-' in part:
                a, b = part.split('-', 1)
            else:
                a, b = part, None
            spans.append((a, b))
        if not spans:
            spans = [(None, None)]
        blocks = []
        for a, b in spans:
            c = code_for(path, a, b, head)
            if c:
                blocks.append({'t': 'code', 'ref': f'{path}:{a or ""}',
                               'path': c['path'], 'line': c['line'], 'code': c['code']})
        if not blocks:
            continue
        if m.start() > last:
            pieces.append({'t': 'text', 'v': text[last:m.start()]})
        pieces.extend(blocks)
        last = m.end()
    if last < len(text):
        pieces.append({'t': 'text', 'v': text[last:]})
    # вокруг вставки не должны оставаться сироты-скобки: «(path:15-24)»
    for i, piece in enumerate(pieces):
        if piece['t'] != 'code':
            continue
        if i and pieces[i-1]['t'] == 'text':
            pieces[i-1]['v'] = pieces[i-1]['v'].rstrip().rstrip('(,').rstrip()
        if i + 1 < len(pieces) and pieces[i+1]['t'] == 'text':
            pieces[i+1]['v'] = pieces[i+1]['v'].lstrip()
            if pieces[i+1]['v'][:1] in ('),', ')'):
                pieces[i+1]['v'] = pieces[i+1]['v'][1:].lstrip()
    return [x for x in pieces if x['t'] == 'code' or x['v'].strip()]


def codes_from_refs(refs):
    """Совместимость: плоские вставки по списку ссылок."""
    out = []
    for r in refs:
        for p in inline_pieces(r, head):
            if p['t'] == 'code':
                out.append(p)
    return out


def refs_of(text):
    out = []
    for m in REF.finditer(text or ''):
        out.append(m.group(0))
        if len(out) >= 3:
            break
    return out


if impl:
    for st in stages:
        if st['state'] == 'IMPLEMENTATION':
            st['coverage'] = [{'req': c.get('id') or c.get('req'),
                               'status': c.get('status'),
                               'text': c.get('evidence'),
                               'pieces': inline_pieces(c.get('evidence'), head)}
                              for c in impl.get('requirement_coverage', [])]
# ── правила приёмки (спецификация приёмки) из профиля дома ────────────────
def rulebook():
    rules = (RULES.get('rules') or {})
    weights = RULES.get('weights') or {}
    groups = {}
    for cid, r in rules.items():
        groups.setdefault(r.get('group') or 'без группы', []).append({
            'id': cid,
            'weight': weights.get(cid),
            'floor': r.get('floor'),
            'verify': r.get('verify'),
            'statement': r.get('statement'),
            'anchors': r.get('anchors') or {},
            'detects': r.get('detects') or [],
        })
    out = []
    for g, items in groups.items():
        items.sort(key=lambda x: (-(x['weight'] or 0), x['id']))
        out.append({'group': g, 'weight': round(sum((i['weight'] or 0) for i in items), 4),
                    'items': items})
    out.sort(key=lambda g: -g['weight'])
    return out


RULEBOOK = rulebook()

def qa_pack(body, head):
    """Человеческая раскладка отчёта качества: пункты с весом/баллом/вкладом.
    Одна функция и для последнего отчёта, и для отчёта конкретного прогона."""
    ev = (body or {}).get('evaluation') or {}
    checks = ev.get('checks') or []
    total_w = sum((c.get('weight') or 0) for c in checks) or 1.0
    scored_w = sum((c.get('weight') or 0) for c in checks if c.get('score') is not None)
    rows = []
    for c in checks:
        w = c.get('weight') or 0.0
        sc = c.get('score')
        rule = (RULES.get('rules') or {}).get(c.get('id')) or None
        text = (c.get('violations') or c.get('evidence') or [''])[0][:700]
        rows.append({'id': c.get('id'), 'weight': w, 'score': sc, 'rule': rule,
                     'state': c.get('state'),
                     'share': round(100.0 * w / total_w, 2),
                     'contrib': None if sc is None else round(sc * w, 4),
                     'loss': None if sc is None else round((1.0 - sc) * w, 4),
                     'evidence': text, 'pieces': inline_pieces(text, head)})
    rows.sort(key=lambda r: (-r['weight'], r['id'] or ''))
    totals = {'weight': round(total_w, 4), 'scored_weight': round(scored_w, 4),
              'contrib_sum': round(sum((r['contrib'] or 0) for r in rows), 4),
              'loss_sum': round(sum((r['loss'] or 0) for r in rows), 4),
              'checks': len(checks),
              'unscored': sum(1 for c in checks if c.get('score') is None),
              'thirds': {'perfect': sum(1 for c in checks if c.get('score') == 1.0),
                         'mid': sum(1 for c in checks if c.get('score') not in (None, 1.0) and (c.get('score') or 0) >= 0.6),
                         'low': sum(1 for c in checks if c.get('score') is not None and c.get('score') < 0.6)},
              'headline': (body.get('human_view') or {}).get('headline') or {},
              'verdict': (body or {}).get('verdict'),
              'blocked_weight': ev.get('blocked_weight'),
              'max_blocked': ev.get('max_blocked_weight')}
    return rows, totals


if qual:
    for st in stages:
        if st['state'] == 'QA':
            rows, totals = qa_pack(qual, head)
            st['checks'] = rows
            st['rulebook'] = RULEBOOK
            st['profile'] = {'version': RULES.get('version'), 'theta': RULES.get('theta'),
                             'floors': RULES.get('floors'),
                             'blocked_max': RULES.get('blocked_max')}
            st['qa_totals'] = totals
if rev:
    for st in stages:
        if st['state'] == 'REVIEW':
            st['findings'] = [{'req': f.get('requirement_ref'), 'severity': f.get('severity'),
                               'text': f.get('description') or '',
                               'pieces': inline_pieces(f.get('description') or '', head)}
                              for f in (rev.get('findings') or [])]

total_tokens = 0
total_cost = 0.0
for st in stages:
    for e in st['executions']:
        u = e.get('usage') or {}
        total_tokens += u.get('total') or 0
        total_cost += u.get('cost') or 0.0

# Прогоны с их артефактами — по ним вкладки показывают ВЫБРАННЫЙ прогон.
for st in stages:
    st['runs'] = runs_of(st)

data = {
    'task': {'id': TASK, 'name': task.get('display'),
             'title': task['title'], 'state': task['state'],
             'display': task.get('display'), 'updated': task['updated_at'],
             'created': task['created_at'], 'position': (prog.get('pipeline') or {}).get('position')},
    'stages': stages,
    'tokens': {'total': total_tokens, 'cost': total_cost},
    'allowed': prog.get('allowed_operations') or [],
    # Исходная постановка — то, что человек написал при заведении задачи:
    # шаг «постановка» показывает её, а не служебные входы/выходы роли.
    'brief': task.get('business_description') or '',
    'convergence': CONV,
    'created': task.get('created_at'),
    'agent_failure': prog.get('agent_failure'),
    'attempt_limit': prog.get('attempt_limit'),
    'convergence': prog.get('convergence'),
    'spec_meta': out_arts.get('REQUIREMENTS_SPEC', {}).get('meta'),
    'artifacts': [{'kind': a['kind'], 'seq': a['seq'], 'status': a['status'],
                   'display': a.get('display'), 'size': a['size']} for a in arts],
    'head': head,
}
json.dump(data, open('/tmp/proto/detail.json', 'w', encoding='utf-8'), ensure_ascii=False)
print('стадии:', [(s['state'], s['status'], s['attempts']) for s in stages])
print('токенов всего:', total_tokens, 'стоимость:', round(total_cost, 2), '$')
print('вопросов платформы:', len(questions), '| спека:', len(spec.get('requirements', [])),
      'треб.,', len(spec.get('open_questions', [])), 'вопросов')
