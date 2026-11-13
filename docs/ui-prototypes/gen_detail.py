#!/usr/bin/env python3
"""Прототип экрана ЗАДАЧИ: вертикальный прогресс, один раскрытый шаг,
вход/выход протокола, вопросы рядом со своим пунктом, код вставками."""
import json
import re
from datetime import datetime, timezone

d = json.load(open('/tmp/proto/detail.json', encoding='utf-8'))
NOW = datetime.now(timezone.utc)
T = d['task']
ST = d['stages']

WORD = {'STATEMENT': 'постановка', 'ANALYSIS': 'анализ', 'REVIEW': 'ревью',
        'APPROVAL': 'подпись', 'IMPLEMENTATION': 'реализация', 'QA': 'приёмка'}
ROLE_WORD = {'statement-reviewer': 'постановщик', 'requirements-analyst': 'аналитик',
             'requirements-reviewer': 'ревьюер', 'implementation-agent': 'исполнитель',
             'quality-auditor': 'аудитор', 'quality-aggregator': 'сводка качества',
             'human': 'человек'}
STATE_WORD = {'DRAFT': 'черновик', 'STATEMENT': 'у постановщика', 'ANALYSIS': 'анализ',
              'REVIEW': 'ревью', 'APPROVAL': 'ждёт подписи', 'IMPLEMENTATION': 'код',
              'QA': 'приёмка', 'REWORK': 'доработка', 'WAITING_FOR_INPUT': 'ждёт человека',
              'PAUSED': 'на паузе', 'BLOCKED': 'остановлена', 'FAILED': 'упала',
              'ACCEPTED': 'принята', 'CANCELLED': 'отменена'}


def parse(ts):
    try:
        return datetime.fromisoformat(ts.replace('Z', '+00:00')) if ts else None
    except Exception:
        return None


def age(ts):
    x = parse(ts)
    if not x:
        return ''
    s = int((NOW - x).total_seconds())
    if s < 60:
        return f'{s} с'
    m = s // 60
    if m < 60:
        return f'{m} мин'
    h = m // 60
    return f'{h} ч {m % 60:02d} мин' if h < 24 else f'{h // 24} д {h % 24} ч'


def dur(a, b):
    x, y = parse(a), parse(b)
    if not x or not y:
        return ''
    s = int((y - x).total_seconds())
    return f'{s} с' if s < 90 else (f'{s // 60} мин' if s < 5400 else f'{s // 3600} ч {s % 3600 // 60:02d} м')


def conv_model():
    """Сходимость для окна: стадии и их круги. score/theta/delta — доли,
    None остаётся None: неизмеренный круг честно остаётся неизмеренным."""
    raw = (d.get('convergence') or {})
    out = []
    for st in raw.get('stages') or []:
        rounds = []
        for r in st.get('series') or []:
            def m(key):
                v = r.get(key)
                return None if v is None else v / 1000.0
            rounds.append({
                'r': r.get('round'),
                's': m('score_milli'),
                'th': m('theta_milli'),
                'd': m('delta_score_milli'),
                'reqs': r.get('requirements_count'),
                'oq': r.get('oq_count'),
                'mode': r.get('mode'),
            })
        out.append({'stage': st.get('stage'), 'rounds': rounds})
    return out


def main():
    if not n:
        return '0'
    if n >= 1e9:
        return f'{n / 1e9:.2f} млрд'
    if n >= 1e6:
        return f'{n / 1e6:.1f} млн'
    if n >= 1e3:
        return f'{n / 1e3:.0f} тыс'
    return str(n)


def money(x):
    return f'${x:,.2f}'.replace(',', ' ')


def stage_tokens(s):
    tot, cost, calls = 0, 0.0, 0
    for e in s['executions']:
        u = e.get('usage') or {}
        tot += u.get('total') or 0
        cost += u.get('cost') or 0.0
        calls += u.get('calls') or 0
    return tot, cost, calls


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


def split_proposal(text):
    m = re.split(r'(Предлагаю:?)', text, maxsplit=1)
    if len(m) == 3:
        return m[0].strip(), (m[1] + m[2]).strip()
    return text.strip(), ''


def step_icon(s):
    return {'passed': 'done', 'current': 'now', 'pending': 'todo'}.get(s['status'], 'todo')


model = []
for s in ST:
    tot, cost, calls = stage_tokens(s)
    ex = s['executions']
    first = ex[0]['started'] if ex else None
    last = ex[-1]['finished'] if ex else None
    entry = {
        'state': s['state'],
        'word': WORD.get(s['state'], s['state'].lower()),
        'role': s.get('role_slug'),
        'roleWord': ROLE_WORD.get(s.get('role_slug') or '', s.get('role_slug') or ''),
    'title': WORD.get(s['state'], s['state'].lower()),
        'version': s.get('role_version'),
        'label': s['label'],
        'gate': s['gate'],
        'status': s['status'],
        'statusWord': s.get('word'),
        'attempts': s['attempts'],
        'running': len(s.get('running') or []),
        'limit': ((d.get('attempt_limit') or {}).get('limit')
                  if (d.get('attempt_limit') or {}).get('role') == s.get('role_slug') else None),
        'tokens': tot, 'cost': cost, 'calls': calls,
        'span': (age(first) + ' → ' + age(last)) if first and last else '',
        'duration': dur(first, last) if first and last else '',
        'inContract': s.get('in_contract'),
        'outContract': s.get('out_contract'),
        'produced': [{'kind': a['kind'], 'seq': a['seq'], 'status': a['status'],
                      'size': a['size'], 'display': a.get('display')} for a in s.get('produced', [])],
        'execs': [{'id': e['id'][:8], 'status': e['status'], 'attempt': e.get('attempt'),
                   'summary': e.get('summary'), 'error': (e.get('error') or '')[:400],
                   'usage': e.get('usage')} for e in ex[-6:]],
        # ВСЕ прогоны шага: по ним рисуется полоса засечек и детали выбранного
        'executions': s.get('runs') or [{'id': e['id'], 'status': e['status'],
                        'attempt': e.get('attempt'), 'started': e.get('started'),
                        'finished': e.get('finished'), 'summary': e.get('summary'),
                        'error': (e.get('error') or '')[:400], 'usage': e.get('usage')}
                        for e in ex],
        'kind': 'gate' if s['gate'] else 'role',
        'body': {},
    }
    if s['state'] == 'STATEMENT':
        entry['body']['brief'] = {'text': d.get('brief') or '', 'created': d.get('created')}
    if s['state'] == 'ANALYSIS' and s.get('spec'):
        spec = s['spec']
        by_req, common = req_oq(spec)
        entry['body']['spec'] = {
            'paragraph': spec.get('human_paragraph') or '',
            'requirements': [{'id': r.get('id'), 'statement': r.get('statement') or '',
                              'ac': r.get('acceptance_criteria') or [],
                              'oq': [{'text': q} for q in by_req.get(r.get('id'), [])]}
                             for r in spec.get('requirements') or []],
            'common': common,
            'assumptions': spec.get('assumptions') or [],
            'unknowns': spec.get('unknowns') or [],
        }
    if s['state'] == 'REVIEW' and s.get('review'):
        rev = s['review']
        entry['body']['review'] = {
            'verdict': rev.get('verdict'),
            'paragraph': rev.get('human_paragraph') or '',
            'findings': [{'req': f.get('req') or f.get('requirement_ref'),
                          'severity': f.get('severity'),
                          'text': f.get('text') or f.get('description') or '',
                          'codes': f.get('codes') or []}
                         for f in (s.get('findings') or rev.get('findings') or [])][:14],
        }
    if s['state'] == 'IMPLEMENTATION' and s.get('worktree'):
        entry['body']['worktree'] = s['worktree']
    if s['state'] == 'IMPLEMENTATION' and s.get('evidence'):
        entry['body']['evidence'] = s['evidence']
    if s['state'] == 'IMPLEMENTATION' and s.get('impl'):
        impl = s['impl']
        entry['body']['impl'] = {
            'paragraph': impl.get('human_paragraph') or '',
            'branch': impl.get('branch'), 'commit': (impl.get('commit') or '')[:10],
            'coverage': [{'req': c.get('req'), 'text': c.get('text'),
                          'status': c.get('status'), 'codes': c.get('codes') or []}
                         for c in (s.get('coverage') or [])],
            'tests': impl.get('test_results') or {},
            'notes': (impl.get('notes') or '')[:1200],
        }
    if s.get('rulebook'):
        entry['body']['rulebook'] = s['rulebook']
        entry['body']['profile'] = s.get('profile') or {}
    if s['state'] == 'QA' and s.get('quality'):
        q = s['quality']
        # вклад каждого пункта: вес, балл, вклад в S и потеря — из данных отчёта
        entry['body']['qa'] = {
            'verdict': q.get('verdict'),
            'basis': q.get('recommendation_basis') or '',
            'checks': s.get('checks') or [],
            'totals': s.get('qa_totals') or {},
            'rulebook': s.get('rulebook') or [],
            'profile': s.get('profile') or {},
            'report': (q.get('report_text') or '')[:4000],
        }
    model.append(entry)

# «спецификация» на гейтах: подпись и приёмка судят по той же спеке
_spec = next((e['body'].get('spec') for e in model if e['body'].get('spec')), None)
if _spec:
    for e in model:
        if e['kind'] == 'gate' and 'spec' not in e['body']:
            e['body']['spec'] = _spec

# главный показатель качества: балл профиля в процентах
_qa = next((e['body']['qa'] for e in model if e['body'].get('qa')), None)
SCORE = None
THETA = None
SCORE_NOTE = ''
SCORE_CLS = ''
if _qa:
    _hl = (_qa.get('totals') or {}).get('headline') or {}
    SCORE = _hl.get('score')
    THETA = _hl.get('theta')
    SCORE_NOTE = _hl.get('verdict') or ''
    _v = (SCORE_NOTE or '').lower()
    if SCORE is not None and THETA is not None and SCORE < THETA:
        SCORE_CLS = 'bad'
    elif 'готов' in _v and 'не ' not in _v:
        SCORE_CLS = 'ok'
    else:
        SCORE_CLS = 'wait'

# доступность кнопок шага — ИЗ СТАТУСА (allowed_operations), не из догадок
ALLOWED = d.get('allowed') or []
for e in model:
    slug = e.get('role')
    run_op = next((o for o in ALLOWED if o == f'run-role-execution({slug})'), None)
    if not run_op and 'run' in ALLOWED and e['status'] in ('current', 'passed'):
        run_op = 'run'
    e['can_run'] = bool(run_op) and not e['gate']
    e['run_op'] = run_op or ''
    e['can_stop'] = bool(e.get('running')) and not e['gate']
    if e['running']:
        e['reason'] = ''
    elif e['can_run']:
        e['reason'] = ''
    elif e['gate']:
        e['reason'] = 'решает человек'
    elif e['status'] == 'passed':
        e['reason'] = 'шаг пройден'
    else:
        e['reason'] = 'платформа пока не разрешает запуск'

total = d['tokens']['total']
cost = d['tokens']['cost']
attempts_all = sum(e['attempts'] for e in model)
cur = next((i for i, e in enumerate(model) if e['status'] == 'current'), len(model) - 1)

