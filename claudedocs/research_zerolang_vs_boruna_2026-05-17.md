# Research: ZeroLang (vercel-labs/zero) vs Boruna — сравнение

**Дата:** 2026-05-17
**Изследван обект:** https://zerolang.ai/ · https://github.com/vercel-labs/zero
**Сравнен с:** Boruna (ai-lang) — този проект
**Дълбочина:** standard · **Увереност:** висока (първични източници: official site + repo README + GitHub API)

---

## Executive Summary

ZeroLang и Boruna **споделят една и съща философия, но са различни продукти**.

- **Обща ДНК:** и двата са „agent-native" езици — експлицитни capabilities в сигнатурите, JSON структурни диагностики с repair-метаданни, една малка toolchain.
- **Различна същност:** Zero е **системен език за компилиране на малки native бинарни инструменти** (без GC, без runtime). Boruna е **платформа за детерминистично, одитируемо изпълнение на enterprise AI workflows** (bytecode + VM + evidence bundles).
- **Не сме конкуренти на едно поле.** Zero конкурира Rust/Zig/C за писане на CLI инструменти. Boruna конкурира workflow/orchestration платформи за compliance.
- **Има 6–7 идеи, които си струва да заемем** — без да губим нашия диференциатор (determinism + replay + hash-chained evidence).

⚠️ **Стратегически сигнал:** Zero е създаден от **Vercel Labs**, на **2026-05-15** (преди 2 дни), и вече има **1166 звезди**. Категорията „agent-native език с експлицитни capabilities + JSON диагностики" вече е валидирана от голям играч. Това е едновременно потвърждение, че сме на права посока, и сигнал, че трябва ясно да защитим това, което Zero **не** прави.

---

## Какво е ZeroLang

| | |
|---|---|
| Тип | Системен programming език (general-purpose, за малки native tools) |
| Автор | Vercel Labs |
| Създаден | 2026-05-15 · 1166 ⭐ за 2 дни · Apache-2.0 |
| Език на компилатора | C (`native/zero-c/`) + self-hosted (`compiler-zero/`) |
| Файлово разширение | `.0` |
| Статус | Experimental, нестабилен |

**Таглайн:** „The programming language for agents — humans and AI agents can read, repair, inspect, and ship small native programs together."

**Ключови технически свойства:**
- **Native артефакти** — статичен dispatch, **без задължителен GC**, без event loop, без скрит runtime. `zero size --json` показва цената на артефакта.
- **Capability-based I/O** — функциите декларират какво докосват; компилаторът отхвърля недостъпни capabilities **по време на компилация**.
- **Експлицитни effects & memory** — сигнатурите излагат fallibility (`raises`) и capabilities; алокацията е видима.
- **Agent-first tooling** — `zero check --json` връща структурни диагностики със стабилни кодове (`NAM003`) и `repair` метаданни.
- **Cross-target проверки** — компилаторът проверява target-neutral код за няколко target-а; emit на `linux-musl-x64` и др.
- **C ABI boundary** — експорт на C ABI символи за low-level interop.
- **Една toolchain:** `check`, `build`, `test`, `format`, `graph`, `size`, `routes`, `skills`, `doctor`, `document`.

---

## Сравнителна таблица

| Измерение | ZeroLang | Boruna |
|---|---|---|
| **Категория** | Системен език за native tools | Платформа за изпълнение на AI workflows |
| **Цел на компилация** | Native бинарни файлове (exe, C ABI) | Bytecode за custom VM |
| **Runtime модел** | Без runtime, без GC, без event loop | VM с capability gateway, actor система |
| **Главен диференциатор** | Малки артефакти, native, размерна прозрачност | Determinism + replay + hash-chained evidence bundles |
| **Capabilities** | ✅ Експлицитни, проверка при компилация | ✅ Експлицитни (`!{net.fetch}`), enforce-ват се в VM |
| **JSON диагностики + repair** | ✅ `check --json`, стабилни кодове | ✅ `lang check --json`, `lang repair`, suggested patches |
| **Agent интеграция** | `zero skills get`, machine-readable docs | `boruna-mcp` MCP сървър (10 tools) |
| **Compliance / audit** | ❌ Не е фокус | ✅ Ядро — EvidenceBundle, AuditLog, verify |
| **Workflow / DAG** | ❌ Няма | ✅ Ядро — WorkflowDef, validator, runner |
| **Framework модел** | ❌ Няма | ✅ Elm-архитектура (init/update/view) |
| **Целеви потребител** | Разработчици/агенти, пишещи CLI tools | Enterprise, изпълняващ одитируеми AI процеси |
| **Зрялост** | 2 дни, experimental, голям hype | По-зрял (557+ теста, 9 крейта, roadmap до 1.0) |

