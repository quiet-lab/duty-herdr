# duty-herdr

Форк herdr для рабочего места duty. Ветвь `master` зеркалит upstream
`herdrdev/herdr` (только fast-forward), ветвь `duty` — `master` с патчами
форка. Версия в `Cargo.toml` остаётся upstream'ной, признак форка —
`HERDR_BUILD_ID` (тег выпуска): `herdr --version` печатает
`herdr 0.9.5 (duty-v0.9.5.1 ee5bfb5)` для выпуска (тег и семь знаков
коммита) и `herdr 0.9.5 (dev)` для сборки без `HERDR_BUILD_ID`.
Документы upstream (`docs/next/**`, `docs/preview/**`, `docs/versions/**`)
не правятся; описание форка ведётся только здесь.

## Что меняет форк

- Значки состояния агентов: таблица `[ui.status_icons]` переопределяет
  значок каждого состояния поверх `status_indicators`.
- Токен `focused_tab` в строках агентов: подпись вкладки, выбранной
  в workspace агента.
- `agent prompt`: Enter отправляется после того, как текст появился
  на экране агента; текст не появился — одна повторная вставка, затем
  ошибка `agent_prompt_lost`.
- Обновление: `herdr update` и удалённая установка берут манифест
  `latest.json` из Releases форка `quiet-lab/duty-herdr`; установка
  из пакета pacman распознаётся, `herdr update` печатает подсказку
  обновить пакет.
- Установка: `distribution/install.sh` ставит выпуск форка.
- Выпускается только `linux-x86_64`; канал `preview` не публикуется.

## Настройка

Значки состояния. Допустимы пять ключей: `idle`, `working`, `blocked`,
`done`, `unknown`. Значение — непустая строка шириной 1–2 ячейки без
селектора вариации U+FE0F; иное значение отклоняет разбор конфигурации.
Незаданное состояние берёт значок из `status_indicators`.

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

Цвет токена из метаданных. Поле `fg` стиля токена и правила `rules`
принимает, кроме `#hex`, ссылку `"$<имя>"` на пользовательский токен
текущего элемента: для панели spaces это токены workspace, для панели
агентов — токены панели. Имя проверяется как у пользовательских токенов
`$name` в `rows` (до 32 символов: буквы ASCII, цифры, `_`, `-`). Значение
токена разбирается как `#RGB` или `#RRGGBB`; токена нет, он пуст или не
цвет — ссылка не действует, берётся `fg` более общего уровня либо цвет
темы. Сравнение значений в `rules` не меняется. Цвет относится к тексту
токена, не к фону строки.

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", { token = "workspace", fg = "$color" }],
  ["branch", "git_status"],
]
```

Цвет задают командой, источник и значение меняются на лету:

```sh
herdr workspace report-metadata <ws> --source duty --token color=#f9e2af
herdr pane report-metadata <pane> --source duty --token color=#f9e2af
```

Главной сессии репозитория выдаётся белый цвет: `color=#ffffff`.

Токен `focused_tab` — подпись вкладки, выбранной в workspace агента,
с учётом выбора этого клиента. В отличие от `tab`, показывается всегда,
а не только при нескольких вкладках или заданной подписи. Это текстовый
токен: принимает `fg`, `bold`, `dim` и `rules` так же, как `tab`.
Раскладка строк агентов:

```toml
[ui.sidebar.agents]
rows = [["state_icon", "machine", "tab"], ["focused_tab"], ["agent"]]
```

Команда `agent prompt` на Linux и macOS отправляет Enter, когда на экране
агента появился хвост текста (последние 24 графемы без пробелов и
символов рамок) или метка свёрнутой вставки (`[Pasted text #…`,
`[Pasted Content …`). Ожидание одной попытки — 5 с; затем текст
вставляется ещё раз, и после второй неудачи команда возвращает ошибку
`agent_prompt_lost`, а Enter не отправляется. На Windows остаётся
пауза 300 мс между текстом и Enter.