html = """<!doctype html>
<html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>__H1__ · прототип экрана задачи</title>
<style>
:root{--bg:#0c0f13;--card:#141a21;--card2:#10151b;--inset:#0b0e12;--line:#242e39;
 --ink:#e9eef4;--dim:#8b97a6;--dim2:#5b6673;--pass:#3ecf8e;--now:#f5b544;
 --stop:#f0616d;--human:#7aa7ff;--mono:ui-monospace,SFMono-Regular,Menlo,monospace}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);overflow-x:hidden;
 font:14px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif}
header{position:sticky;top:0;z-index:20;background:rgba(12,15,19,.95);
 backdrop-filter:blur(10px);border-bottom:1px solid var(--line);padding:14px 26px 12px;
 display:grid;grid-template-columns:1fr auto;gap:20px;align-items:start}
h1{margin:0;font-size:18px;font-weight:600;line-height:1.25;max-width:74ch}
.meta{margin-top:6px;display:flex;gap:12px;align-items:center;flex-wrap:wrap;
 font-size:11px;color:var(--dim2)}
.meta .id{font-family:var(--mono)}
.meta .sep{opacity:.4}
.hero{display:flex;gap:10px;align-items:flex-start}
.badge{font-size:11px;font-weight:650;padding:3px 9px;border-radius:6px;border:1px solid transparent;white-space:nowrap}
.b-now{color:var(--now);background:rgba(245,181,68,.1);border-color:rgba(245,181,68,.3)}
.b-stop{color:var(--stop);background:rgba(240,97,109,.1);border-color:rgba(240,97,109,.3)}
.b-human{color:var(--human);background:rgba(122,167,255,.1);border-color:rgba(122,167,255,.3)}
.b-pass{color:var(--pass);background:rgba(62,207,142,.1);border-color:rgba(62,207,142,.3)}
.b-mute{color:var(--dim);background:#1a222b;border-color:var(--line)}
.runchip{display:flex;align-items:center;gap:6px;font-size:11px;padding:2px 9px;border-radius:6px;
 border:1px solid var(--line);color:var(--dim);white-space:nowrap}
.runchip i{width:6px;height:6px;border-radius:50%;background:#39434f}
.runchip.run{color:var(--pass);border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.08)}
.runchip.run i{background:var(--pass);animation:blink 1.3s infinite}
.runchip.stop{color:var(--stop);border-color:rgba(240,97,109,.35);background:rgba(240,97,109,.08)}
.runchip.stop i{background:var(--stop)}
.runchip.warn{color:var(--now);border-color:rgba(245,181,68,.35);background:rgba(245,181,68,.08)}
.runchip.warn i{background:var(--now)}
@keyframes blink{50%{opacity:.2}}
.tokens{text-align:right;min-width:180px}
.tokens .n{font-size:26px;font-weight:650;letter-spacing:-.4px;line-height:1}
.tokens .n small{font-size:12px;font-weight:500;color:var(--dim);letter-spacing:0}
.tokens .sub{font-size:11px;color:var(--dim2);margin-top:5px}
.bar{display:flex;height:6px;border-radius:3px;overflow:hidden;margin-top:8px;width:180px;margin-left:auto}
.bar i{display:block;height:100%}

main{padding:16px 26px 80px;max-width:1400px}
.steps{position:relative;padding-left:0}
.step{position:relative;display:grid;grid-template-columns:44px minmax(0,1fr);gap:14px}
.rail{position:absolute;left:21px;top:34px;bottom:-2px;width:2px;background:#1f2833}
.step.done .rail{background:linear-gradient(#2f9d6e,#1f4f3c)}
.step:last-child .rail{display:none}
.nodebox{display:flex;justify-content:center;padding-top:12px;position:relative;z-index:2}
.node{width:18px;height:18px;border-radius:50%;background:#171e26;border:2px solid #2f3a46;
 display:flex;align-items:center;justify-content:center;font-size:9px;color:#0c0f13}
.step.done .node{background:var(--pass);border-color:var(--pass)}
.step.now .node{width:24px;height:24px;margin-top:-3px;background:var(--now);border-color:#ffe0a8;
 box-shadow:0 0 0 5px rgba(245,181,68,.12),0 0 16px rgba(245,181,68,.45);animation:pulse 2s infinite}
.step.gate .node{border-radius:3px;transform:rotate(45deg)}
.step.gate .node span{transform:rotate(-45deg)}
@keyframes pulse{0%,100%{box-shadow:0 0 0 5px rgba(245,181,68,.12),0 0 14px rgba(245,181,68,.3)}
 50%{box-shadow:0 0 0 9px rgba(245,181,68,.04),0 0 20px rgba(245,181,68,.5)}}

.card{background:var(--card);border:1px solid var(--line);border-radius:10px;
 margin-bottom:8px;overflow:hidden;min-width:0}
.code .cap span:first-child{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:75%}
.head{display:grid;grid-template-columns:1fr auto;gap:14px;align-items:center;
 padding:9px 16px;cursor:pointer;user-select:none}
.head:hover{background:#172028}
.t1{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.role{font-size:13.5px;font-weight:600}
.ver{font-family:var(--mono);font-size:10.5px;color:var(--dim2)}
.chip{font-size:10.5px;color:var(--dim);border:1px solid var(--line);border-radius:5px;
 padding:1px 7px;white-space:nowrap;font-variant-numeric:tabular-nums}
.chip.hot{color:var(--now);border-color:rgba(245,181,68,.3)}
.chip-score{display:inline-flex;align-items:baseline;gap:5px;font-size:14px;font-weight:700;
 font-variant-numeric:tabular-nums;padding:2px 9px;border-radius:6px;border:1px solid;line-height:1.1}
.chip-score i{font-size:9.5px;font-weight:500;font-style:normal;letter-spacing:.06em;text-transform:uppercase;opacity:.75}
.chip-score.ok{color:var(--pass);border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.08)}
.chip-score.wait{color:var(--now);border-color:rgba(245,181,68,.35);background:rgba(245,181,68,.08)}
.chip-score.bad{color:var(--stop);border-color:rgba(240,97,109,.35);background:rgba(240,97,109,.08)}
.chip.bad{color:var(--stop);border-color:rgba(240,97,109,.3)}
.chip.good{color:var(--pass);border-color:rgba(62,207,142,.3)}
.t2{display:flex;align-items:center;gap:12px;font-size:11px;color:var(--dim2);
 font-variant-numeric:tabular-nums}
.act{font:inherit;font-size:11px;padding:3px 10px;border-radius:6px;cursor:pointer;
 border:1px solid var(--line);background:#172028;color:#cfd8e2;white-space:nowrap;transition:.12s}
.act.run{border-color:rgba(62,207,142,.45);color:#8fe3ba;background:rgba(62,207,142,.08)}
.act.run:hover{background:rgba(62,207,142,.16);border-color:var(--pass)}
.act.stop{border-color:rgba(240,97,109,.45);color:#ffb3bb;background:rgba(240,97,109,.08)}
.act.stop:hover{background:rgba(240,97,109,.16);border-color:var(--stop)}
.act.off{opacity:.45;cursor:not-allowed;border-style:dashed}
.act.busy{opacity:.8;cursor:progress}
.act-why{font-size:10px;color:var(--dim2);margin-left:6px}
.caret{color:var(--dim2);transition:transform .15s}
.step.open .caret{transform:rotate(90deg)}
.body{display:none;border-top:1px solid var(--line);padding:0}
.step.open .body{display:block}

.tabs{display:flex;gap:6px;padding:10px 16px;border-bottom:1px solid var(--line);background:var(--card2)}
.tab{font-size:11.5px;padding:3px 10px;border-radius:6px;border:1px solid var(--line);
 color:var(--dim);cursor:pointer}
.tab.on{color:var(--ink);background:#1b242e;border-color:#33414f}
.tab b{color:var(--now);font-weight:600}
.panel{display:none;padding:14px 16px 18px}
.panel.on{display:block}

.lead{color:#cfd8e2;font-size:13px;line-height:1.5;max-width:95ch}
.lead.clamp{display:-webkit-box;-webkit-line-clamp:4;-webkit-box-orient:vertical;overflow:hidden}
.more{font-size:11px;color:var(--human);cursor:pointer;margin-top:6px;display:inline-block}

.req{border:1px solid var(--line);border-radius:8px;padding:10px 12px;margin-bottom:8px;background:var(--card2)}
.req .rid{font-family:var(--mono);font-size:11px;color:var(--human)}
.req .st{margin-top:4px;font-size:12.5px;color:#dbe3ec;line-height:1.45}
.ac{margin:8px 0 0;padding-left:16px;color:var(--dim);font-size:12px}
.ac li{margin-bottom:3px}
.oq{margin-top:8px;border-left:2px solid var(--now);padding:4px 0 4px 10px;background:rgba(245,181,68,.04)}
.oq .q{font-size:12px;color:#e8dcc4}
.oq .p{font-size:11.5px;color:var(--now);margin-top:3px}
.sec{margin:14px 0 8px;font-size:10.5px;letter-spacing:.12em;text-transform:uppercase;color:var(--dim2)}
.ul{margin:6px 0 0;padding-left:16px;color:var(--dim);font-size:12px}

.code{margin:8px 0;border:1px solid var(--line);border-radius:8px;overflow:hidden;background:var(--inset)}
.code .cap{font-family:var(--mono);font-size:10.5px;color:var(--dim2);padding:5px 10px;
 border-bottom:1px solid var(--line);background:#0f1318;display:flex;justify-content:space-between}
.code pre{margin:0;padding:10px 12px;overflow:auto;font-family:var(--mono);font-size:11.5px;
 line-height:1.55;color:#c5d0dc}
.code pre .ln{color:#3d4a58;user-select:none;display:inline-block;width:2.4em}

.kv{display:grid;grid-template-columns:150px 1fr;gap:4px 14px;font-size:12px}
.kv .k{color:var(--dim2)}
.kv .v{color:#d3dce6;font-variant-numeric:tabular-nums}
.list{display:flex;flex-direction:column;gap:4px;margin-top:6px}
.item{display:flex;gap:8px;align-items:baseline;font-size:12px;color:var(--dim)}
.item .n{font-family:var(--mono);color:#9fb0c2;min-width:2.2em}
.item .t{color:#d3dce6}

/* ── приёмка: вклад каждого пункта ─────────────────────────── */
/* главный показатель: шапка и тело */
.conv{padding:0 26px;margin:0 0 6px;max-width:1400px}
.cvtitle{font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--dim2);margin:10px 0 8px}
.cvtitle span{letter-spacing:0;text-transform:none;color:var(--dim2);opacity:.75;margin-left:6px}
.cvrow{display:flex;align-items:center;gap:14px;padding:6px 12px;border:1px solid var(--line);
  border-radius:8px;background:var(--card);margin-bottom:6px}
.cvleft{display:flex;flex-direction:column;gap:2px;min-width:250px}
.cvlab{font-size:12px;color:var(--ink)}
.cvmeta{font-size:10.5px;color:var(--dim2);font-family:var(--mono)}
.cvbar{display:flex;align-items:center;gap:10px;flex:1;min-width:0}
.cvtrack{display:flex;align-items:flex-end;gap:2px;flex:1;min-width:0;height:22px;overflow:hidden}
.cvtrack{position:relative}
.cvline{position:absolute;left:0;right:0;height:0;border-top:1px dashed rgba(233,238,244,.35);pointer-events:none}
.cvtrack.dense .tk{flex:0 0 3px}
.tk{flex:0 0 5px;border-radius:1px 1px 0 0;height:22px;background:#2b3a4a}
.tk.ok{background:var(--pass)}
.tk.bad{background:var(--stop)}
.tk.un{height:9px;background:#2b3440}
.cvlast{font-family:var(--mono);font-size:13px;min-width:56px;text-align:right}
.cvlast.ok{color:var(--pass)} .cvlast.bad{color:var(--stop)} .cvlast.none{color:var(--dim2);font-size:11px}
.cvdelta{font-family:var(--mono);font-size:11px;min-width:56px;text-align:right}
.cvdelta.up{color:var(--pass)} .cvdelta.down{color:var(--stop)} .cvdelta.flat{color:var(--dim2)}
.cvmode{font-size:10.5px;padding:2px 7px;border-radius:999px;border:1px solid var(--line);color:var(--dim)}
.cvmode.up{color:var(--pass);border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.07)}
.cvmode.down{color:var(--stop);border-color:rgba(240,97,109,.35);background:rgba(240,97,109,.07)}
.humanmark{display:inline-flex;align-items:center;color:var(--human);opacity:.85;cursor:help}
.spark{display:inline-flex;align-items:flex-end;gap:1px;height:14px;margin-left:2px;cursor:help}
.spark i{width:3px;border-radius:1px 1px 0 0;background:#2b3a4a}
.spark i.ok{background:var(--pass)} .spark i.bad{background:var(--stop)}
.spark i.mid{background:#4a6b86} .spark i.un{background:#2b3440}
.spark.up{filter:saturate(1)} .spark.down i.ok,.spark.down i.mid{background:#7a5a3a}
/* путь шагов: круги с баллом и соединения */
.path{display:flex;align-items:flex-start;padding:14px 26px 4px;max-width:1400px}
.pstep{display:flex;flex-direction:column;align-items:center;gap:5px;cursor:pointer;min-width:76px}
.pc{width:42px;height:42px;border-radius:50%;display:flex;align-items:center;justify-content:center;
  font-family:var(--mono);font-size:13px;background:var(--card);border:2px solid #2b3a4a;color:var(--dim)}
.pc.ok{border-color:rgba(62,207,142,.65);color:var(--pass);background:rgba(62,207,142,.08)}
.pc.warn{border-color:rgba(245,181,68,.65);color:var(--now);background:rgba(245,181,68,.08)}
.pc.none{border-color:#2b3a4a;color:var(--dim2)}
.pstep.cur .pc{box-shadow:0 0 0 4px rgba(122,167,255,.16);border-color:var(--human)}
.pstep.todo .pl{color:var(--dim2)}
.pl{font-size:11.5px;color:var(--ink);text-align:center;white-space:nowrap}
.pm{font-size:10px;color:var(--dim2);font-family:var(--mono)}
.conn{flex:1;height:2px;background:#222c37;margin-top:21px;min-width:18px}
.conn.done{background:linear-gradient(90deg,#2f9d6e,#2b6b52)}
/* прогоны шага */
.runmemo{font-size:10.5px;color:var(--dim2);font-family:var(--mono);padding:7px 14px 0}
.runs{padding:10px 14px 0}
.rstrip{display:flex;align-items:center;gap:3px;flex-wrap:wrap}
.rlab{font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--dim2);margin-right:6px}
.rd{width:7px;height:14px;border-radius:2px;background:#2b3440;cursor:pointer;display:inline-block}
.rd.ok{background:var(--pass)} .rd.bad{background:var(--stop)} .rd.run{background:var(--now)}
.rd.sel{outline:2px solid var(--human);outline-offset:1px}
.rsel{font-family:var(--mono);font-size:10.5px;color:var(--dim2);margin-left:8px}
.rdet{display:flex;flex-wrap:wrap;align-items:center;gap:6px 14px;
  font-size:11.5px;margin:8px 0 2px;padding:7px 11px;background:var(--card2);border:1px solid var(--line);border-radius:8px}
.rdet .kv{font-family:var(--mono);color:#cfd8e2;display:inline-flex;gap:6px;align-items:baseline;white-space:nowrap}
.rdet .kv i{font-style:normal;font-size:9.5px;letter-spacing:.08em;text-transform:uppercase;color:var(--dim2);font-family:var(--sans,inherit)}
.rdet .kv.err{color:var(--stop);white-space:normal;flex-basis:100%}
.badge-status{font-size:10.5px;padding:2px 8px;border-radius:999px;border:1px solid var(--line)}
.b-pass{color:var(--pass);border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.07)}
.b-stop{color:var(--stop);border-color:rgba(240,97,109,.35);background:rgba(240,97,109,.07)}
.b-now{color:var(--now);border-color:rgba(245,181,68,.35);background:rgba(245,181,68,.07)}
.mstats{display:flex;align-items:center;gap:9px;font-family:var(--mono);font-size:11px;color:var(--dim2)}
.mstats b{color:var(--dim);font-weight:500}
.mstats .lim{color:#5a6877}
.mstats .dot{width:3px;height:3px;border-radius:50%;background:#39424e;display:inline-block}
.rolememo{font-family:var(--mono);font-size:10.5px;color:var(--dim2);padding:8px 14px 0}
.rolememo b{color:var(--dim);font-weight:500}
.cvvol{font-family:var(--mono);font-size:11.5px;color:var(--ink);white-space:nowrap}
.cvvol small{color:var(--dim2);font-size:10px}
.tk.mid{background:#4a6b86}
.tk.flat{background:#3a4a5a}
.cvtheta{font-size:10.5px;color:var(--dim2);font-family:var(--mono)}
.hero-score{text-align:right;min-width:150px;padding-left:6px;border-left:1px solid var(--line);margin-left:4px}
.hs-n{font-size:34px;font-weight:700;line-height:1;letter-spacing:-1px;color:var(--pass);font-variant-numeric:tabular-nums}
.hs-n.wait{color:var(--now)}
.hs-n.bad{color:var(--stop)}
.hs-p{font-size:18px;font-weight:600;color:var(--dim);margin-left:2px}
.hs-l{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--dim2);margin-top:3px}
.hs-s{font-size:10px;color:var(--now);margin-top:2px;max-width:170px;margin-left:auto}
.qhero{display:flex;align-items:center;gap:22px;background:linear-gradient(90deg,rgba(62,207,142,.07),transparent 70%);
 border:1px solid var(--line);border-radius:10px;padding:14px 18px;margin:2px 0 12px}
.qh-n{font-size:56px;font-weight:700;letter-spacing:-2px;line-height:1;color:var(--pass);font-variant-numeric:tabular-nums}
.qhero.wait .qh-n{color:var(--now)}
.qhero.bad .qh-n{color:var(--stop)}
.qhero.wait .qh-bar i{background:linear-gradient(90deg,#7a6236,#f5b544)}
.qhero.bad .qh-bar i{background:linear-gradient(90deg,#7a3a42,#f0616d)}
.qh-p{font-size:24px;color:var(--dim);font-weight:600;margin-left:3px}
.qh-l{font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--dim2)}
.qh-bar{position:relative;height:10px;border-radius:5px;background:#1b232c;margin:8px 0 6px;width:520px;max-width:60vw}
.qh-bar i{position:absolute;left:0;top:0;bottom:0;border-radius:5px;
 background:linear-gradient(90deg,#2f7d5c,#3ecf8e)}
.qh-bar b{position:absolute;top:-3px;width:2px;height:16px;background:var(--now);border-radius:1px}
.qh-sub{font-size:11px;color:var(--dim)}
.qsum{display:grid;grid-template-columns:repeat(5,1fr);gap:8px;margin:2px 0 12px}
.qcell{background:var(--card2);border:1px solid var(--line);border-radius:8px;padding:9px 11px}
.ql{font-size:10px;letter-spacing:.1em;text-transform:uppercase;color:var(--dim2)}
.qv{font-size:19px;font-weight:650;margin-top:3px;font-variant-numeric:tabular-nums}
.qv .qs{font-size:11px;color:var(--dim2);font-weight:400}
.qn{font-size:10.5px;color:var(--dim2);margin-top:2px}
.qhead{display:grid;grid-template-columns:24px 1fr 150px 52px 64px 64px;gap:8px;
 font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--dim2);padding:0 2px 5px}
.qitem{border-bottom:1px solid #161d25;padding:2px 0;border-left:2px solid transparent}
.qitem.picked{border-left-color:var(--stop);background:rgba(240,97,109,.04)}
.qitem.picked .qid{color:#e7c3c7}
.qitem:not(.picked){border-left-color:#2b3642;opacity:.72}
.qfree{font-size:9.5px;color:var(--dim2);border:1px dashed var(--line);border-radius:4px;padding:0 5px;margin-left:6px}
.qpick{width:18px;height:18px;padding:0;border-radius:4px;border:1px solid #33414f;background:#10151b;
 color:var(--stop);font-size:11px;line-height:1;cursor:pointer;display:flex;align-items:center;justify-content:center}
.qitem.picked .qpick{border-color:var(--stop);background:rgba(240,97,109,.16);color:#ffb6bd}
.qpick:hover{border-color:var(--stop)}
.qitem.picked .qpick{border-color:var(--stop);background:rgba(240,97,109,.15)}
.qpickbar{font-size:11px;color:var(--dim);margin-left:6px}
.qpickbar b{color:#e9eef4;font-variant-numeric:tabular-nums}
.qrow{display:grid;grid-template-columns:24px 1fr 150px 52px 64px 64px;gap:8px;align-items:center;
 padding:6px 2px 4px;font-size:12px;font-variant-numeric:tabular-nums;cursor:pointer;user-select:none}
.qrow:hover .qid{color:#fff}
.qcaret{display:inline-block;width:12px;color:var(--dim2);transition:transform .12s;font-size:9px}
.qitem.open .qcaret{transform:rotate(90deg);color:var(--ink)}
.qbody{display:none;padding:2px 0 8px 14px;border-left:2px solid #1c2632;margin:2px 0 4px 2px}
.qitem.open .qbody{display:block}
.qrow .qid{font-family:var(--mono);font-size:11.5px;color:#cfd8e2;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.qrow.ok .qscore{color:var(--pass)}
.qrow.mid .qscore{color:var(--now)}
.qrow.low .qscore{color:var(--stop)}
.qrow.un .qscore{color:var(--dim2)}
.qbar{display:flex;align-items:center;gap:7px}
.qbar em{position:relative;flex:1;height:7px;background:#222c37;border-radius:4px;font-style:normal}
.qbar em i{position:absolute;left:0;top:0;bottom:0;background:#5d8cc4;border-radius:4px}
.qrow.ok .qbar em i{background:#2f7d5c}
.qrow.un .qbar em i{background:#5b6673}
.qrow.low .qbar em i{background:#b0475a}
.qrow.mid .qbar em i{background:#a8802f}
.qbar span{font-size:10.5px;color:var(--dim2);min-width:34px;text-align:right;font-variant-numeric:tabular-nums}
.qcontrib{color:#d3dce6}
.qloss{color:var(--stop)}
.qrow.ok .qloss{color:var(--dim2)}
.qev{font-size:11.5px;color:#9aa7b5;padding:3px 2px 6px;line-height:1.5;max-width:140ch}
.qcode{padding:2px 2px 6px}
.qcode summary{font-size:10.5px;color:var(--human)}
.qs2{font-size:9.5px;color:var(--now);border:1px solid rgba(245,181,68,.3);border-radius:4px;padding:0 4px}
.brief{max-width:120ch}
.bh{font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--dim2);margin-bottom:8px}
.bt{font-size:13px;line-height:1.6;color:#d6dfe8;white-space:pre-wrap;background:var(--card2);
 border:1px solid var(--line);border-radius:8px;padding:12px 14px}
/* worktree: изменения против базы */
.wthead{display:flex;gap:14px;align-items:baseline;flex-wrap:wrap;font-size:11.5px;color:var(--dim);
 background:var(--card2);border:1px solid var(--line);border-radius:8px;padding:8px 11px;margin-bottom:10px}
.wtbranch{font-family:var(--mono);color:#cfd8e2}
.wtbase{font-family:var(--mono);color:var(--dim2)}
.wtstat b.add,.wtfile .num b.add{color:var(--pass)}
.wtstat b.del,.wtfile .num b.del{color:var(--stop)}
.wtfile{border:1px solid var(--line);border-radius:8px;margin-bottom:6px;background:var(--card2);overflow:hidden}
.wtfile summary{display:flex;gap:10px;align-items:center;padding:7px 11px;cursor:pointer;font-size:12px;list-style:none}
.wtfile summary::-webkit-details-marker{display:none}
.wtfile .st{width:16px;text-align:center;font-family:var(--mono);font-size:11px;border-radius:3px}
.wtfile .st.A{color:var(--pass)} .wtfile .st.M{color:var(--now)} .wtfile .st.D{color:var(--stop)}
.wtfile .path{flex:1;font-family:var(--mono);font-size:11.5px;color:#cfd8e2;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.wtfile .num{font-family:var(--mono);font-size:11px;color:var(--dim2)}
pre.diff{margin:0;padding:8px 0;background:var(--inset);border-top:1px solid var(--line);
 font-family:var(--mono);font-size:11.5px;line-height:1.5;overflow:auto;max-height:520px}
pre.diff .dl{display:block;padding:0 12px;white-space:pre}
pre.diff .add{background:rgba(62,207,142,.10);color:#b8e8cd}
pre.diff .del{background:rgba(240,97,109,.10);color:#f2bcc2}
pre.diff .hunk{color:var(--human);background:rgba(122,167,255,.07)}
pre.diff .meta{color:var(--dim2)}
pre.diff .fold{color:var(--human);background:rgba(122,167,255,.06);cursor:pointer;
  border-top:1px dashed rgba(122,167,255,.25);border-bottom:1px dashed rgba(122,167,255,.25);
  padding:2px 12px;font-size:11px;letter-spacing:.02em}
pre.diff .fold:hover{background:rgba(122,167,255,.13)}
.kv .v.okv{color:var(--pass)}
.ev{border:1px solid var(--line);border-radius:8px;margin-bottom:6px;background:var(--card2)}
.ev summary{display:flex;gap:10px;align-items:center;padding:7px 11px;cursor:pointer;list-style:none;font-size:12px}
.ev summary::-webkit-details-marker{display:none}
.ev .evn{flex:1;font-family:var(--mono);font-size:11.5px;color:#cfd8e2}
.ev .evm{font-family:var(--mono);font-size:11px;color:var(--pass)}
.ev .evm.zero{color:var(--stop)}
.ev .evt{font-size:10.5px;color:var(--dim2)}
pre.evlog{margin:0;padding:8px 12px;border-top:1px solid var(--line);background:var(--inset);
 font-family:var(--mono);font-size:11px;line-height:1.45;color:#b9c4d0;overflow:auto;max-height:260px;white-space:pre-wrap}
.implrep{margin-top:10px;border:1px solid var(--line);border-radius:8px;background:var(--card2);padding:8px 11px}
.implrep summary{cursor:pointer;font-size:11.5px;color:var(--dim);list-style:none}
.implrep summary::-webkit-details-marker{display:none}
.implrep summary:before{content:'▸ ';color:var(--dim2)}
.dtools{display:flex;gap:8px;align-items:center;padding:6px 11px;background:var(--inset);
 border-top:1px solid var(--line);font-size:11px;color:var(--dim2)}
.dtools button{background:transparent;border:1px solid var(--line);color:var(--human);
 border-radius:6px;padding:2px 8px;font-size:11px;cursor:pointer}
.dtools button:hover{background:rgba(122,167,255,.1)}
.rbhead{background:var(--card2);border:1px solid var(--line);border-radius:8px;padding:9px 11px;
 font-size:11.5px;color:#c3cedb;line-height:1.6;margin-bottom:10px}
.rbhead b{color:#fff}
.rbn{color:var(--dim2);font-size:10.5px}
.rbgroup{margin-bottom:12px}
.rbgt{font-size:11px;letter-spacing:.1em;text-transform:uppercase;color:var(--human)}
.rbgw{font-size:10px;color:var(--dim2);margin:2px 0 6px}
.rbrow{border:1px solid var(--line);border-radius:8px;padding:8px 11px;margin-bottom:6px;background:var(--card2)}
.rbid{font-family:var(--mono);font-size:11.5px;color:#cfd8e2}
.rbmeta{font-size:10.5px;color:var(--dim2);margin-top:1px;font-variant-numeric:tabular-nums}
.rbstate{font-size:12px;color:#cdd7e2;line-height:1.5;margin-top:5px;max-width:140ch}
.rbscale{margin-top:7px;display:flex;flex-direction:column;gap:2px}
.rbk{font-size:11px;color:var(--dim2)}
.rbk b{color:var(--now);font-family:var(--mono);margin-right:5px}
.rbdet{margin-top:5px;font-size:10.5px;color:var(--dim2)}
.qrule{background:#0f141a;border:1px solid var(--line);border-radius:8px;padding:9px 11px;margin:2px 0 8px}
.qr-h{display:flex;gap:10px;align-items:baseline;flex-wrap:wrap;font-size:10.5px;color:var(--dim2)}
.qr-g{color:var(--human);letter-spacing:.02em}
.qr-m{color:var(--dim2)}
.qr-floor{color:var(--stop)}
.qr-s{font-size:12px;color:#d7e0ea;line-height:1.5;margin-top:5px;max-width:130ch}
.qr-a{margin-top:7px;display:flex;flex-direction:column;gap:2px}
.qr-a-row{display:flex;gap:8px;font-size:11px;color:var(--dim2);padding:1px 0}
.qr-a-row.on{color:var(--ink);background:rgba(245,181,68,.07);border-left:2px solid var(--now);padding-left:6px;margin-left:-8px}
.qr-v{font-family:var(--mono);min-width:26px;color:inherit;opacity:.9}
.qr-a-row.on .qr-v{color:var(--now);font-weight:600}
.qr-d{margin-top:6px;font-size:10.5px;color:var(--dim2)}
.qtools{display:flex;align-items:center;gap:6px;margin:2px 0 8px}
.qtab{font-size:11px;padding:3px 10px;border-radius:6px;border:1px solid var(--line);color:var(--dim);cursor:pointer}
.qtab.on{color:var(--ink);background:#1b242e;border-color:#33414f}
.qhint{font-size:10.5px;color:var(--dim2);margin-left:auto}
.qformula{margin:0 0 10px}
.qformula summary{color:var(--human);font-size:11px}
.qf{font-size:11.5px;color:#9aa7b5;margin-top:6px;line-height:1.55;max-width:120ch;
 background:var(--card2);border:1px solid var(--line);border-radius:8px;padding:9px 11px}
.qf b{color:#d7e0ea;font-weight:600}
.empty{color:var(--dim2);font-size:12px}
details summary{cursor:pointer;color:var(--dim2);font-size:11.5px;margin-top:8px}
details[open] summary{color:var(--dim)}
.raw{margin-top:8px;background:var(--inset);border:1px solid var(--line);border-radius:8px;
 padding:10px;font-family:var(--mono);font-size:11px;color:#9fb0c2;white-space:pre;overflow:auto;max-height:340px}
.note{font-size:11px;color:var(--dim2);margin-top:6px}
/* подсветка кода — Prism, файлы лежат рядом (prism/) */
pre[class*=language-]{background:var(--inset)!important;margin:0;border-radius:0;
  border:1px solid var(--line);border-top:0;font-size:11.5px;line-height:1.55}
code[class*=language-]{font-family:var(--mono);font-size:11.5px;line-height:1.55;color:#c8d3df;text-shadow:none}
.code pre,.code code{font-size:12px!important;line-height:1.6!important}
.token.comment,.token.prolog,.token.doctype,.token.cdata{color:#5f7285;font-style:italic}
.token.punctuation{color:#8b9bb0}
.token.property,.token.tag,.token.constant,.token.symbol,.token.deleted{color:#f07178}
.token.boolean,.token.number{color:#f78c6c}
.token.selector,.token.attr-name,.token.string,.token.char,.token.builtin,.token.inserted{color:#c3e88d}
.token.operator,.token.entity,.token.url{color:#89ddff}
.token.atrule,.token.attr-value,.token.keyword{color:#c792ea}
.token.function,.token.class-name{color:#82aaff}
.token.regex,.token.important,.token.variable{color:#ffcb6b}
.line-numbers .line-numbers-rows{border-right:1px solid #1c242e;padding-right:6px}
.line-numbers-rows>span:before{color:#3d4a58}
</style>
<link rel="stylesheet" href="prism/prism.min.css">
<link rel="stylesheet" href="prism/line-numbers.min.css">
<script src="prism/prism-core.min.js"></script>
<script src="prism/prism-markup-templating.min.js"></script>
<script src="prism/prism-rust.min.js"></script>
<script src="prism/prism-typescript.min.js"></script>
<script src="prism/prism-json.min.js"></script>
<script src="prism/prism-bash.min.js"></script>
<script src="prism/prism-yaml.min.js"></script>
<script src="prism/prism-sql.min.js"></script>
<script src="prism/prism-ini.min.js"></script>
<script src="prism/prism-toml.min.js"></script>
<script src="prism/prism-line-numbers.min.js"></script>
<script>window.__errs=[];window.addEventListener('error',function(e){
  var t=(e.message||'')+' @'+(e.lineno||0)+':'+(e.colno||0);
  window.__errs.push(t); document.title='ERR '+t;
  var d=document.createElement('div'); d.id='err'; d.textContent='ERR '+t; d.style.cssText='position:fixed;bottom:0;left:0;right:0;background:#3a0d12;color:#ffb3bb;font:12px monospace;padding:8px;z-index:99';
  document.documentElement.appendChild(d);});</script>
</head><body>
<header>
  <div>
    <h1>__H1__</h1>
    <div class="meta">
      <span class="id">__ID__</span><span class="sep">·</span>
      <span>создана __CREATED__</span><span class="sep">·</span>
      <span>обновлена __AGE__ назад</span><span class="sep">·</span>
      <span>__NSTAGES__ шагов, прогонов __RUNS__</span>
    </div>
  </div>
  <div class="hero">
    <div style="display:flex;flex-direction:column;gap:6px;align-items:flex-end">
      <span class="badge __STATECLS__">__STATEWORD__</span>
      <span class="runchip __RUNCLS__"><i></i>__RUNWORD__</span>
    </div>
    <div class="hero-score">
      <div class="hs-n __SCORECLS__">__SCORE__<span class="hs-p">%</span></div>
      <div class="hs-l">качество</div>
      <div class="hs-s">__SCORENOTE__</div>
    </div>
    <div class="tokens" style="margin-left:18px">
      <div class="n">__TOKENS__ <small>токенов</small></div>
      <div class="sub">__COST__ · __CALLS__ вызовов модели</div>
      <div class="bar">__BAR__</div>
    </div>
  </div>
</header>
<main><div class="steps" id="s"></div></main>
<script>
const CONV = __CONV__;
const S = __DATA__;
const CUR = __CUR__;
const esc = s => String(s ?? '').replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
const fmtT = n => !n ? '0' : n >= 1e9 ? (n/1e9).toFixed(2)+' млрд'
  : n >= 1e6 ? (n/1e6).toFixed(1)+' млн' : n >= 1e3 ? Math.round(n/1e3)+' тыс' : String(n);

function code(c){
  if (!c || !c.code) return '';
  const LANG = {rs:'rust', ts:'typescript', vue:'markup', js:'javascript', json:'json',
    yaml:'yaml', yml:'yaml', sh:'bash', sql:'sql', toml:'toml', ini:'ini', md:'markdown'};
  const ext = (c.path||'').split('.').pop().toLowerCase();
  const lang = LANG[ext] || 'none';
  const n = (c.code.match(/^/gm)||[]).length;
  return `<div class="code"><div class="cap"><span>${esc(c.path)}:${c.line}</span>
    <span>${n} стр</span></div>
    <pre class="line-numbers" data-start="${c.line}"><code class="language-${lang}">${esc(c.code)}</code></pre></div>`;
}

function piecesHTML(pieces){
  if (!pieces || !pieces.length) return '';
  return `<div class="qev">${pieces.map(p => p.t === 'text'
    ? `<span>${esc(p.v)}</span>`.replace(/\s+$/,' ')
    : code({path:p.path, line:p.line, code:p.code})).join('')}</div>`;
}

function rulebookHTML(rb, profile){
  const p = profile || {};
  let h = `<div class="rbhead">правила приёмки · профиль <b>v${esc(p.version ?? '—')}</b>
    · порог <b>${p.theta != null ? Number(p.theta).toFixed(3) : '—'}</b>
    · полы: катастрофа ${p.floors && p.floors.catastrophic != null ? p.floors.catastrophic : '—'},
      значимое ${p.floors && p.floors.major != null ? p.floors.major : '—'},
      мелочь ${p.floors && p.floors.minor != null ? p.floors.minor : '—'}
    · вес блокеров ≤ ${p.blocked_max != null ? p.blocked_max : '—'}<br>
    <span class="rbn">эти правила — то, чем судили задачу; каждое даёт балл 0…1, вес даёт вклад в общий балл</span></div>`;
  (rb||[]).forEach(g=>{
    h += `<div class="rbgroup"><div class="rbgt">${esc(g.group)}</div>
      <div class="rbgw">суммарный вес ${g.weight != null ? Number(g.weight).toFixed(3) : '—'}</div>`;
    g.items.forEach(it=>{
      const anchors = it.anchors || {};
      const keys = ['1.0','0.7','0.4','0.0'].filter(k => anchors[k] != null);
      h += `<div class="rbrow">
        <div class="rbid">${esc(it.id)}</div>
        <div class="rbmeta">вес ${it.weight != null ? Number(it.weight).toFixed(3) : '—'}
          ${it.floor != null ? ` · пол ${it.floor}` : ''}
          ${it.verify ? ` · ${esc(it.verify)}` : ''}</div>
        <div class="rbstate">${esc(it.statement)}</div>
        ${keys.length ? `<div class="rbscale">${keys.map(k =>
          `<span class="rbk"><b>${k}</b> ${esc(anchors[k])}</span>`).join('')}</div>` : ''}
        ${(it.detects||[]).length ? `<div class="rbdet">ловит: ${(it.detects||[]).map(d=>esc(d.t)).join(', ')}</div>` : ''}
      </div>`;
    });
    h += '</div>';
  });
  return h;
}

function briefPanel(b){
  const created = b.created ? new Date(b.created).toLocaleDateString('ru-RU') : '';
  return `<div class="brief">
    <div class="bh">исходная постановка${created ? ' · создана ' + esc(created) : ''}</div>
    <div class="bt">${esc(b.text || 'постановки нет')}</div>
  </div>`;
}

function specPanel(b){
  const s = b.spec;
  let h = `<div class="lead clamp" id="lead">${esc(s.paragraph)}</div>
    <span class="more" onclick="document.getElementById('lead').classList.toggle('clamp');this.textContent=this.textContent.startsWith('весь')?'свернуть':'весь абзац'">весь абзац</span>`;
  h += `<div class="sec">требования · ${s.requirements.length}</div>`;
  s.requirements.forEach(r=>{
    h += `<div class="req"><div class="rid">${esc(r.id)}</div>
      <div class="st">${esc(r.statement)}</div>`;
    if (r.ac.length) h += `<ul class="ac">${r.ac.map(a=>`<li>${esc(a)}</li>`).join('')}</ul>`;
    (r.oq||[]).forEach(o=>{
      const [q, p] = splitP(o.text);
      h += `<div class="oq"><div class="q">${esc(q)}</div>${p?`<div class="p">${esc(p)}</div>`:''}</div>`;
    });
    h += '</div>';
  });
  if (s.common.length){
    h += `<div class="sec">вопросы без привязки к пункту · ${s.common.length}</div>
      <div class="note" style="margin:-2px 0 6px">в контракте спеки у вопроса нет поля «к какому пункту»:
      показываем рядом со списком требований, а не у пункта. Чтобы вопросы стояли у своих пунктов,
      контракт должен нести ссылку на требование.</div>`;
    s.common.forEach(o=>{ const [q,p]=splitP(o);
      h += `<div class="oq"><div class="q">${esc(q)}</div>${p?`<div class="p">${esc(p)}</div>`:''}</div>`; });
  }
  if (s.assumptions.length) h += `<details><summary>допущения · ${s.assumptions.length}</summary>
    <ul class="ul">${s.assumptions.map(a=>`<li>${esc(a)}</li>`).join('')}</ul></details>`;
  if (s.unknowns.length) h += `<details><summary>не проверено · ${s.unknowns.length}</summary>
    <ul class="ul">${s.unknowns.map(a=>`<li>${esc(a)}</li>`).join('')}</ul></details>`;
  return h;
}
function splitP(t){
  const i = t.indexOf('Предлагаю');
  return i < 0 ? [t, ''] : [t.slice(0,i).trim(), t.slice(i).trim()];
}

function reviewPanel(b){
  const r = b.review;
  let h = `<div class="kv"><div class="k">вердикт</div><div class="v">${esc(r.verdict)}</div></div>
    <div class="lead clamp" id="rlead" style="margin-top:10px">${esc(r.paragraph)}</div>
    <span class="more" onclick="document.getElementById('rlead').classList.toggle('clamp')">весь абзац</span>`;
  h += `<div class="sec">замечания · ${r.findings.length}</div>`;
  r.findings.forEach(f=>{
    h += `<div class="req"><div class="rid">${esc(f.req||'')} ${f.severity?`· ${esc(f.severity)}`:''}</div>
      <div class="st">${piecesHTML(f.pieces) || esc(f.text)}</div></div>`;
  });
  return h;
}

function implPanel(b){
  const i = b.impl;
  // Здесь только то, что платформа действительно знает: ветка и коммит,
  // проштампованные при публикации. «Что проверено», «чем закрыто требование»
  // и файлы улик убраны — их никто не читает и не сверяет, а рядом со словами
  // они выглядели как доказательство. Код смотри во вкладке worktree.
  let h = `<div class="wthead">
    <span class="wtbranch">${esc(i.branch || '—')}</span>
    <span class="wtbase">${esc((i.commit || '').slice(0, 8))}</span>
  </div>`;
  if (i.paragraph)
    h += `<details class="implrep"><summary>что исполнитель говорит о своей работе — его пересказ</summary>
      <div class="lead" style="margin-top:8px">${esc(i.paragraph)}</div></details>`;
  if (i.notes)
    h += `<details class="implrep"><summary>служебные заметки исполнителя</summary>
      <div class="lead" style="margin-top:8px;font-family:var(--mono);font-size:11px">${esc(i.notes)}</div></details>`;
  return h;
}

function qaChecks(list, mode){
  // «потери» — всё, что не 1.0 (и не оценённое), сильнее потеря — выше
  let rows = list.slice();
  if (mode === 'loss'){
    rows = rows.filter(c => c.score === null || c.score < 1);
    rows.sort((a,b) => (b.loss||0) - (a.loss||0) || (b.weight||0) - (a.weight||0));
  } else {
    rows.sort((a,b) => (b.weight||0) - (a.weight||0) || String(a.id).localeCompare(String(b.id)));
  }
  return rows;
}

const PICK = new Set();   // id пунктов, которые человек отправляет на исправление

function pickCount(){
  const marked = PICK.size;
  const el = document.getElementById('pickbar');
  if (el) el.innerHTML = `на доработку <b>${marked}</b> · снято с контроля <b>${TOTAL - marked}</b>`;
}

function togglePick(ev, id){
  ev.stopPropagation();
  if (PICK.has(id)) PICK.delete(id); else PICK.add(id);
  const item = ev.target.closest('.qitem');
  item.classList.toggle('picked', PICK.has(id));
  const box = item.querySelector('.qpick');
  box.textContent = PICK.has(id) ? '✕' : '';
  box.setAttribute('aria-checked', PICK.has(id) ? 'true' : 'false');
  // чип «снято с контроля» живёт в строке пункта
  const qid = item.querySelector('.qid');
  const chip = qid.querySelector('.qfree');
  if (PICK.has(id)) { if (chip) chip.remove(); }
  else if (!chip) { qid.insertAdjacentHTML('beforeend', ' <span class="qfree">снято с контроля</span>'); }
  pickCount();
}

function ruleHTML(rule, score, w){
  if (!rule) return '';
  const anchors = rule.anchors || {};
  const order = ['1.0','0.7','0.4','0.0'].filter(k => anchors[k] != null);
  const got = score == null ? null : String(score);
  return `<div class="qrule">
    <div class="qr-h"><span class="qr-g">${esc(rule.group)}</span>
      <span class="qr-m">проверка: ${esc(rule.verify||'—')}</span>
      <span class="qr-m">вес ${w!=null? w.toFixed(3):'—'}</span>
      ${rule.floor!=null? `<span class="qr-m qr-floor">пол ${rule.floor}</span>`:''}</div>
    <div class="qr-s">${esc(rule.statement)}</div>
    <div class="qr-a">${order.map(k=>`<div class="qr-a-row ${got && Number(k)===Number(got) ? 'on':''}">
      <span class="qr-v">${k}</span><span class="qr-t">${esc(anchors[k])}</span></div>`).join('')}</div>
    ${(rule.detects||[]).length? `<div class="qr-d">ловит: ${(rule.detects||[]).map(d=>esc(d.t)+(d.sev?` (${esc(d.sev)})`:'')).join(', ')}</div>`:''}
  </div>`;
}

function qaRows(list, mode){
  const rows = qaChecks(list, mode);
  if (!rows.length) return '<div class="empty">потерь нет: все пункты на 1.0</div>';
  const num = v => (v==null? '—' : (v>=0.01? v.toFixed(3) : v.toFixed(4)));
  let h = '';
  const maxW = rows.reduce((m, c) => Math.max(m, c.weight || 0), 0) || 1;
  rows.forEach(c=>{
    const cls = c.score==null? 'un' : c.score>=1? 'ok' : c.score>=0.6? 'mid' : 'low';
    const share = Math.max(100 * (c.weight || 0) / maxW, 1.5);
    const body = ruleHTML(c.rule, c.score, c.weight)
      + (piecesHTML(c.pieces) || (c.evidence? `<div class="qev">${esc(c.evidence)}</div>`:''));
    const picked = PICK.has(c.id);
    h += `<div class="qitem ${cls} ${picked?'picked':''}">
      <div class="qrow" onclick="toggleItem(this)">
        <button class="qpick" title="отправить на исправление"
          onclick="togglePick(event, '${esc(c.id)}')">${picked?'✕':''}</button>
        <div class="qid"><span class="qcaret">▶</span>${esc(c.id)}${c.state && c.state!=='SCORED'? ` <span class="qs2">${esc(c.state)}</span>`:''}${picked?'':' <span class="qfree">снято с контроля</span>'}</div>
        <div class="qbar"><em><i style="width:${share}%"></i></em><span>${num(c.weight)}</span></div>
        <div class="qscore">${c.score==null? '—' : c.score.toFixed(1)}</div>
        <div class="qcontrib">${c.contrib==null? '—' : c.contrib.toFixed(3)}</div>
        <div class="qloss">${c.loss==null? '—' : (c.loss>0? '−'+c.loss.toFixed(3) : '0')}</div>
      </div>
      <div class="qbody">${body}</div>
    </div>`;
  });
  return h;
}

function toggleItem(head){
  const item = head.closest('.qitem');
  const open = !item.classList.contains('open');
  // один развёрнутый пункт за раз: открыл следующий — предыдущий свернулся
  item.parentElement.querySelectorAll('.qitem.open').forEach(x=>x.classList.remove('open'));
  item.classList.toggle('open', open);
  if (open && window.Prism) Prism.highlightAllUnder(item);
}

function qaSwitch(el, mode){
  const box = el.closest('.qapanel');
  box.dataset.mode = mode;
  box.querySelectorAll('.qtab').forEach(t=>t.classList.toggle('on', t.dataset.m===mode));
  box.querySelector('.qlist').innerHTML = qaRows(JSON.parse(decodeURIComponent(box.dataset.checks)), mode);
  if (window.Prism) Prism.highlightAllUnder(box.querySelector('.qlist'));
}

function diffHTML(text){
  if (!text) return '<div class="empty">пустой diff</div>';
  const NL = String.fromCharCode(10);
  const lines = text.split(NL);
  // Классификация строк diff: шапка, кусок, добавлено, удалено, контекст.
  const clsOf = function(l){
    if (l.startsWith('+++') || l.startsWith('---')) return 'meta';
    if (l.startsWith('@@')) return 'hunk';
    if (l.startsWith('+')) return 'add';
    if (l.startsWith('-')) return 'del';
    if (l.startsWith('diff --git') || l.startsWith('index ') ||
        l.startsWith('new file') || l.startsWith('deleted file')) return 'meta';
    return 'ctx';
  };
  // Длинные куски неизменённого схлопываем: по 3 строки сверху и снизу,
  // остальное — «скрыто N строк», разворачивается кликом. Развернуть можно
  // ВЕСЬ файл: контекст пришёл целиком.
  const out = [];
  let i = 0;
  while (i < lines.length){
    const cls = clsOf(lines[i]);
    if (cls !== 'ctx'){ out.push({kind:'line', cls, text: lines[i]}); i++; continue; }
    let k = i;
    while (k < lines.length && clsOf(lines[k]) === 'ctx') k++;
    const run = lines.slice(i, k);
    if (run.length > 12){
      run.slice(0, 3).forEach(t => out.push({kind:'line', cls:'ctx', text:t}));
      out.push({kind:'hidden', n: run.length - 6, text: run.slice(3, run.length - 3)});
      run.slice(run.length - 3).forEach(t => out.push({kind:'line', cls:'ctx', text:t}));
    } else {
      run.forEach(t => out.push({kind:'line', cls:'ctx', text:t}));
    }
    i = k;
  }
  window.__DIFFS = window.__DIFFS || [];
  const id = window.__DIFFS.push(out) - 1;
  const hidden = out.filter(function(it){ return it.kind === 'hidden'; }).length;
  const tools = hidden
    ? '<div class="dtools" id="dt' + id + '">⋯ строк скрыто: ' + hidden +
      ' <button onclick="foldAll(' + id + ',1)">развернуть весь файл</button>' +
      ' <button onclick="foldAll(' + id + ',0)">свернуть</button></div>'
    : '';
  return tools + '<pre class="diff" data-diff="' + id + '">' + renderDiff(id, out) + '</pre>';
}

function renderDiff(id, items){
  return items.map(function(it, k){
    if (it.kind === 'hidden'){
      return '<span class="dl fold" data-d="' + id + '" data-k="' + k + '" onclick="unfold(this)">⋯ ' +
        it.n + ' строк скрыто · развернуть</span>';
    }
    return '<span class="dl ' + it.cls + '">' + (esc(it.text) || ' ') + '</span>';
  }).join('');
}

function foldAll(id, open){
  const pre = document.querySelector('pre.diff[data-diff="' + id + '"]');
  if (!pre) return;
  const items = window.__DIFFS[id];
  if (open){
    // разворачиваем весь файл: скрытые куски встают на своё место
    pre.innerHTML = items.map(function(it){
      if (it.kind === 'hidden')
        return it.text.map(function(t){ return '<span class="dl ctx">' + (esc(t) || ' ') + '</span>'; }).join('');
      return '<span class="dl ' + it.cls + '">' + (esc(it.text) || ' ') + '</span>';
    }).join('');
  } else {
    // сворачиваем обратно в те же «⋯ N строк скрыто»
    pre.innerHTML = renderDiff(id, items);
  }
}

function unfold(el){
  const id = Number(el.dataset.d);
  const k = Number(el.dataset.k);
  const items = window.__DIFFS[id];
  const run = items[k].text;
  el.outerHTML = run.map(t => '<span class="dl ctx">' + (esc(t) || ' ') + '</span>').join('');
}

function worktreePanel(w){
  if (!w) return '<div class="empty">изменения не записаны платформой</div>';
  const files = w.files || [];
  const add = files.reduce((n,f)=>n + (f.diff.match(/^\+(?!\+\+)/gm)||[]).length, 0);
  const del = files.reduce((n,f)=>n + (f.diff.match(/^-(?!--)/gm)||[]).length, 0);
  let h = '<div class="wthead">' +
    '<span class="wtbranch">' + esc(w.branch || '—') + '</span>' +
    '<span class="wtbase">' + esc((w.baseline||'').slice(0,8)) + ' → ' + esc((w.tip||'').slice(0,8)) + '</span>' +
    '<span class="wtstat"><b class="add">+' + add + '</b> <b class="del">−' + del + '</b> · файлов ' + files.length + '</span>' +
  '</div>';
  // по умолчанию открыт первый ИЗМЕНЁННЫЙ файл: у добавленного нечего разворачивать
  const openI = Math.max(0, files.findIndex(function(x){ return x.status === 'M'; }));
  files.forEach(function(f, i){
    const a = (f.diff.match(/^\+(?!\+\+)/gm)||[]).length;
    const d = (f.diff.match(/^-(?!--)/gm)||[]).length;
    h += '<details class="wtfile"' + (i === openI ? ' open' : '') + '>' +
      '<summary>' +
        '<span class="st ' + esc(f.status) + '">' + esc(f.status) + '</span>' +
        '<span class="path">' + esc(f.path) + '</span>' +
        '<span class="num"><b class="add">+' + a + '</b> <b class="del">−' + d + '</b></span>' +
      '</summary>' +
      (f.binary ? '<div class="empty">двоичный файл</div>' : diffHTML(f.full || f.diff)) +
    '</details>';
  });
  return h;
}

function qaPanel(b){
  const q = b.qa, t = q.totals || {}, hl = t.headline || {};
  const num = v => (v==null? '—' : (v>=0.01? v.toFixed(3) : v.toFixed(4)));
  const losses = (q.checks||[]).filter(c => c.score === null || c.score < 1).length;

  const S = hl.score, TH = hl.theta;
  const pct = (v) => (v == null ? '—' : (v * 100).toFixed(1));
  const gap = (S != null && TH != null) ? (S - TH) : null;
  const cls = (S == null) ? '' : (S < (TH ?? 0.8) ? ' bad' : (String(hl.verdict||'').startsWith('ВЕРДИКТ') && !String(hl.verdict||'').includes('НЕ ГОТОВ') ? ' ok' : ' wait'));
  let h = `<div class="qhero${cls}">
    <div class="qh-n">${pct(S)}<span class="qh-p">%</span></div>
    <div class="qh-side">
      <div class="qh-l">общая оценка качества</div>
      <div class="qh-bar"><i style="width:${S != null ? Math.min(100, S * 100) : 0}%"></i>
        <b style="left:${TH != null ? Math.min(100, TH * 100) : 80}%"></b></div>
      <div class="qh-sub">порог ${pct(TH)}%
        ${gap != null ? `· ${gap >= 0 ? 'выше на ' + (gap*100).toFixed(1) : 'ниже на ' + (-gap*100).toFixed(1)} пп` : ''}
        ${hl.verdict ? '· ' + hl.verdict.toLowerCase() : ''}</div>
    </div>
  </div>`;
  h += `<div class="qsum">
    <div class="qcell"><div class="ql">итоговый балл</div>
      <div class="qv">${S!=null? S.toFixed(3) : '—'}<span class="qs">/1.00</span></div>
      <div class="qn">порог ${TH!=null? TH.toFixed(3):'—'} · ${S!=null&&TH!=null?(S>=TH?'выше на '+(S-TH).toFixed(3):'ниже на '+(TH-S).toFixed(3)):'—'}</div></div>
    <div class="qcell"><div class="ql">сумма весов</div>
      <div class="qv">${num(t.weight)}</div>
      <div class="qn">оценено ${num(t.scored_weight)} · не оценено ${t.unscored??0}</div></div>
    <div class="qcell"><div class="ql">пунктов</div>
      <div class="qv">${t.checks??(q.checks||[]).length}</div>
      <div class="qn">${(t.thirds||{}).perfect??0} по 1.0 · ${(t.thirds||{}).mid??0} средних · ${(t.thirds||{}).low??0} слабых</div></div>
    <div class="qcell"><div class="ql">сумма вклада</div>
      <div class="qv">${num(t.contrib_sum)}</div>
      <div class="qn">потеряно ${num(t.loss_sum)}</div></div>
    <div class="qcell"><div class="ql">вес блокеров</div>
      <div class="qv">${num(t.blocked_weight)}</div>
      <div class="qn">предел ${num(t.max_blocked)}</div></div>
  </div>`;

  // формула — своими словами, коротко
  h += `<details class="qformula"><summary>как из этих цифр получается балл</summary>
    <div class="qf">каждая проверка ставит <b>балл 0…1</b> и имеет <b>вес</b>. Вклад = балл × вес.
    Итоговый балл S = сумма вкладов / сумма весов <b>оценённых</b> проверок: непроверенные и
    заблокированные выпадают из суммы, а веса нормируются заново (поэтому 40 пунктов с весами
    до 1.000 дают S=0.902 при сумме вкладов 0.857 — делим на 0.950, а не на 1.000).<br>
    Вердикт считается по порядку, первое сработавшее правило выигрывает:
    шлюз не пройден → возврат; шлюз не выполнен, нет подписи человека или вес блокеров больше
    предела (0.200) → «не решено»; пробита красная линия (пол) или S ниже порога 0.800 → возврат;
    иначе — приёмка.</div></details>`;

  h += `<div class="qapanel" data-mode="loss" data-checks="${encodeURIComponent(JSON.stringify(q.checks||[]))}">
    <div class="qtools">
      <span class="qtab on" data-m="loss" onclick="qaSwitch(this,'loss')">только потери · ${losses}</span>
      <span class="qtab" data-m="all" onclick="qaSwitch(this,'all')">все · ${(q.checks||[]).length}</span>
      <span class="qpickbar" id="pickbar"></span>
      <span class="qhint">все отмечены → идут в доработку; снятая отметка = снято с контроля</span>
    </div>
    <div class="qhead"><span></span><span>пункт</span><span>важность</span><span>балл</span><span>вклад</span><span>потеря</span></div>
    <div class="qlist"></div>
  </div>`;

  if (q.basis) h += `<div class="note" style="margin-top:10px">основание: ${esc(q.basis)}</div>`;
  if (q.report) h += `<details><summary>машинный текст отчёта</summary><div class="raw">${esc(q.report)}</div></details>`;
  return h;
}

let TOTAL = 0;
function initQa(root){
  (root||document).querySelectorAll('.qapanel').forEach(box=>{
    const all = JSON.parse(decodeURIComponent(box.dataset.checks));
    TOTAL = all.length;
    // по умолчанию в доработку идёт ВСЁ: снимать отметку — решение человека
    if (!PICK.size) all.forEach(c => PICK.add(c.id));
    box.querySelector('.qlist').innerHTML = qaRows(all, box.dataset.mode||'loss');
    pickCount();
  });
}


function ioPanel(s, which){
  const c = which === 'in' ? s.inContract : s.outContract;
  let h = '<div class="kv">';
  if (c && c.name) h += `<div class="k">контракт</div><div class="v">${esc(c.name+'@'+(c.version||''))}</div>`;
  h += `<div class="k">роль</div><div class="v">${esc(s.role||'человек')}${s.version?'@v'+s.version:''}</div>`;
  if (which === 'in'){
    h += `<div class="k">получает</div><div class="v">${s.state==='ANALYSIS'?'постановку задачи':s.state==='REVIEW'?'спеку аналитика':s.state==='IMPLEMENTATION'?'утверждённую спеку и решения человека':s.state==='QA'?'отчёт реализации и изменение':'решение человека'}</div>`;
  } else {
    h += `<div class="k">отдаёт</div><div class="v">${s.produced.length?s.produced.map(p=>esc(p.display||p.kind)).join(', '):'ничего (решение человека)'}</div>`;
  }
  h += '</div>';
  if (s.produced.length && which === 'out'){
    h += `<div class="sec">артефакты стадии</div><div class="list">${
      s.produced.map(p=>`<div class="item"><span class="t">${esc(p.display||p.kind)}</span>
        <span class="n">${esc(p.status)}</span><span class="n">${Math.round(p.size/1024)} КБ</span></div>`).join('')}</div>`;
  }
  h += `<div class="sec">прогоны · ${s.attempts}</div>`;
  if (!s.execs.length) h += '<div class="empty">прогонов не было — этот шаг решает человек</div>';
  else h += `<div class="list">${s.execs.map(e=>
    `<div class="item"><span class="n">#${e.attempt??''}</span>
      <span class="t">${esc(e.status)}</span>
      <span class="n">${e.usage? fmtT(e.usage.total)+' ток':''}</span>
      <span class="n">${esc((e.summary||'').slice(0,80))}</span></div>`).join('')}</div>`;
  if (s.execs.length && s.execs[s.execs.length-1].error)
    h += `<div class="code"><div class="cap"><span>последний отказ</span></div><pre>${esc(s.execs[s.execs.length-1].error)}</pre></div>`;
  return h;
}

function btn(s){
  // на пройденных и будущих шагах кнопки нет — там нечего запускать
  if (!s.can_run && !s.can_stop && s.status !== 'current') return '';
  if (s.can_stop)
    return `<button class="act stop" title="остановить прогон: платформа закроет исполнение"
      onclick="event.stopPropagation(); act(this,'stop')">■ остановить</button>`;
  if (s.can_run)
    return `<button class="act run" title="${esc(s.run_op)}"
      onclick="event.stopPropagation(); act(this,'run')">▶ запустить</button>`;
  const why = s.reason || 'недоступно';
  return `<button class="act off" disabled title="${esc(why)}">▶ запустить</button>
    <span class="act-why">${esc(why)}</span>`;
}

function act(el, kind){
  el.classList.add('busy');
  el.textContent = kind === 'run' ? 'запускаю…' : 'останавливаю…';
  setTimeout(()=>{ el.classList.remove('busy');
    el.textContent = kind === 'run' ? '▶ запустить' : '■ остановить'; }, 800);
}

function stageHTML(s, i){
  // Сходимость стадии — прямо в строке шага: последние круги как засечки.
  // У ревью мерится объём (требования/вопросы), у приёмки — балл.
  const conv = (function(){
    const key = s.state === 'REVIEW' ? 'spec_review' : s.state === 'QA' ? 'quality' : null;
    return key ? ((CONV || []).find(function(c){ return c.stage === key; }) || null) : null;
  })();
  function convSpark(){
    if (!conv || !conv.rounds.length) return '';
    const isSpec = conv.stage === 'spec_review';
    const all = conv.rounds;
    const shown = all.slice(-20);
    const vals = all.map(function(r){ return isSpec ? r.reqs : r.s; }).filter(function(v){ return v !== null && v !== undefined; });
    const max = vals.length ? Math.max.apply(null, vals) : 1;
    const theta = isSpec ? null : ((all.find(function(r){ var v = r.th; return v !== null && v !== undefined; }) || {}).th ?? null);
    const mode = MODE_WORD[all[all.length - 1].mode] || MODE_WORD.UNKNOWN;
    let bars = '';
    shown.forEach(function(r){
      const v = isSpec ? r.reqs : r.s;
      let cls = 'un', hgt = 4;
      if (v !== null && v !== undefined){
        if (isSpec) { cls = 'mid'; hgt = 4 + Math.round((v / (max || 1)) * 10); }
        else { cls = (theta !== null && v < theta) ? 'bad' : 'ok'; hgt = 4 + Math.round(v * 10); }
      }
      bars += '<i class="' + cls + '" style="height:' + hgt + 'px"></i>';
    });
    const r0 = all[0], rN = all[all.length - 1];
    const tip = isSpec
      ? 'сходимость по объёму: кругов ' + all.length + ', требований ' + (r0.reqs ?? '—') + ' → ' + (rN.reqs ?? '—') +
        ', вопросов ' + (r0.oq ?? '—') + ' → ' + (rN.oq ?? '—') + ' · ' + mode[0] +
        (all.length > 20 ? ' · показаны последние 20' : '')
      : 'сходимость по баллу: кругов ' + all.length + ', измерено ' + vals.length +
        (rN.s !== null && rN.s !== undefined ? ', последний ' + (rN.s * 100).toFixed(1) + '%' : '') +
        ' · ' + mode[0] + (all.length > 20 ? ' · показаны последние 20' : '');
    return `<span class="spark ${mode[1]}" title="${esc(tip)}">${bars}</span>`;
  }
  const cls = ['step', s.status === 'passed' ? 'done' : s.status === 'current' ? 'now' : 'todo',
               s.gate ? 'gate' : '', i === CUR ? 'open' : '', 'by-' + s.state].join(' ');
  const hot = s.status === 'current';
  // показатель качества в шапке шага: виден и когда шаг свёрнут
  let scoreChip = '';
  if (s.body && s.body.qa){
    const hl = ((s.body.qa.totals||{}).headline)||{};
    if (hl.score != null){
      const v = String(hl.verdict||'');
      const c = (hl.theta != null && hl.score < hl.theta) ? 'bad'
              : (v.includes('НЕ ГОТОВ') || !v ? 'wait' : 'ok');
      scoreChip = `<span class="chip-score ${c}">${(hl.score*100).toFixed(1)}%<i>качество</i></span>`;
    }
  }
  // Слева — что решает человек: оценка и «решает человек». Статус словами не
  // пишем: он уже в иконке слева, слова ушли в её подсказку. Всё измеримое —
  // справа одной группой: время, прогоны, токены, цена.
  const MAN = '<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><circle cx="8" cy="4.4" r="2.6" fill="currentColor"/><path d="M8 8.2c-2.9 0-5 1.7-5 4.2V15h10v-2.6c0-2.5-2.1-4.2-5-4.2Z" fill="currentColor"/></svg>';
  const chips = [scoreChip,
    s.gate ? `<span class="humanmark" title="решает человек: шаг ждёт подписи или ответа, а не прогона">${MAN}</span>` : ''
  ].join('');
  const stats = [
    s.duration ? `<span>${esc(s.duration)}</span>` : '',
    s.attempts ? `<span>прогонов <b>${s.attempts}</b></span>` : '',
    s.tokens ? `<span><b>${fmtT(s.tokens)}</b> ток</span>` : '',
    s.cost ? `<span><b>$${s.cost.toFixed(2)}</b></span>` : '',
    s.limit ? `<span class="lim">лимит ${s.limit}</span>` : '',
  ].filter(Boolean).join('<i class="dot"></i>');
  // Данные вкладок берём у ВЫБРАННОГО прогона: у каждого свой артефакт.
  // Нет артефакта у прогона — вкладка честно говорит, что он ничего не отдал.
  const selRun = (s.executions || [])[selOf(s)] || null;
  const art = selRun && selRun.art ? selRun.art : null;
  const eff = Object.assign({}, s.body);
  if (art) {
    if (art.spec) eff.spec = art.spec;
    if (art.review) eff.review = art.review;
    if (art.impl) eff.impl = art.impl;
    if (art.checks) eff.qa = Object.assign({}, s.body.qa || {}, {checks: art.checks, totals: art.qa_totals});
  }
  // У прогона есть засечка, но артефакта он не отдал — вкладок нет вовсе:
  // показывать чужие данные под выбранным прогоном было бы враньём.
  const noData = (s.executions || []).length > 0 && !art;
  const bodyKinds = noData ? [] : Object.keys(eff);
  const isGate = s.kind === 'gate';
  const tabs = [];
  // «вход/выход» убраны: служебные контракты роли ничего не объясняют человеку.
  // У шага показываем только то, по чему он судится или что он произвёл.
  if (bodyKinds.includes('brief')) tabs.push(['brief','постановка']);

  if (bodyKinds.includes('qa')) tabs.push(['qa','качество']);      // первая — открыта
  if (bodyKinds.includes('spec') || bodyKinds.includes('rulebook'))
    tabs.push(['spec', bodyKinds.includes('rulebook') ? 'спецификация приёмки'
      : (s.state === 'APPROVAL' ? 'что подписываешь' : 'спецификация')]);
  if (bodyKinds.includes('review')) tabs.push(['review','проверка']);
  if (bodyKinds.includes('impl')) tabs.push(['impl','реализация']);
  if (bodyKinds.includes('worktree')) tabs.push(['worktree','worktree']);
  // открытая по умолчанию — первая вкладка шага
  const firstTab = tabs.length ? tabs[0][0] : null;
  const panels = [];
  if (bodyKinds.includes('brief'))
    panels.push(`<div class="panel ${'brief'===firstTab?'on':''}" data-p="brief">${briefPanel(s.body.brief)}</div>`);

  if (bodyKinds.includes('qa')) panels.push(`<div class="panel ${'qa'===firstTab?'on':''}" data-p="qa">${qaPanel(eff)}</div>`);
  if (bodyKinds.includes('spec') || bodyKinds.includes('rulebook'))
    panels.push(`<div class="panel ${'spec'===firstTab?'on':''}" data-p="spec">${
      bodyKinds.includes('rulebook') ? rulebookHTML(eff.rulebook, eff.profile) : specPanel(eff)}</div>`);
  if (bodyKinds.includes('review')) panels.push(`<div class="panel ${'review'===firstTab?'on':''}" data-p="review">${reviewPanel(eff)}</div>`);
  if (bodyKinds.includes('impl')) panels.push(`<div class="panel ${'impl'===firstTab?'on':''}" data-p="impl">${implPanel(eff)}</div>`);
  if (bodyKinds.includes('worktree')) panels.push(`<div class="panel ${'worktree'===firstTab?'on':''}" data-p="worktree">${worktreePanel(s.body.worktree)}</div>`);
  return `<div class="${cls}" data-i="${i}">
    <div class="nodebox" title="${esc(s.statusWord||s.status)}"><div class="node"><span>${s.status==='passed'?'✓':''}</span></div>
      <div class="rail"></div></div>
    <div>
      <div class="card">
        <div class="head" onclick="toggle(${i})">
          <div class="t1"><span class="role">${esc(s.title)}</span>${convSpark()}${chips}</div>
          <div class="t2"><span class="mstats">${stats}</span>${btn(s)}<span class="caret">▶</span></div>
        </div>
        <div class="body">
          ${runsHTML(s)}
          <div class="runmemo">${art
            ? 'вкладки ниже — данные прогона ' + (selOf(s) + 1) + ' из ' + (s.executions || []).length + ' · ' + esc(art.display || art.kind)
            : 'прогон ' + (selOf(s) + 1) + ' из ' + (s.executions || []).length + ' ничего не отдал — вкладки недоступны'}</div>
          ${s.role ? `<div class="rolememo">роль <b>${esc(s.role)}</b>${s.version ? ' v' + s.version : ''}${s.in_contract ? ' · вход ' + esc(s.in_contract) : ''}${s.out_contract ? ' · выход ' + esc(s.out_contract) : ''}</div>` : ''}
          <div class="tabs">${tabs.map(([k,w])=>
            `<div class="tab ${k===firstTab?'on':''}" data-p="${k}" onclick="tab(event, ${i}, '${k}')">${w}</div>`).join('')}</div>
          ${panels.join('')}
        </div>
      </div>
    </div>
  </div>`;
}

// ── сходимость ────────────────────────────────────────────────────────────
// Одна строка на стадию: все раунды подряд, как они шли. Измеренный раунд —
// столбик выше порога (зелёный) или ниже (красный); неизмеренный — тонкая
// серая засечка: платформа балла не давала, и мы его не рисуем.
const STAGE_WORD = {spec_review: 'спека', quality: 'приёмка', rework: 'доработка'};
const MODE_WORD = {CONVERGING: ['рост', 'up'], DIVERGING: ['расхождение', 'down'],
                   PLATEAU: ['застой', 'flat'], UNKNOWN: ['нет данных', 'flat']};

function convHTML(){
  if (!CONV || !CONV.length) return '';
  let h = '<div class="cvtitle">сходимость <span>круги работы, слева направо</span></div>';
  CONV.forEach(function(st){
    const rounds = st.rounds || [];
    if (!rounds.length) return;
    const isSpec = st.stage === 'spec_review';
    // «Спека» сходится ОБЪЁМОМ: балла в отчёте ревью нет и контрактом он не
    // требуется — считаем, тянет ли спека к стабильности (требования и вопросы).
    // «Приёмка» сходится БАЛЛОМ: evaluation.score против порога theta.
    if (isSpec){
      const reqs = rounds.map(function(r){ return r.reqs; }).filter(function(v){ return v !== null && v !== undefined; });
      const maxReq = reqs.length ? Math.max.apply(null, reqs) : 1;
      let ticks = '';
      let prev = null;
      rounds.forEach(function(r){
        const v = (r.reqs === null || r.reqs === undefined) ? null : r.reqs;
        let cls = 'flat', hgt = 6;
        if (v !== null){
          hgt = 6 + Math.round((v / (maxReq || 1)) * 16);
          if (prev !== null && v < prev) cls = 'ok';
          else if (prev !== null && v > prev) cls = 'bad';
          else cls = 'mid';
        }
        const tip = 'круг ' + r.r + (v === null ? ' · требований нет данных' : ' · требований ' + v) +
          (r.oq !== null && r.oq !== undefined ? ' · вопросов ' + r.oq : '');
        ticks += '<span class="tk ' + (v === null ? 'un' : cls) + '" style="height:' + (v === null ? 9 : hgt) + 'px" title="' + esc(tip) + '"></span>';
        prev = v;
      });
      const r0 = rounds[0], rN = rounds[rounds.length - 1];
      const mode = MODE_WORD[rounds[rounds.length - 1].mode] || MODE_WORD.UNKNOWN;
      h += '<div class="cvrow">' +
        '<div class="cvleft"><span class="cvlab">спека</span>' +
          '<span class="cvmeta">кругов ' + rounds.length + ' · балла нет по контракту — смотрим объём</span></div>' +
        '<div class="cvbar"><div class="cvtrack' + (rounds.length > 50 ? ' dense' : '') + '">' + ticks + '</div>' +
          '<span class="cvvol">' + (r0.reqs ?? '—') + ' → ' + (rN.reqs ?? '—') + ' <small>требований</small></span>' +
          '<span class="cvvol">' + (r0.oq ?? '—') + ' → ' + (rN.oq ?? '—') + ' <small>вопросов</small></span>' +
          '<span class="cvmode ' + mode[1] + '">' + mode[0] + '</span>' +
        '</div></div>';
      return;
    }

    const measured = rounds.filter(function(r){ return r.s !== null; });
    const theta = (measured.find(function(r){ return r.th !== null; }) || {}).th ?? null;
    const last = measured[measured.length - 1] || null;
    const prevM = measured[measured.length - 2] || null;
    const delta = last && prevM ? last.s - prevM.s : null;
    const mode = MODE_WORD[rounds[rounds.length - 1].mode] || MODE_WORD.UNKNOWN;
    let ticks = '';
    rounds.forEach(function(r){
      const cls = r.s === null ? 'un' : (theta !== null && r.s < theta ? 'bad' : 'ok');
      const tip = 'круг ' + r.r + (r.s === null ? ' · не измерено' : ' · ' + (r.s * 100).toFixed(1) + '%');
      ticks += '<span class="tk ' + cls + '" title="' + esc(tip) + '"></span>';
    });
    h += '<div class="cvrow">' +
      '<div class="cvleft"><span class="cvlab">приёмка</span>' +
        '<span class="cvmeta">кругов ' + rounds.length + ' · измерено ' + measured.length + '</span></div>' +
      '<div class="cvbar"><div class="cvtrack' + (rounds.length > 50 ? ' dense' : '') + '">' +
        (theta !== null ? '<span class="cvline" style="bottom:' + (theta * 22).toFixed(1) + 'px"></span>' : '') +
        ticks + '</div>' +
        (last ? '<span class="cvlast ' + (theta !== null && last.s < theta ? 'bad' : 'ok') + '">' + (last.s * 100).toFixed(1) + '%</span>'
              : '<span class="cvlast none">балл не записан</span>') +
        (delta !== null
          ? '<span class="cvdelta ' + (delta > 0 ? 'up' : delta < 0 ? 'down' : 'flat') + '">' +
            (delta > 0 ? '↑ +' : delta < 0 ? '↓ ' : '→ ') + Math.abs(delta).toFixed(3) + '</span>'
          : '') +
        '<span class="cvmode ' + mode[1] + '">' + mode[0] + '</span>' +
        (theta !== null ? '<span class="cvtheta">порог ' + (theta * 100).toFixed(0) + '%</span>' : '') +
      '</div></div>';
  });
  return h;
}

function renderConv(){
  const el = document.getElementById('conv');
  const h = convHTML();
  el.innerHTML = h;
  el.style.display = h ? '' : 'none';
}

// ── путь шагов и прогоны ───────────────────────────────────────────────────
// Круги — шаги, внутри балл качества (или «—», если отчёта нет), цвет от балла
// против порога. Под кругами — сколько прогонов. Клик открывает шаг.
function scoreOfStage(s){
  if (s.body && s.body.qa){
    const hl = ((s.body.qa.totals||{}).headline)||{};
    if (hl.score != null) return {score: hl.score, theta: (hl.theta != null ? hl.theta : 0.8)};
  }
  return null;
}

function nodeCls(q){
  if (!q) return 'none';
  return q.score < q.theta ? 'warn' : 'ok';
}

function pathHTML(){
  return S.map(function(s, i){
    const q = scoreOfStage(s);
    const cls = !q ? 'none' : (q.score < q.theta ? 'warn' : 'ok');
    const num = q ? (q.score * 100).toFixed(0) : '—';
    const conn = i ? '<div class="conn ' + (S[i-1].status === 'passed' ? 'done' : '') + '"></div>' : '';
    return conn +
      '<div class="pstep ' + s.status + (s.state === CUR ? ' cur' : '') + '" onclick="gotoStep(' + i + ')">' +
        '<div class="pc ' + cls + '" title="' + esc(s.title) + (q ? ' · качество ' + (q.score*100).toFixed(1) + '% · порог ' + (q.theta*100).toFixed(0) + '%' : ' · оценки нет') + '">' + num + '</div>' +
        '<div class="pl">' + esc(s.title) + '</div>' +
        '<div class="pm">' + (s.attempts ? s.attempts + ' прогонов' : '—') + '</div>' +
      '</div>';
  }).join('');
}

function renderPath(){
  const el = document.getElementById('path');
  if (el) el.innerHTML = pathHTML();
}

function gotoStep(i){
  document.querySelectorAll('.step').forEach(function(el){
    el.classList.toggle('open', +el.dataset.i === i);
  });
  const el = document.querySelector('.step[data-i="' + i + '"]');
  if (el) el.scrollIntoView({block: 'center', behavior: 'smooth'});
}

// Выбранный прогон шага: по умолчанию ПОСЛЕДНИЙ.
const SEL = {};
function selOf(s){
  const n = (s.executions || []).length;
  if (!(s.state in SEL)) SEL[s.state] = n ? n - 1 : -1;
  return SEL[s.state];
}
function pick(state, k){
  SEL[state] = k;
  document.getElementById('s').innerHTML = S.map(stageHTML).join('');
  initQa(document.getElementById('s'));
  if (window.Prism) Prism.highlightAllUnder(document.getElementById('s'));
  const el = document.querySelector('.step.by-' + state);
  if (el) el.classList.add('open');
}

function dur(e){
  if (!e.started) return '—';
  const a = new Date(e.started).getTime();
  const b = e.finished ? new Date(e.finished).getTime() : Date.now();
  const m = Math.max(0, Math.round((b - a) / 60000));
  return m < 60 ? m + ' мин' : Math.floor(m / 60) + ' ч ' + (m % 60) + ' мин';
}
function when(e){
  return e.started ? new Date(e.started).toLocaleString('ru-RU', {day:'2-digit', month:'2-digit', hour:'2-digit', minute:'2-digit'}) : '—';
}
function fmtTok(n){
  if (n == null) return '—';
  if (n >= 1e6) return (n/1e6).toFixed(1) + ' млн';
  if (n >= 1e3) return Math.round(n/1e3) + ' тыс';
  return String(n);
}

function runsHTML(s){
  const ex = s.executions || [];
  if (!ex.length) return '<div class="empty" style="padding:2px 0">прогонов не было</div>';
  const sel = selOf(s);
  const dots = ex.map(function(e, k){
    const cls = e.status === 'COMPLETED' ? 'ok' : (e.status === 'FAILED' ? 'bad' : 'run');
    return '<span class="rd ' + cls + (k === sel ? ' sel' : '') +
      '" data-state="' + s.state + '" data-k="' + k + '"' +
      ' title="прогон #' + e.attempt + ' · ' + e.status + ' · ' + when(e) + '"></span>';
  }).join('');
  const e = ex[sel] || {};
  const u = e.usage || null;
  const arts = (s.produced || []).filter(function(a){ return a.execution_id === e.id; });
  const word = e.status === 'COMPLETED' ? 'дошёл до результата' : e.status === 'FAILED' ? 'упал' : 'идёт';
  let d = '<div class="runs">' +
    '<div class="rstrip"><span class="rlab">прогоны</span>' + dots +
      '<span class="rsel">выбран ' + (sel + 1) + ' из ' + ex.length + '</span></div>' +
    '<div class="rdet">' +
      '<span class="badge-status ' + (e.status === 'FAILED' ? 'b-stop' : e.status === 'RUNNING' ? 'b-now' : 'b-pass') + '">' + word + '</span>' +
      '<span class="kv"><i>попытка</i>№' + (e.attempt ?? '—') + '</span>' +
      '<span class="kv"><i>начат</i>' + when(e) + '</span>' +
      '<span class="kv"><i>длился</i>' + dur(e) + '</span>' +
      (u ? '<span class="kv"><i>токены</i>' + fmtTok(u.in) + ' вход · ' + fmtTok(u.out) + ' выход · <b>' + fmtTok(u.total) + '</b> · ' + u.calls + ' вызовов</span>' : '') +
      (u ? '<span class="kv"><i>цена</i>$' + (u.cost || 0).toFixed(3) + ' · ' + (u.model || '—') + '</span>' : '') +
      (arts.length ? '<span class="kv"><i>отдал</i>' + arts.map(function(a){ return esc(a.display || a.kind); }).join(', ') + '</span>' : '') +
      (e.error ? '<span class="kv err"><i>ошибка</i>' + esc(e.error) + '</span>' : '') +
      (e.summary ? '<span class="kv"><i>итог</i>' + esc(e.summary) + '</span>' : '') +
        '</div></div>';
  return d;
}

function toggle(i){
  document.querySelectorAll('.step').forEach(el=>{
    el.classList.toggle('open', +el.dataset.i === i ? !el.classList.contains('open') : false);
  });
}
function tab(ev, i, p){
  ev.stopPropagation();
  const step = document.querySelector(`.step[data-i="${i}"]`);
  const on = !step.querySelector(`.tab[data-p="${p}"]`).classList.contains('on');
  step.querySelectorAll('.tab').forEach(t=>t.classList.remove('on'));
  step.querySelectorAll('.panel').forEach(x=>x.classList.remove('on'));
  if (on){
    step.querySelector(`.tab[data-p="${p}"]`).classList.add('on');
    step.querySelector(`.panel[data-p="${p}"]`).classList.add('on');
  }
}
try{
  document.getElementById('s').innerHTML = S.map(stageHTML).join('');
  // Клик по засечке прогона: обработчик живёт на контейнере и переживает
  // перерисовку содержимого, поэтому регистрируется один раз.
  document.getElementById('s').addEventListener('click', function(ev){
    const dot = ev.target.closest('.rd');
    if (!dot) return;
    ev.stopPropagation();
    pick(dot.dataset.state, Number(dot.dataset.k));
  });
  initQa(document.getElementById('s'));
  if (window.Prism) Prism.highlightAllUnder(document.getElementById('s'));
  // #qa / #spec / #review — открыть нужный шаг и вкладку сразу (удобно для снимков)
  let want = (location.hash||'').replace('#','');
  const openCode = want.includes('code');
  want = want.replace('code','').replace(/[^a-z]/g,'');
  if (openCode) setTimeout(()=>{document.querySelectorAll('.step.open .qcode, .step.open details').forEach(d=>d.open=true);}, 30);
  if (want){
    const map = {qa:'qa', spec:'spec', rules:'rulebook', review:'review', impl:'impl',
                 brief:'brief', sign:'spec', wt:'worktree', worktree:'worktree'};
    const tabName = map[want] ? map[want] : null;
    let idx = tabName
      ? S.findIndex(s=>s.body && (s.body[tabName] || (tabName==='spec' && s.body.rulebook)))
      : -1;
    // #sign — шаг подписи: что именно подписываешь
    let panelName = tabName;
    if (want === 'sign'){
      idx = S.findIndex(s => s.state === 'APPROVAL');
      panelName = 'spec';
    }
    if (idx >= 0){
      document.querySelectorAll('.step').forEach(el=>el.classList.toggle('open', +el.dataset.i===idx));
      const st = document.querySelector(`.step[data-i="${idx}"]`);
      const panelKey = panelName === 'rulebook' ? 'spec' : (want === 'sign' ? 'spec' : panelName);
      st.querySelectorAll('.tab').forEach(t=>t.classList.toggle('on', t.dataset.p===panelKey));
      st.querySelectorAll('.panel').forEach(x=>x.classList.toggle('on', x.dataset.p===panelKey));
    }
  }
}catch(e){
  document.body.insertAdjacentHTML('beforeend','<pre style="color:#f0616d;padding:20px">ОШИБКА: '+e.message+' | '+e.stack+'</pre>');
}
</script></body></html>"""

