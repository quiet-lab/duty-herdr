#!/usr/bin/env bash
# Парный замер `render_scale_profile` между базой и текущим деревом.
#
#   scripts/bench_render_scale_pair.sh <base-ref> [out-dir]
#
# Одна сборка на коммит даёт разброс +-10 % при равном числе инструкций
# (разбиение на codegen units, раскладка кода), поэтому сравнение идёт по
# нескольким вариантам сборки, парно: база и HEAD одновременно на закреплённых
# ядрах разных CCX, ядра меняются каждый раунд, из раундов берётся минимум
# медиан. Вывод читается по <out>/summary.md.
#
# Переменные окружения:
#   BENCH_PAIR_STAGE      all (сборки и замер), build (только сборки), measure
#                         (только замер: worktree базы и сборки переиспользуются) (all)
#   BENCH_PAIR_VARIANTS   варианты сборки через пробел (default "default cgu1 align")
#   BENCH_PAIR_ROUNDS     число раундов (4)
#   BENCH_PAIR_CPUS       два ядра разных CCX (одно для базы, другое для HEAD) ("4 8")
#   BENCH_PAIR_MAX_LOAD   предельный loadavg за минуту перед замером (8)
#   BENCH_PAIR_THRESHOLD  порог ухудшения, % (10)
#   CARGO_BUILD_JOBS      потоков сборки (8)
#   ZIG                   путь к Zig 0.16.0 (по умолчанию установка mise)
#
# Нагрузка проверяется перед сборками и перед замером; при высокой нагрузке
# код возврата 3 (после собственных сборок loadavg ещё высок: повторите
# с BENCH_PAIR_STAGE=measure).
# Worktree базы остаётся в <out-dir>/base-tree и переиспользуется, пока он на
# том же коммите (иначе сборки базы потеряли бы кэш из-за новых mtime).
#
# Код возврата: 0 — устойчиво хуже порога нет; 1 — есть; 3 — станция занята;
# 2, 4 и выше — ошибка.
set -euo pipefail

if (($# < 1 || $# > 2)); then
  echo "использование: $0 <base-ref> [out-dir]" >&2
  exit 2
fi

root=$(git rev-parse --show-toplevel)
cd "$root"
base_ref=$1
base_sha=$(git rev-parse --short=8 "$base_ref^{commit}")
head_sha=$(git rev-parse --short=8 HEAD)
out=${2:-target/bench-pair/$base_sha-$head_sha}
mkdir -p "$out"
out=$(cd "$out" && pwd)

# Варианты сборки: имя -> RUSTFLAGS и переменные окружения cargo.
variant_env() {
  case $1 in
    default) echo "" ;;
    cgu1) echo "CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1" ;;
    align) echo "RUSTFLAGS=-C llvm-args=-align-all-functions=6 -C llvm-args=-align-all-nofallthru-blocks=5" ;;
    *) echo "неизвестный вариант сборки: $1" >&2; return 1 ;;
  esac
}

variants=${BENCH_PAIR_VARIANTS:-default cgu1 align}
rounds=${BENCH_PAIR_ROUNDS:-4}
read -r cpu_a cpu_b <<<"${BENCH_PAIR_CPUS:-4 8}"
max_load=${BENCH_PAIR_MAX_LOAD:-8}
threshold=${BENCH_PAIR_THRESHOLD:-10}
stage=${BENCH_PAIR_STAGE:-all}
case $stage in all | build | measure) ;; *) echo "BENCH_PAIR_STAGE: all, build или measure, получено $stage" >&2; exit 2 ;; esac
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8}

export ZIG=${ZIG:-$HOME/.local/share/mise/installs/zig/0.16.0/zig}
if [[ ! -x $ZIG ]]; then
  echo "Zig не найден: $ZIG" >&2
  echo "установите: mise use -g zig@0.16.0, либо задайте ZIG (docs/duty/README.md, «Сборка на станции»)" >&2
  exit 2
fi

compare=$root/scripts/bench_render_scale_compare.py
base_tree=$out/base-tree
base_full=$(git rev-parse "$base_sha^{commit}")
creating=
remove_base_tree() {
  git -C "$root" worktree remove --force "$base_tree" 2>/dev/null || rm -rf "$base_tree"
  git -C "$root" worktree prune
}
# Worktree базы удаляется только при прерванном создании.
trap '[[ -z $creating ]] || remove_base_tree' EXIT

# Отказ при высокой нагрузке: loadavg за минуту ниже предела.
check_load() {
  local l1
  l1=$(cut -d' ' -f1 /proc/loadavg)
  awk -v l="$l1" -v m="$max_load" 'BEGIN{exit !(l < m)}' && return 0
  echo "loadavg $l1 >= $max_load: станция занята, $1 не начат" >&2
  echo "$2" >&2
  exit 3
}

