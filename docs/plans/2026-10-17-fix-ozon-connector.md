# План исправления коннектора Ozon в ru-marketplace-mcp-rs

> **Цель:** Обеспечить стабильную работу коннектора Ozon (`ozon_search`, `ozon_card`) на чистом Rust без открытия видимых графических окон в Wayland/Niri, устранить блокировки Variti WAF и ликвидировать тихий сбой парсера.

**Архитектура:** Двухуровневая модель с нативным CDP-солвером на `chromiumoxide`. Быстрые запросы через `reqwest` с сессионными куками; при необходимости обновления — запуск Chromium в фоне с инъекцией CDP Stealth-скриптов (`Page.addScriptToEvaluateOnNewDocument`) и изоляцией дисплея в Xvfb `:99`.
**Стек:** Pure Rust 2024, `chromiumoxide` 0.9.1, `reqwest` 0.13, `tokio`, `serde_json`.

---

## 1. Выявленные первопричины сбоя (Root Causes)

1. **Утечка автоматизации через `DEFAULT_ARGS` в `chromiumoxide`:**
   Библиотека `chromiumoxide` по умолчанию добавляет флаг `--enable-automation` и аргумент `lang=en_US`. При наличии `--enable-automation` Chromium принудительно выставляет `navigator.webdriver = true` и оповещает страницу об автоматизации, что моментально детектируется Variti WAF и приводит к редиректу на `/?__rr=1` («Похоже, нет соединения»).
2. **Отсутствие CDP Evasions до загрузки страницы:**
   Вызов `browser.new_page("https://www.ozon.ru/")` начинает загрузку скриптов Ozon до того, как могут быть применены какие-либо модификации свойств (`navigator.webdriver`, `window.chrome`, `plugins`, `languages`).
3. **Фиксированный таймаут `sleep(4s)` вместо событийного ожидания:**
   Антибот-челлендж Variti решает Proof-of-Work и выставляет куки динамически. Запуск `fetch(...)` через жесткие 4 секунды приводит к вызову API на незавершившейся сессии или на странице блокировки.
4. **Игнорирование статуса HTTP и `incidentId` в JS-скрипте:**
   Скрипт `fetch(...).then(r => r.text())` возвращает тело ответа независимо от кода статуса. При HTTP 403 возвращается JSON инцидента безопасности: `{"incidentId": "fab_...", "supportURL": "..."}`.
5. **Тихий сбой парсера (`parse_composer_json` и `parse_card_json`):**
   Парсер ожидает только ключ `widgetStates`. При получении ответа с ошибкой ключ отсутствует, и парсер возвращает `Ok(Vec::new())` (для поиска) или `Err(NotFound)` (для карточки), маскируя ошибку безопасности под пустую выдачу или отсутствие товара.
6. **Рассинхронизация версий User-Agent:**
   В `src/transport/fingerprint.rs` захардкожен Chrome 124, в `native_solver.rs` — Chrome 133, а в Chromium CLI флаг `--user-agent` вообще не передавался.

---

## 2. Задачи реализации (Implementation Tasks)

### Задача 1: Stealth-конфигурация и CDP Evasions в `NativeCdpSolver`
**Файлы:**
- `src/session/native_solver.rs`
- `tests/solver_tests.rs`

- [ ] Вызвать `.disable_default_args()` в `BrowserConfig::builder()`, чтобы исключить автоматическое добавление `--enable-automation` и `lang=en_US`.
- [ ] Сформировать полный набор безопасных CLI-аргументов:
  - `--disable-blink-features=AutomationControlled`
  - `--no-sandbox`, `--disable-setuid-sandbox`
  - `--disable-dev-shm-usage`
  - `--disable-infobars`
  - `--window-size=1920,1080`
  - `--start-maximized`
  - `--lang=ru-RU,ru`
  - `--user-agent=...`
  - `--ozone-platform=x11`
  - `--no-first-run`
  - `--password-store=basic`
  - `--use-mock-keychain`
