// Минимальные объявления node-модулей для тестов фронтенда: браузерный
// tsconfig не тянет @types/node, а src/__tests__/responsive.test.ts читает
// файлы проекта напрямую. Объявлены только используемые функции.
declare module "node:fs" {
  export function readFileSync(path: string, encoding: "utf8"): string;
  export function readdirSync(path: string): string[];
}

declare module "node:path" {
  export function dirname(path: string): string;
  export function join(...parts: string[]): string;
  export function resolve(...parts: string[]): string;
}

declare module "node:url" {
  export function fileURLToPath(url: string): string;
}
