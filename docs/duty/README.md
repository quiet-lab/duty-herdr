# duty-herdr

Форк herdr для рабочего места duty. Ветвь `master` зеркалит upstream
`herdrdev/herdr` (только fast-forward), ветвь `duty` — `master` с патчами
форка. Версия в `Cargo.toml` остаётся upstream'ной, признак форка —
`HERDR_BUILD_ID`: `herdr --version` печатает `0.9.5 (duty-v0.9.5)`.
Документы upstream (`docs/next/**`, `docs/preview/**`, `docs/versions/**`)
не правятся; описание форка ведётся только здесь.

## Что меняет форк

- Значки состояния агентов: таблица `[ui.status_icons]` переопределяет
  значок каждого состояния поверх `status_indicators`.
- Токен `focused_tab` в строках агентов: подпись вкладки, выбранной
  в workspace агента.
- `agent prompt`: Enter отправляется после того, как хвост текста
  появился в строке ввода агента; текст не появился — одна повторная
  вставка, затем ошибка `agent_prompt_lost`. Пауза перед Enter убрана.
- Обновление: `herdr update` берёт манифест `latest.json` из Releases
  форка `quiet-lab/duty-herdr`; установка из пакета pacman распознаётся,
  `herdr update` печатает подсказку обновить пакет.
- Установка: `distribution/install.sh` ставит выпуск форка.

## Настройка

Значки состояния. Незаданные состояния берут значок текущего стиля.
Допустима строка шириной 1–2 ячейки, без селектора вариации FE0F.
Цвет палитры к эмодзи не применяется.

```toml
[ui.status_icons]
idle = "🟡"
working = "🟢"
blocked = "🔴"
done = "✅"
unknown = "·"
```

Вариант с символами:

```toml
[ui.status_icons]
idle = "○"
working = "●"
blocked = "◆"
done = "✓"
```

Цвет заголовков spaces задают правила у токена `workspace` в панели
spaces. Поддерживаются `equals`, `contains`, `starts_with`, `gt`, `lt`;
правил не больше 16; цвет относится к тексту токена, не к фону строки.

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", { token = "workspace", rules = [
      { starts_with = "🐘", fg = "#f9e2af" },
      { starts_with = "🧪", fg = "#94e2d5" },
      { contains = "monorepo-ocp", fg = "#89b4fa" },
  ] }],
  ["branch", "git_status"],
]
```

Раскладка строк агентов с токеном `focused_tab`:

```toml
[ui.sidebar.agents]
rows = [["state_icon", "machine", "tab"], ["focused_tab"], ["agent"]]
```

## Установка и обновление

Установщик одной командой:

```sh
curl -fsSL https://raw.githubusercontent.com/quiet-lab/duty-herdr/duty/distribution/install.sh | sh
```

Репозиторий pacman на Releases форка. Строки для `/etc/pacman.conf`:

```ini
[duty-herdr]
SigLevel = Optional TrustAll
Server = https://github.com/quiet-lab/duty-herdr/releases/latest/download
```

Установка пакета:

```sh
paru -S duty-herdr-bin
```

Пакет `duty-herdr-bin` объявляет `provides=(herdr)` и
`conflicts=(herdr herdr-bin)`: пакеты `herdr` и `herdr-bin` перед
установкой удаляются. Бинарник ставится в `/usr/bin/herdr`.

Обновление установки через установщик — `herdr update`.

## Выпуск

Выпуск собирается на станции и заливается сценарием
`scripts/release-local.sh`. Ветвь `duty` должна быть чистой, тег
не должен существовать, `gh` — авторизован, `ZIG` — задан.

```sh
just release-local duty-v0.9.5
just release-local duty-v0.9.5 --dry-run
just release-local duty-v0.9.5.1 --install --notes-file заметки.md
```

Тег — `duty-v<cargo-версия>`; повторный выпуск той же версии —
`duty-v<cargo-версия>.N`. Сценарий выполняет по порядку:

1. `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`;
2. `cargo build --release --locked` с `HERDR_BUILD_ID=<тег>` и
   `HERDR_BUILD_COMMIT=<sha>`;
3. `latest.json` (поле `build_id` равно тегу, `protocol` из
   `src/protocol/wire.rs`, `endpoint_generation` из
   `distribution/latest.json`);
4. пакет `duty-herdr-bin` через `makepkg` в `.local/release/<тег>/`;
5. репозиторий pacman: `repo-add`, `duty-herdr.db` и `duty-herdr.files`
   копиями обычными файлами;
6. аннотированный тег, push тега и `gh release create` со всеми
   активами: `herdr-linux-x86_64`, `latest.json`, пакет
   `duty-herdr-bin-<pkgver>-1-x86_64.pkg.tar.zst`, `duty-herdr.db`,
   `duty-herdr.db.tar.gz`, `duty-herdr.files`,
   `duty-herdr.files.tar.gz`.

Ключ `--install` ставит собранный бинарник в `~/.local/bin/herdr`.
GitHub Actions форка не используются.

## Слияние upstream

1. `git fetch upstream`, затем в ветви `master`:
   `git merge --ff-only upstream/master`.
2. В ветви `duty`: `git merge v0.9.x` (тег upstream), разрешение
   конфликтов в пользу сохранения патчей форка.
3. `just test`, `cargo clippy --all-targets`, `cargo fmt --check`.
4. Выпуск: `just release-local duty-v0.9.x`.

## Сборка на станции

Для vendored libghostty-vt нужен Zig 0.16.0; он ставится через mise:

```sh
mise use -g zig@0.16.0
export ZIG="$HOME/.local/share/mise/installs/zig/0.16.0/zig"
export CARGO_BUILD_JOBS=16
cargo build --release --locked
```

Без `ZIG` сборка завершается ошибкой. Первая сборка занимает около
трёх минут на 16 потоках.