- [ ] Реализовать константу со скриптом обхода детекции (`STEALTH_EVASION_SCRIPT`):
  - `Object.defineProperty(navigator, 'webdriver', { get: () => undefined })`
  - Эмуляция `window.chrome` и `window.chrome.runtime`
  - Эмуляция `navigator.plugins` и `navigator.mimeTypes` (PDF Viewer)
  - Подстановка `navigator.languages = ['ru-RU', 'ru', 'en-US', 'en']`
  - Подмена `navigator.permissions.query` для уведомлений
- [ ] В `drive_composer_fetch` и `drive_ozon_harvest`:
  - Открывать чистую страницу: `browser.new_page("about:blank")`
  - Регистрировать stealth-скрипт до перехода: `page.evaluate_on_new_document(STEALTH_EVASION_SCRIPT)`
  - Выполнять переход через `page.goto("https://www.ozon.ru/")`

### Задача 2: Событийное ожидание сессии и безопасный вызов API
**Файлы:**
- `src/session/native_solver.rs`

- [ ] Заменить фиксированный `sleep(4s)` на цикл ожидания готовности сессии (проверка смены URL с `/?__rr=1`, исчезновения экранов челленджа, появления кук `__Secure-` / `xcid`).
- [ ] Переписать внутренний JavaScript `fetch` в `drive_composer_fetch` на безопасный возврат структуры `{ status, ok, body, error }`:
  ```javascript
  async () => {
      try {
          const res = await fetch('/api/composer-api.bx/page/json/v2?url=' + encodeURIComponent(...), {
              headers: { 'accept': 'application/json', 'x-o3-app-name': 'dtable' },
              credentials: 'include'
          });
          const text = await res.text();
          return JSON.stringify({ status: res.status, ok: res.ok, body: text });
      } catch (e) {
          return JSON.stringify({ status: 0, ok: false, error: String(e) });
      }
  }
  ```
- [ ] В Rust десериализовать обертку: при `status != 200` или `!ok` генерировать `ScraperError::WafBlocked` с деталями ошибки и `incidentId`, а не отдавать некорректное тело в парсер.

### Задача 3: Исправление парсера Composer API в `OzonConnector`
**Файлы:**
- `src/connectors/ozon.rs`
- `tests/connector_tests.rs`

- [ ] В `parse_composer_json`: проверять наличие полей `incidentId` / `error` в корневом объекте и возвращать типизированную ошибку `ScraperError::WafBlocked`.
- [ ] Добавить поддержку дополнительных префиксов виджетов поиска (`tileGrid-`, `catalog-`, `searchResultsV2-`).
- [ ] В `parse_card_json`: при отсутствии `widgetStates` проверять наличие инцидента безопасности и возвращать `ScraperError::WafBlocked`, а не `NotFound`.

### Задача 4: Синхронизация User-Agent и Client Hints
**Файлы:**
- `src/transport/fingerprint.rs`
- `src/session/native_solver.rs`
- `tests/session_tests.rs`

- [ ] Синхронизировать значение `DEFAULT_USER_AGENT` между `fingerprint.rs` и `native_solver.rs` (Chrome 133).
- [ ] Обновить заголовок `sec-ch-ua` в `fingerprint.rs` до версии 133 для соответствия User-Agent.

### Задача 5: Полная верификация и сквозное тестирование
- [ ] Прогон модульных тестов: `cargo test -- -j 2`
- [ ] Сборка и проверка синтаксиса: `cargo check`
- [ ] Проверка поиска товаров через реальный запрос `ozon_search` (проверка непустой выдачи товаров с ценами).
- [ ] Проверка получения карточки через реальный запрос `ozon_card` (проверка корректных данных товара).
- [ ] Подтверждение отсутствия видимых окон на рабочем столе пользователя (Niri / Wayland).
