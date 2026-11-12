// Выгрузка ПРАВИЛ приёмки из профиля дома в JSON, чтобы прототип мог
// показать у каждого пункта правило, по которому его оценивали.
// Запуск: node dump_rules.js <profile.yaml> <out.json>
const YAML = require('/home/guerrero/.hermes/lsp/node_modules/yaml');
const fs = require('fs');

const [src, out] = process.argv.slice(2);
const d = YAML.parse(fs.readFileSync(src, 'utf8'));
const floors = (d.scoring && d.scoring.floors) || {};
const severities = {};
for (const [t, v] of Object.entries(d.error_types || {})) {
  severities[t] = v && v.severity ? v.severity : null;
}

const rules = {};
for (const ev of d.evaluations || []) {
  for (const c of ev.checks || []) {
    let floor = null;
    for (const t of c.detects || []) {
      const sev = severities[t];
      const f = sev ? floors[sev] : null;
      if (f != null && (floor == null || f > floor)) floor = f;
    }
    rules[c.id] = {
      group: ev.title || ev.id,
      statement: c.statement || '',
      verify: c.verify || '',
      anchors: c.anchors || {},
      detects: (c.detects || []).map(t => ({ t, sev: severities[t] || null })),
      floor,
    };
  }
}

const meta = {
  version: d.version,
  project: d.project,
  theta: (d.scoring || {}).theta,
  floors,
  blocked_max: ((d.scoring || {}).blocked_policy || {}).max_blocked_weight,
  weights: (d.scoring || {}).weights || {},
  gates: (d.gates || []).map(g => ({ id: g.id, title: g.title, kind: g.kind })),
  rules,
};
fs.writeFileSync(out, JSON.stringify(meta, null, 1));
console.log('правил выгружено:', Object.keys(rules).length, '→', out);