def human(n):
    """Токены по-человечески: 45.3 млн, 228 тыс, 812."""
    n = n or 0
    if n >= 1_000_000:
        return f'{n / 1_000_000:.1f} млн'
    if n >= 1_000:
        return f'{n / 1_000:.0f} тыс'
    return str(n)


STATE_CLS = {'APPROVAL': 'b-human', 'WAITING_FOR_INPUT': 'b-human', 'BLOCKED': 'b-stop',
             'FAILED': 'b-stop', 'PAUSED': 'b-stop', 'REWORK': 'b-now', 'QA': 'b-pass',
             'DRAFT': 'b-mute'}
STATE_W = STATE_WORD.get(T['state'], T['state'].lower())
if T['state'] in ('APPROVAL', 'WAITING_FOR_INPUT'):
    RUN_CLS, RUN_W = 'warn', 'стоит · ждёт человека ' + age(T['updated'])
elif T['state'] in ('BLOCKED', 'FAILED', 'PAUSED'):
    af = d.get('agent_failure') or {}
    RUN_CLS, RUN_W = 'stop', 'стоит · ' + (af.get('class_word') or STATE_W) + ' ' + age(T['updated'])
elif T['state'] == 'DRAFT':
    RUN_CLS, RUN_W = 'warn', 'не запускалась'