---

## Прилики (реално конвергентни решения)

1. **Capability-based effects** — почти идентична концепция. Zero: „compiler rejects unavailable capabilities". Boruna: `!{net.fetch}` анотации, VM gateway. И двата правят side effects видими в сигнатурата.
2. **JSON диагностики с repair-метаданни** — Zero: `"repair": {"id": "declare-missing-symbol"}`. Boruna: diagnostics със suggested patches + `boruna lang repair`. Една и съща идея: „хората четат текста, агентите четат JSON-а".
3. **Local reasoning** — сигнатурите излагат fallibility + capabilities за двата езика.
4. **Една малка toolchain** — обединен CLI за check/build/test/format/inspect.
5. **Agent-native позициониране** — и двата изрично се продават като езици, проектирани да бъдат поддържани от AI агенти.

**Извод:** Не сме копирали един друг — независимо стигнахме до едни и същи принципи. Това валидира дизайна на Boruna.

---

## Ключови разлики (нашият защитен ров)

Zero **не прави** нищо от следното, а то е сърцето на Boruna:
- Детерминистично изпълнение с гаранция „same input → same output".
- Запис/replay на изпълнения (`EventLog`, `ReplayEngine`).
- Hash-chained, tamper-evident evidence bundles за compliance/audit.
- DAG workflow оркестрация с policy gates и human approval.
- Enterprise compliance шаблони (SOC 2, HIPAA, финанси).

Обратно — Zero има неща, които Boruna няма (native компилация, без GC, C ABI, размерни отчети), но те **не са релевантни** за нашата ниша (изпълнение на workflows, не доставка на native бинарни файлове).

---

## Идеи за заемане (приоритизирани)

| # | Идея от Zero | Приложимост за Boruna | Усилие |
|---|---|---|---|
| 1 | **Стабилни кодове за диагностики** (`NAM003`) | Ако още нямаме стабилни, документирани error codes — да въведем. Критично за agent-repair надеждност. | Малко |
| 2 | **`size --json` / отчети за цена на артефакт** | `boruna` може да докладва размер на bytecode модул, брой стъпки, budget цена преди изпълнение. | Средно |
| 3 | **`graph --json`** — graph facts от CLI | Boruna има workflow DAG-ове — изложи `workflow graph --json` за визуализация/инспекция от агенти. | Малко |
| 4 | **`doctor --json`** — диагностика на toolchain/среда | Команда, която проверява среда, features, policy конфигурация и връща JSON. | Малко |
| 5 | **`skills get`** — machine-readable docs пакети за агенти | Boruna има MCP; добави skill/docs пакет, който агентите дърпат за самообучение по `.ax`. | Средно |
| 6 | **Cross-target / pre-flight проверки** | Boruna може да докладва изисквани capabilities/policy **преди** изпълнение („този workflow ще иска net.fetch + db.query"). | Средно |
| 7 | **Позициониране/маркетинг** — `curl \| bash` инсталатор, ясен „за агенти" таглайн, docs site | Подобри лендинга на Boruna с конкретен agent-native наратив. | Малко |

**Какво да НЕ заемаме:** native компилация, премахване на VM, C ABI, без GC — те противоречат на detmerinism/replay модела ни.

---

## Препоръка

1. **Запази стратегията.** Boruna и Zero не са конкуренти — различни слоеве. Не пренасочвай Boruna към native компилация.
2. **Засили диференциатора.** В маркетинга и docs изрично подчертай: determinism + replay + evidence bundles — точно това, което Zero (и Vercel) не предлагат.
3. **Бързи победи:** въведи стабилни диагностични кодове (#1), `doctor --json` (#4), `workflow graph --json` (#3) — евтини, директно повишават agent-надеждността.
4. **Следи Zero.** Голям играч в съседна ниша; ако Zero добави workflow/audit слой, става директен конкурент. Преглеждай repo-то им периодично.

---

## Източници

- https://zerolang.ai/ — официален сайт (fetched 2026-05-17)
- https://github.com/vercel-labs/zero — README + GitHub API метаданни (fetched 2026-05-17)
- Boruna `architecture` Serena memory + project CLAUDE.md

*Това е изследователски доклад. Без имплементация — следващата стъпка е решение на потребителя.*