Если эхо первой попытки приходит после повтора в одном кадре с ним, текст
на экране оказывается дважды: Enter не отправляется, команда возвращает
`agent_prompt_lost` с пояснением об удвоении. Эхо первой попытки,
нарисованное отдельно раньше эха повтора, от него не отличить. Пока
отправка ждёт эха, ввод пользователя в панель (в том числе Ctrl-C)
проходит сразу; экран проверяется только по изменению содержимого и только
по нижним 12 строкам.

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
установкой удаляются. Бинарник ставится в `/usr/bin/herdr`. Пакет собирается из шаблона
`packaging/arch/duty-herdr-bin/PKGBUILD`.

Установщик читает `releases/latest/download/latest.json`, проверяет
SHA-256 и ставит бинарник в каталог установки. Платформа — только
Linux x86_64; на других установщик завершается ошибкой.

Обновление — `herdr update`. Манифест берётся по адресу
`https://github.com/quiet-lab/duty-herdr/releases/latest/download/latest.json`;
обновление ставится при большей версии либо при той же версии с другим
`build_id` (сборка без `HERDR_BUILD_ID` отличается от любого выпуска).
Канал `preview` отклоняется с подсказкой `herdr channel set stable`.
Установка из пакета pacman (`/usr/bin/herdr` числится в
`/var/lib/pacman/local/duty-herdr-bin-*`) не обновляется самой
программой: `herdr update` печатает `paru -Syu duty-herdr-bin`.

Удалённая установка (`herdr` на SSH-хосте) берёт тот же манифест и ставит
только `linux-x86_64`; для другой платформы нужна переменная
`HERDR_REMOTE_BINARY=<путь к бинарнику>`.

## Выпуск

Выпуск собирается на станции и заливается сценарием
`scripts/release-local.sh`. Ветвь `duty` должна быть чистой, тег
не должен существовать, `gh` — авторизован, `ZIG` — задан.

```sh
just release-local duty-v0.9.5
just release-local duty-v0.9.5 --dry-run
just release-local duty-v0.9.5.1 --install --notes-file заметки.md
```

Тег — `duty-v<версия Cargo.toml>`; повторный выпуск той же версии —
`duty-v<версия Cargo.toml>.N`; иной тег сценарий отклоняет. Сценарий выполняет по порядку:

1. `cargo fmt --check`, `cargo clippy --all-targets`, `just test`
   (`just check` не годится: требует Windows SDK);
2. `cargo build --release --locked` с `HERDR_BUILD_ID=<тег>` и
   `HERDR_BUILD_COMMIT=<sha>`;
3. `latest.json` (поле `build_id` равно тегу, `protocol` из
   `src/protocol/wire.rs`, `endpoint_generation` из
   `distribution/latest.json`);
4. пакет `duty-herdr-bin` через `makepkg` в `.local/release/<тег>/`;
5. репозиторий pacman: `repo-add`, `duty-herdr.db` и `duty-herdr.files`
   копиями обычными файлами;
6. `gh release create <тег> --target <sha>` со всеми активами:
   `herdr-linux-x86_64`, `latest.json`, пакет
   `duty-herdr-bin-<pkgver>-1-x86_64.pkg.tar.zst`, `duty-herdr.db`,
   `duty-herdr.db.tar.gz`, `duty-herdr.files`,
   `duty-herdr.files.tar.gz`; тег в origin создаёт сам `gh`, затем
   сценарий забирает его командой `git fetch`. При сбое выпуск и тег
   удаляются, повтор того же тега проходит;
7. только после успешного выпуска — `--install`.

Ключи: `--install` после успешного выпуска ставит собранный бинарник в `~/.local/bin/herdr`,
`--dry-run` печатает шаги без сборки и публикации (провал проверок
становится предупреждением), `--notes-file` задаёт заметки выпуска (без
него — `git log` от прошлого тега `duty-v*`). Окружение: `ZIG`,
необязательно `CARGO_BUILD_JOBS`. GitHub Actions форка не используются.

## Слияние upstream

1. `git fetch upstream`, затем в ветви `master`:
   `git merge --ff-only upstream/master`.
2. В ветви `duty`: `git merge v0.9.x` (тег upstream), разрешение
   конфликтов в пользу сохранения патчей форка.
3. `just test`, `cargo clippy --all-targets`, `cargo fmt --check`.
4. Выпуск: `just release-local duty-v0.9.x` — сценарий создаёт тег форка.

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
