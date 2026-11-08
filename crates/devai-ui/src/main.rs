//! Бинарная обёртка поверхности devai-ui: вся точка входа — в библиотеке
//! (`devai_ui::run`), чтобы тот же код собирался и как Android-приложение
//! (Tauri mobile требует lib-таргет с `mobile_entry_point`).

fn main() {
    devai_ui::run();
}
