#!/usr/bin/env bash
# Выпуск форка duty-herdr с этой станции: сборка, latest.json, пакет pacman,
# репозиторий pacman и GitHub Release.
#
# Использование: scripts/release-local.sh <тег> [--install] [--dry-run] [--notes-file ФАЙЛ]
#   <тег>        duty-v<cargo-версия>[.N], например duty-v0.9.5 или duty-v0.9.5.1
#   --install    после сборки поставить бинарник в ~/.local/bin/herdr
#   --dry-run    напечатать шаги без сборки и публикации
#   --notes-file заметки выпуска; без него — git log от прошлого тега duty-v*
#
# Окружение: ZIG (путь к zig 0.16.0), CARGO_BUILD_JOBS (необязательно).
set -euo pipefail

REPO_SLUG="quiet-lab/duty-herdr"

usage() {
    sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
}

tag=""
install_local=0
dry_run=0
notes_file=""
while [ $# -gt 0 ]; do
    case "$1" in
        --install) install_local=1 ;;
        --dry-run) dry_run=1 ;;
        --notes-file)
            [ $# -ge 2 ] || usage
            notes_file="$2"
            shift
            ;;
        -h | --help) usage ;;
        -*) echo "неизвестный ключ: $1" >&2; usage ;;
        *)
            [ -z "$tag" ] || usage
            tag="$1"
            ;;
    esac
    shift
done
[ -n "$tag" ] || usage

if ! [[ "$tag" =~ ^duty-v[0-9]+\.[0-9]+\.[0-9]+(\.[0-9]+)?$ ]]; then
    echo "тег должен иметь вид duty-v<major>.<minor>.<patch>[.N]: $tag" >&2
    exit 2
