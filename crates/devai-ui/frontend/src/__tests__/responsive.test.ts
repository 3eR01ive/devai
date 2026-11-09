// Адаптивная раскладка (REQ-RESP-01/02/03/05/08): правила проверяются
// механически, а не «на глаз».
//
// 1) Стили: телефонная раскладка живёт в медиа-запросе <= 767px, таблица
//    там — карточки (thead скрыт, tr — flex), цели нажатия не меньше 44px,
//    длинные строки переносятся, страница не прокручивается вбок. Правила
//    десктопа остаются вне этого запроса — эталон не переписан.
// 2) Разметка: в каждой таблице каждого экрана у КАЖДОЙ ячейки есть
//    data-label, а число колонок совпадает с числом заголовков. Значит на
//    телефоне карточка показывает ровно те же поля, что колонки десктопа —
//    без «+N ещё» и без скрытых колонок (REQ-RESP-02 AC4, REQ-RESP-07).
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE, "..");
const CSS = readFileSync(join(SRC, "styles.css"), "utf8");

function viewFiles(): string[] {
  return readdirSync(join(SRC, "views"))
    .filter((f: string) => f.endsWith(".vue"))
    .map((f: string) => join(SRC, "views", f));
}

function stripTags(html: string): string {
  return html
    .replace(/<[^>]*>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function tableBlocks(source: string): string[] {
  return [...source.matchAll(/<table\b[^>]*>([\s\S]*?)<\/table>/g)].map((m) => m[1]);
}

function firstDataRow(block: string): string {
  const body = block.match(/<tbody\b[^>]*>([\s\S]*?)<\/tbody>/);
  const rows = (body ? body[1] : block).match(/<tr\b[^>]*>[\s\S]*?<\/tr>/);
  return rows ? rows[0] : "";
}

describe("responsive stylesheet", () => {
  const mobile = CSS.slice(CSS.indexOf("@media (max-width: 767px)"));

  it("keeps the phone layout inside a <=767px media query", () => {
    expect(CSS).toContain("@media (max-width: 767px)");
    // Desktop base rules stay outside: the table is a table, header is flex.
    expect(CSS).toContain("table.data {\n  width: 100%;");
    expect(CSS).toContain("header.topbar {\n  display: flex;");
  });

  it("turns each table row into a card and hides only the header row", () => {
    expect(mobile).toContain("table.data thead {\n    display: none;");
    expect(mobile).toContain("table.data tr {\n    display: flex;");
    expect(mobile).toContain("table.data td::before {\n    content: attr(data-label);");
  });

  it("gives every tap target at least 44x44 and wraps long text", () => {
    expect(mobile).toContain("min-height: 44px");
    expect(mobile).toContain("min-width: 44px");
    expect(mobile).toContain("overflow-wrap: anywhere");
    expect(mobile).toContain("pre.content {\n    overflow-x: hidden;");
  });
});

describe("every table field survives the phone card", () => {
  for (const file of viewFiles()) {
    const source = readFileSync(file, "utf8");
    const tables = tableBlocks(source);
    if (!tables.length) continue;
    const name = file.slice(SRC.length + 1);

    it(`${name}: each cell has a label and the column count matches the header`, () => {
      for (const block of tables) {
        const headers = [...block.matchAll(/<th\b[^>]*>([\s\S]*?)<\/th>/g)].map((m) =>
          stripTags(m[1]),
        );
        const row = firstDataRow(block);
        const cellCount = (row.match(/<td\b/g) ?? []).length;
        const labels = [...row.matchAll(/<td\b[^>]*\bdata-label="([^"]*)"/g)].map(
          (m) => m[1],
        );
        expect(cellCount).toBeGreaterThan(0);
        // Каждая ячейка несёт подпись — ни одна не теряется на телефоне.
        expect(labels).toHaveLength(cellCount);
        expect(labels.every((l) => l.trim().length > 0)).toBe(true);
        // Число полей карточки = число колонок десктопной таблицы.
        // У таблицы без заголовка (журнал событий) колонок-заголовков нет,
        // поэтому сверяем только формы с шапкой.
        if (headers.length > 0) expect(cellCount).toBe(headers.length);
        // Непустые заголовки видны подписями ниже первой строки карточки.
        for (const h of headers.filter((h) => h.length > 0)) {
          expect(labels).toContain(h);
        }
      }
    });
  }
});