else:
    RUN_CLS, RUN_W = 'stop', 'нет прогона · ' + age(T['updated'])

bar = ''
for e in model:
    if e['tokens']:
        pct = 100.0 * e['tokens'] / max(total, 1)
        col = {'ANALYSIS': '#7aa7ff', 'REVIEW': '#8f7aff', 'IMPLEMENTATION': '#3ecf8e',
               'QA': '#f5b544'}.get(e['state'], '#5b6673')
        bar += f'<i style="width:{pct:.2f}%;background:{col}"></i>'

html = (html
        # '</script' внутри данных (тексты отчётов, куски кода) обязан быть экранирован,
        # иначе HTML-парсер закрывает тег раньше времени
        .replace('__DATA__', json.dumps(model, ensure_ascii=False).replace('</', '<\\/'))
        .replace('__CONV__', json.dumps(conv_model(), ensure_ascii=False).replace('</', '<\\/'))
        .replace('__CUR__', str(cur))
        .replace('__H1__', (T.get('display') or T['title']).replace('<', '&lt;'))
        .replace('__ID__', T['id'])
        .replace('__CREATED__', T['created'][:10])
        .replace('__AGE__', age(T['updated']))
        .replace('__NSTAGES__', str(len(model)))
        .replace('__RUNS__', str(attempts_all))
        .replace('__TOKENS__', human(total))
        .replace('__COST__', money(cost))
        .replace('__CALLS__', str(sum(e['calls'] for e in model)))
        .replace('__BAR__', bar)
        .replace('__STATECLS__', STATE_CLS.get(T['state'], 'b-mute'))
        .replace('__STATEWORD__', STATE_W)
        .replace('__RUNCLS__', RUN_CLS)
        .replace('__RUNWORD__', RUN_W)
        .replace('__SCORECLS__', SCORE_CLS)
        .replace('__SCORE__', ('%.1f' % (SCORE * 100)) if SCORE is not None else '—')
        .replace('__SCORENOTE__', (SCORE_NOTE or 'оценки пока нет').lower()))

open('/tmp/proto/detail.html', 'w', encoding='utf-8').write(html)
print('detail.html:', len(html), 'байт; открыт шаг', cur, model[cur]['state'] if model else '')