fi
pkgver="${tag#duty-v}"
pkgrel=1
root="$(git rev-parse --show-toplevel)"
cd "$root"
# Версия пакета herdr из Cargo.toml (первая строка version в разделе [package]).
cargo_version="$(awk '/^\[package\]/{p=1;next} /^\[/{p=0} p && /^version *=/{gsub(/[" ]|version=/,"");sub(/^version=/,"");print;exit}' Cargo.toml)"
[ -n "$cargo_version" ] || { echo "не найдена version в Cargo.toml" >&2; exit 1; }
if [ "$tag" != "duty-v$cargo_version" ] && ! [[ "$tag" =~ ^duty-v${cargo_version//./\.}\.[0-9]+$ ]]; then
    echo "тег $tag не соответствует версии Cargo.toml: нужен duty-v$cargo_version или duty-v$cargo_version.N" >&2
    exit 2
fi
out="$root/.local/release/$tag"

# Шаг, меняющий состояние: при --dry-run только печатается.
run() {
    if [ "$dry_run" -eq 1 ]; then
        printf '[dry-run] %q' "$1"
        shift
        printf ' %q' "$@"
        printf '\n'
    else
        "$@"
    fi
}

# Проверка: при --dry-run провал — предупреждение, иначе остановка.
check() {
    local message="$1"
    shift
    if "$@"; then
        return 0
    fi
    if [ "$dry_run" -eq 1 ]; then
        echo "[dry-run] предупреждение: $message" >&2
    else
        echo "ошибка: $message" >&2
        exit 1
    fi
}

tree_is_clean() { [ -z "$(git status --porcelain)" ]; }
on_duty_branch() { [ "$(git rev-parse --abbrev-ref HEAD)" = "duty" ]; }
tag_is_free() {
    ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null &&
        [ -z "$(git ls-remote --tags origin "refs/tags/$tag")" ]
}
zig_is_set() { [ -n "${ZIG:-}" ] && [ -x "$ZIG" ]; }

echo "== проверки"
check "дерево не чистое" tree_is_clean
check "текущая ветвь не duty" on_duty_branch
check "тег $tag уже существует (локально или в origin)" tag_is_free
check "gh не авторизован (gh auth status)" gh auth status --hostname github.com >/dev/null 2>&1
check "ZIG не задан или не исполняемый" zig_is_set

commit="$(git rev-parse HEAD)"

echo "== cargo fmt, clippy, test"
run cargo fmt --check
run cargo clippy --all-targets --locked
run cargo test --locked

echo "== сборка $tag ($commit)"
run env HERDR_BUILD_ID="$tag" HERDR_BUILD_COMMIT="$commit" cargo build --release --locked

bin="$root/target/release/herdr"
run rm -rf "$out"
run mkdir -p "$out/src" "$out/build"

echo "== бинарник и контрольная сумма"
run cp "$bin" "$out/herdr-linux-x86_64"
if [ "$dry_run" -eq 1 ]; then
    sha="<sha256 herdr-linux-x86_64>"
else
    sha="$(sha256sum "$out/herdr-linux-x86_64" | cut -d' ' -f1)"
fi

echo "== заметки выпуска"
notes_out="$out/notes.md"
if [ -n "$notes_file" ]; then
    run cp "$notes_file" "$notes_out"
else
    prev="$(git describe --tags --match 'duty-v*' --abbrev=0 2>/dev/null || true)"
    if [ -n "$prev" ]; then
        range="$prev..HEAD"
    else
        range="HEAD~20..HEAD"
    fi
    if [ "$dry_run" -eq 1 ]; then
        echo "[dry-run] git log --format='- %s' $range > $notes_out"
    else
        git log --format='- %s' "$range" >"$notes_out"
    fi
fi

echo "== latest.json"
protocol="$(sed -n 's/^pub const PROTOCOL_VERSION: u32 = \([0-9]*\);.*/\1/p' src/protocol/wire.rs)"
[ -n "$protocol" ] || { echo "не найден PROTOCOL_VERSION в src/protocol/wire.rs" >&2; exit 1; }
if [ "$dry_run" -eq 1 ]; then
    echo "[dry-run] latest.json: version=$cargo_version protocol=$protocol build_id=$tag"
else
    python3 -I - "$out" "$tag" "$cargo_version" "$protocol" "$sha" "$REPO_SLUG" <<'PY'
import json, sys
out, tag, version, protocol, sha, slug = sys.argv[1:7]
with open("distribution/latest.json", encoding="utf-8") as f:
    upstream = json.load(f)
with open(f"{out}/notes.md", encoding="utf-8") as f:
    notes = f.read().strip()
manifest = {
    "version": version,
    "build_id": tag,
    "protocol": int(protocol),
    "endpoint_generation": upstream["endpoint_generation"],
    "notes": notes,
    "assets": {
        "linux-x86_64": f"https://github.com/{slug}/releases/download/{tag}/herdr-linux-x86_64",
    },
    "sha256": {"linux-x86_64": sha},
}
with open(f"{out}/latest.json", "w", encoding="utf-8") as f:
    json.dump(manifest, f, ensure_ascii=False, indent=2)
    f.write("\n")
PY
fi

echo "== пакет pacman"
src_name="herdr-linux-x86_64-$pkgver"
run cp "$out/herdr-linux-x86_64" "$out/src/$src_name"
run cp packaging/arch/duty-herdr-bin/PKGBUILD "$out/build/PKGBUILD"
run sed -i \
    -e "s/^pkgver=.*/pkgver=$pkgver/" \
    -e "s/^pkgrel=.*/pkgrel=$pkgrel/" \
    -e "s/^sha256sums=.*/sha256sums=('$sha')/" \
    "$out/build/PKGBUILD"
# SRCDEST с готовым файлом: makepkg его не скачивает.
run env -C "$out/build" SRCDEST="$out/src" PKGDEST="$out" BUILDDIR="$out/build/work" \
    makepkg --force --nodeps --nocheck
pkg="duty-herdr-bin-$pkgver-$pkgrel-x86_64.pkg.tar.zst"

echo "== репозиторий pacman"
run env -C "$out" repo-add duty-herdr.db.tar.gz "$pkg"
# GitHub не хранит символические ссылки: копии обычными файлами.
run env -C "$out" cp --remove-destination -L duty-herdr.db.tar.gz duty-herdr.db
run env -C "$out" cp --remove-destination -L duty-herdr.files.tar.gz duty-herdr.files

if [ "$install_local" -eq 1 ]; then
    echo "== установка в ~/.local/bin"
    run install -Dm755 "$bin" "$HOME/.local/bin/herdr"
fi

echo "== тег и выпуск"
run git tag -a "$tag" -m "$tag" "$commit"
run git push origin "$tag"
run gh release create "$tag" --repo "$REPO_SLUG" --title "$tag" --notes-file "$out/notes.md" \
    "$out/herdr-linux-x86_64" \
    "$out/latest.json" \
    "$out/$pkg" \
    "$out/duty-herdr.db" \
    "$out/duty-herdr.db.tar.gz" \
    "$out/duty-herdr.files" \
    "$out/duty-herdr.files.tar.gz"

echo "готово: $tag"
