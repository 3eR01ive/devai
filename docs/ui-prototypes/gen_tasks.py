#!/usr/bin/env python3
"""Прототип v2 экрана «Список задач» на реальных данных живого дома."""
import json
from datetime import datetime, timezone

rows = json.load(open('/tmp/rows.json', encoding='utf-8'))
NOW = datetime.now(timezone.utc)

SHORT = {'STATEMENT': 'постановка', 'ANALYSIS': 'анализ', 'REVIEW': 'ревью',
         'APPROVAL': 'подпись', 'IMPLEMENTATION': 'код', 'QA': 'приёмка'}
LONG = {'STATEMENT': 'постановщик', 'ANALYSIS': 'аналитик', 'REVIEW': 'ревьюер',
        'APPROVAL': 'подпись человека', 'IMPLEMENTATION': 'реализация', 'QA': 'приёмка'}
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
    d = parse(ts)
    if not d:
        return ''
    s = int((NOW - d).total_seconds())
    if s < 60: return f'{s} с'
    m = s // 60
    if m < 60: return f'{m} мин'
    h = m // 60
    return f'{h} ч {m % 60:02d} мин' if h < 24 else f'{h // 24} д {h % 24} ч'

def elapsed(ts):
    d = parse(ts)
    if not d: return ''
    m = int((NOW - d).total_seconds() // 60)
    return f'{m} мин' if m < 60 else f'{m // 60} ч {m % 60:02d} мин'

items = []
for r in rows:
    stages, passed = [], 0
    for s in r['stages']:
        if s['status'] == 'passed':
            passed += 1
        stages.append({'state': s['state'], 'short': SHORT.get(s['state'], s['state'].lower()),
                       'long': LONG.get(s['state'], s['state'].lower()),
                       'gate': bool(s['gate']), 'status': s['status']})
    running = r['running'] or []
    if running:
        kind, word = 'run', 'идёт · ' + elapsed(running[0]['started'])
    elif r['state'] in ('APPROVAL', 'WAITING_FOR_INPUT'):
        kind, word = 'human', ('ждёт подписи' if r['state'] == 'APPROVAL' else 'ждёт ответа') + ' · ' + age(r['updated'])
    elif r['state'] in ('BLOCKED', 'FAILED', 'PAUSED'):
        kind, word = 'stop', {'BLOCKED': 'остановлена', 'FAILED': 'упала', 'PAUSED': 'на паузе'}[r['state']] + ' · ' + age(r['updated'])
    elif r['state'] in ('STATEMENT', 'ANALYSIS', 'REVIEW', 'IMPLEMENTATION', 'QA', 'REWORK'):
        # стадия живая, а прогона нет: именно этот случай раньше был невидим
        kind, word = 'warn', 'прогона нет · ' + age(r['updated'])
    else:
        kind, word = 'idle', 'прогона нет · ' + age(r['updated'])
    why = ''
    if r['state'] in ('BLOCKED', 'FAILED', 'PAUSED') and r.get('af_class'):
        why = r['af_class']
    items.append({'id': r['id'][:8], 'name': r['name'], 'state': r['state'],
                  'stateWord': STATE_WORD.get(r['state'], r['state'].lower()),
                  'stages': stages, 'passed': passed, 'total': len(stages),
                  'runner': {'kind': kind, 'word': word}, 'why': why,
                  'attempts': r.get('attempts'), 'attemptLimit': r.get('attempt_limit')})

GROUPS = [
    ('you', 'Ждут тебя', ['WAITING_FOR_INPUT', 'APPROVAL']),
    ('work', 'Идёт работа', ['STATEMENT', 'ANALYSIS', 'REVIEW', 'IMPLEMENTATION', 'QA', 'REWORK']),
    ('stop', 'Стоят', ['BLOCKED', 'FAILED', 'PAUSED']),
    ('draft', 'Черновики', ['DRAFT']),
]
grouped = []
for key, title, states in GROUPS:
    g = [i for i in items if i['state'] in states]
    if g:
        grouped.append({'key': key, 'title': title, 'items': g})

html = """<!doctype html>
<html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>devai · задачи (прототип v2)</title>
<style>
:root{--bg:#0c0f13;--card:#141a21;--card2:#10151b;--line:#242e39;
  --ink:#e9eef4;--dim:#8b97a6;--dim2:#5b6673;
  --pass:#3ecf8e;--now:#f5b544;--stop:#f0616d;--human:#7aa7ff}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);
  font:14px/1.35 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif}
header{position:sticky;top:0;z-index:9;background:rgba(12,15,19,.94);
  backdrop-filter:blur(8px);border-bottom:1px solid var(--line);padding:12px 24px 10px}
h1{margin:0;font-size:13px;font-weight:600;color:#c8d2de;letter-spacing:.3px}
h1 i{color:var(--dim2);font-style:normal;font-weight:400}
main{padding:10px 24px 60px}

.grp{position:sticky;top:57px;z-index:8;background:var(--bg);padding:14px 0 8px;
  display:flex;align-items:baseline;gap:10px}
.grp .t{font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--dim2)}
.grp .n{font-size:11px;color:var(--dim2);font-variant-numeric:tabular-nums}
.grp .bar{flex:1;height:1px;background:var(--line)}
.grp.you .t{color:var(--human)} .grp.stop .t{color:var(--stop)}
.grp.work .t{color:var(--now)} .grp.draft .t{color:var(--dim2)}

.row{display:grid;grid-template-columns:290px 1fr 190px;gap:16px;align-items:center;
  background:var(--card);border:1px solid var(--line);border-radius:10px;
  padding:10px 14px;margin-bottom:6px;position:relative;overflow:hidden;min-height:64px}
.row::before{content:"";position:absolute;left:0;top:0;bottom:0;width:3px;background:#37424f}
.row.s-WAITING_FOR_INPUT::before,.row.s-APPROVAL::before{background:var(--human)}
.row.s-BLOCKED::before,.row.s-FAILED::before,.row.s-PAUSED::before{background:var(--stop)}
.row.s-REWORK::before{background:var(--now)}
.row.s-QA::before{background:var(--pass)}
.row.flat{min-height:44px;padding:7px 14px}
.row.flat .tl{opacity:.5}

.name{font-size:13px;font-weight:550;line-height:1.25;display:-webkit-box;
  -webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden}
.flat .name{font-size:12.5px;color:#b9c4d1;-webkit-line-clamp:1}
.ident{font-family:ui-monospace,Menlo,monospace;font-size:10.5px;color:var(--dim2);
  margin-top:3px;display:flex;gap:8px;align-items:center}
.ident .why{color:var(--stop)}

.tl{display:flex;align-items:flex-start;position:relative;padding-top:6px}
.step{flex:1;display:flex;flex-direction:column;align-items:center;position:relative}
.step .rail{position:absolute;top:8px;left:50%;width:100%;height:2px;background:#212a34}
.step:last-child .rail{display:none}
.step.passed .rail{background:linear-gradient(90deg,#2f9d6e,var(--pass))}
.step.current .rail{background:linear-gradient(90deg,#2f9d6e 30%,#3a3325)}
.node{position:relative;z-index:2;width:16px;height:16px;border-radius:50%;
  background:#171e26;border:2px solid #2f3a46;display:flex;align-items:center;justify-content:center}
.node .tick{width:6px;height:6px;border-radius:50%;background:#111720}
.step.passed .node{background:var(--pass);border-color:var(--pass)}
.step.current .node{width:22px;height:22px;margin-top:-3px;background:var(--now);
  border-color:#ffe0a8;box-shadow:0 0 0 5px rgba(245,181,68,.13),0 0 16px rgba(245,181,68,.5);
  animation:pulse 2s infinite}
@keyframes pulse{0%,100%{box-shadow:0 0 0 5px rgba(245,181,68,.12),0 0 14px rgba(245,181,68,.35)}
  50%{box-shadow:0 0 0 9px rgba(245,181,68,.04),0 0 20px rgba(245,181,68,.55)}}
.step.gate .node{border-radius:2px;transform:rotate(45deg)}
.step.current.gate .node{margin-top:-3px}
.step .cap{margin-top:7px;font-size:10px;color:var(--dim2);white-space:nowrap;letter-spacing:.02em}
.step.passed .cap{color:#6f7d8b}
.step.current .cap{color:var(--now);font-weight:700}

.side{display:flex;flex-direction:column;align-items:flex-end;gap:5px}
.state{font-size:11.5px;font-weight:650;padding:2px 8px;border-radius:5px;border:1px solid transparent;white-space:nowrap}
.s-WAITING_FOR_INPUT .state,.s-APPROVAL .state{color:var(--human);background:rgba(122,167,255,.1);border-color:rgba(122,167,255,.28)}
.s-BLOCKED .state,.s-FAILED .state,.s-PAUSED .state{color:var(--stop);background:rgba(240,97,109,.1);border-color:rgba(240,97,109,.28)}
.s-REWORK .state{color:var(--now);background:rgba(245,181,68,.1);border-color:rgba(245,181,68,.28)}
.s-QA .state{color:var(--pass);background:rgba(62,207,142,.1);border-color:rgba(62,207,142,.28)}
.s-DRAFT .state{color:var(--dim);background:#1a222b;border-color:var(--line)}
.s-STATEMENT .state,.s-ANALYSIS .state,.s-REVIEW .state,.s-IMPLEMENTATION .state{
  color:#a9c8ff;background:rgba(122,167,255,.07);border-color:rgba(122,167,255,.18)}
.runchip{font-size:10.5px;display:flex;align-items:center;gap:6px;padding:2px 8px;
  border-radius:5px;border:1px solid var(--line);color:var(--dim);font-variant-numeric:tabular-nums;white-space:nowrap}
.runchip .dot{width:6px;height:6px;border-radius:50%;background:#39434f;flex:none}
.runchip.run{color:var(--pass);border-color:rgba(62,207,142,.35);background:rgba(62,207,142,.08)}
.runchip.run .dot{background:var(--pass);animation:blink 1.3s infinite}
.runchip.human{color:var(--human);border-color:rgba(122,167,255,.32);background:rgba(122,167,255,.08)}
.runchip.human .dot{background:var(--human)}
.runchip.stop{color:var(--stop);border-color:rgba(240,97,109,.32);background:rgba(240,97,109,.07)}
.runchip.stop .dot{background:var(--stop)}
.runchip.idle{color:#93a0ae;border-style:dashed}
.runchip.warn{color:var(--now);border-color:rgba(245,181,68,.35);background:rgba(245,181,68,.07)}
.runchip.warn .dot{background:var(--now)}
.tlc{display:flex;align-items:center;padding-left:4px}
.dots{display:flex;gap:14px;align-items:center;position:relative}
.dots .d{width:8px;height:8px;border-radius:50%;background:#1b232c;border:1px solid #2c3641}
.dots .d.gate{border-radius:2px;transform:rotate(45deg)}
.dots .d.passed{background:var(--pass);border-color:var(--pass)}
.dots .d.current{background:var(--now);border-color:#ffd48a}
.ghost{font-size:10.5px;color:var(--dim2);border:1px dashed var(--line);border-radius:5px;padding:2px 8px}
.whyidle{color:var(--dim2)}
@keyframes blink{50%{opacity:.2}}
.meter{font-size:10.5px;color:var(--dim2);font-variant-numeric:tabular-nums}
.meter b{color:#a9b5c2;font-weight:600}
</style></head><body>
<header><h1>Задачи <i>· незакрыто __N__ · снимок __AT__ · дом ~/.devai</i></h1></header>
<main id="m"></main>
<script>
const G = __DATA__;
const esc = s => String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
function steps(t){
  return t.stages.map((s,i)=>`<div class="step ${s.status} ${s.gate?'gate':''}">
    ${i?'<div class="rail"></div>':''}
    <div class="node"><span class="tick"></span></div>
    <div class="cap">${s.short}</div></div>`).join('');
}
function dots(t){
  return '<div class="dots">'+t.stages.map(s=>`<span class="d ${s.status} ${s.gate?'gate':''}"></span>`).join('')+'</div>';
}
function row(t){
  const flat = t.state === 'DRAFT';
  const meter = (t.attempts!==null && t.attempts!==undefined && t.attemptLimit && !flat)
    ? ` · попыток <b>${t.attempts}/${t.attemptLimit}</b>` : '';
  if (flat) {
    return `<div class="row s-${t.state} flat">
      <div><div class="name">${esc(t.name)}</div>
        <div class="ident"><span>${t.id}</span><span class="whyidle">${esc(t.runner.word)}</span></div></div>
      <div class="tlc">${dots(t)}</div>
      <div class="side"><span class="ghost">${esc(t.stateWord)}</span></div></div>`;
  }
  return `<div class="row s-${t.state}">
    <div>
      <div class="name">${esc(t.name)}</div>
      <div class="ident"><span>${t.id}</span>${t.why?`<span class="why">${esc(t.why)}</span>`:''}</div>
    </div>
    <div class="tl">${steps(t)}</div>
    <div class="side">
      <div class="state">${esc(t.stateWord)}</div>
      <div class="runchip ${t.runner.kind}"><span class="dot"></span>${esc(t.runner.word)}</div>
      ${meter?`<div class="meter">${meter.replace(' · ','')}</div>`:''}
    </div></div>`;
}
document.getElementById('m').innerHTML = G.map(g=>`
  <div class="grp ${g.key}"><span class="t">${g.title}</span>
    <span class="n">${g.items.length}</span><span class="bar"></span></div>
  ${g.items.map(row).join('')}`).join('');
</script></body></html>"""

html = html.replace('__DATA__', json.dumps(grouped, ensure_ascii=False))
html = html.replace('__N__', str(len(items)))
html = html.replace('__AT__', NOW.astimezone().strftime('%d.%m %H:%M'))
open('/tmp/proto/tasks.html', 'w', encoding='utf-8').write(html)
print('v2 written', len(html), 'байт;', len(items), 'задач в', len(grouped), 'группах')
for g in grouped:
    print(' ', g['title'], len(g['items']))
