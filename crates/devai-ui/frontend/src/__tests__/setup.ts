// Окружение тестов: в этой сборке jsdom (Node 26) window.localStorage
// отсутствует, а Node объявляет глобальный localStorage пустым без флага.
// Настройки интервала UI читают localStorage — даём простую in-memory
// реализацию ровно с теми методами, которые использует settings.ts.
if (typeof window !== "undefined" && !window.localStorage) {
  const map = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, String(v)),
      removeItem: (k: string) => void map.delete(k),
      clear: () => map.clear(),
      key: (i: number) => [...map.keys()][i] ?? null,
      get length() {
        return map.size;
      },
    },
  });
}
Object.defineProperty(globalThis, "localStorage", {
  value: window.localStorage,
  configurable: true,
});