log() { echo "[$(date +%H:%M:%S)] $*"; }
load() { cut -d' ' -f1-3 /proc/loadavg; }

if [[ -n $(git status --porcelain --untracked-files=no) ]]; then
  log "внимание: в дереве HEAD есть незакоммиченные правки, они входят в замер"
  dirty=" (с незакоммиченными правками)"
else
  dirty=""
fi

# Сборкам тишина не нужна: проверка до сборок только у полного прогона,
# чтобы не собирать впустую.
[[ $stage != all ]] || check_load "прогон" "соберите заранее с BENCH_PAIR_STAGE=build, замер — с BENCH_PAIR_STAGE=measure"
if [[ -d $base_tree && $(git -C "$base_tree" rev-parse HEAD 2>/dev/null) == "$base_full" ]]; then
  log "этап 1 из 3: worktree базы $base_ref ($base_sha) переиспользован: $base_tree"
else
  log "этап 1 из 3: worktree базы $base_ref ($base_sha) в $base_tree"
  [[ ! -e $base_tree ]] || remove_base_tree
  creating=1
  git worktree add --detach "$base_tree" "$base_sha" >/dev/null
  creating=
fi

# Сборка тестового бинарника в дереве $1, target-каталог $2, окружение варианта $3; печатает путь.
build() {
  local tree=$1 target=$2 venv=$3
  local -a envs=()
  [[ -n $venv ]] && envs=(env "$venv")
  (cd "$tree" && "${envs[@]}" cargo test --release --locked --bin herdr --no-run \
      --target-dir "$target" --message-format=json) |
    python3 -Ic 'import sys, json
exe = None
for l in sys.stdin:
    try:
        m = json.loads(l)
    except Exception:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "herdr" and m["profile"]["test"]:
        exe = m["executable"]
    if m.get("reason") == "compiler-message" and m["message"]["level"] == "error":
        print(m["message"]["rendered"], file=sys.stderr)
if not exe:
    sys.exit("тестовый бинарник не найден")
print(exe)'
}

nvar=$(wc -w <<<"$variants")
i=0
for v in $variants; do
  i=$((i + 1))
  venv=$(variant_env "$v")
  log "этап 2 из 3: сборка, вариант $v ($i из $nvar), load $(load)"
  exe_base[$i]=$(build "$base_tree" "$out/target-$v-base" "$venv")
  exe_head[$i]=$(build "$root" "$out/target-$v-head" "$venv")
done

if [[ $stage == build ]]; then
  log "сборки готовы, замер: BENCH_PAIR_STAGE=measure"
  exit 0
fi
check_load "замер" "повторите с BENCH_PAIR_STAGE=measure, сборки переиспользуются"

args=(render_scale_profile --ignored --nocapture --test-threads=1)
i=0
for v in $variants; do
  i=$((i + 1))
  vdir=$out/$v
  mkdir -p "$vdir"
  for ((r = 1; r <= rounds; r++)); do
    if ((r % 2)); then cb=$cpu_a ch=$cpu_b; else cb=$cpu_b ch=$cpu_a; fi
    log "этап 3 из 3: замер, вариант $v ($i из $nvar), раунд $r из $rounds, load $(load)"
    (cd "$base_tree" && taskset -c "$cb" "${exe_base[$i]}" "${args[@]}" >"$vdir/base_r$r.log" 2>&1) &
    pb=$!
    (cd "$root" && taskset -c "$ch" "${exe_head[$i]}" "${args[@]}" >"$vdir/head_r$r.log" 2>&1) &
    ph=$!
    for side in base:"$pb" head:"$ph"; do
      wait "${side#*:}" || { echo "тест упал (${side%%:*}, раунд $r): $vdir/${side%%:*}_r$r.log" >&2; exit 4; }
      grep -q 'test result: ok' "$vdir/${side%%:*}_r$r.log" ||
        { echo "тест не прошёл (${side%%:*}, раунд $r): $vdir/${side%%:*}_r$r.log" >&2; exit 4; }
    done
  done
  python3 -I "$compare" variant "$vdir" --threshold "$threshold" >"$vdir/compare.md"
done

{
  echo "# Парный замер render_scale_profile"
  echo
  echo "база: $base_ref ($base_sha); HEAD: $head_sha$dirty"
  echo "варианты: $variants; раундов: $rounds; ядра: $cpu_a и $cpu_b"
  echo
} >"$out/summary.md"
rc=0
# shellcheck disable=SC2086
python3 -I "$compare" summary "$out" $variants --threshold "$threshold" >>"$out/summary.md" || rc=$?
cat "$out/summary.md"
log "итог: $out/summary.md, код $rc"
exit "$rc"
