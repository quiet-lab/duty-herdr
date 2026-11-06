"""Сравнение журналов `just bench-render-scale-pair`.

Режимы запуска (`python3 -I scripts/bench_render_scale_compare.py ...`):

  variant <каталог> [--threshold N]
      читает base_r*.log и head_r*.log, печатает таблицу Δ % по строкам
      (минимум медиан по раундам);
  summary <каталог-замера> <вариант>... [--threshold N]
      читает <каталог-замера>/<вариант>/{base,head}_r*.log, печатает сводку
      по вариантам сборки; код возврата 1, если есть строки, которые хуже
      порога во всех вариантах.
"""
import argparse
import collections
import glob
import re
import statistics
import sys

TEST_PREFIX = re.compile(r"^test \S+ \.\.\. ")
MEASURE_START = re.compile(r"[\d.]+ (?:ticks/s|us/frame|us/tick)")
MEASURE_VALUE = re.compile(r"([\d.]+) us/(?:frame|tick)")
MEDIAN = re.compile(r"median_(?:us|ns)=([\d.]+)")
MEASURE_FIELDS = re.compile(r"(?:median|p95|max)_(?:us|ns)=[\d.]+")

MEASURE_COLUMN = re.compile(r"_(?:us|ns|ms)$")

Result = collections.namedtuple("Result", "rows only_base only_head")


def parse_log(path):
    """Строки таблиц bench -> {ключ: median_us, median_ns или us/frame, us/tick}.

    Строка таблицы — строка с тем же числом колонок, что и заголовок с
    `median_us`; ключ строки — колонки левее `median_us`, кроме замеров (`*_us`, `*_ns`, `*_ms`); нечисловые входят.
    Ключ остальных строк — описание без измеренных чисел.
    """
    res = {}
    ctx = []
    hdr = None
    with open(path, errors="replace") as f:
        for line in f:
            s = line.rstrip("\n")
            if not s.strip():
                continue
            toks = s.split()
            if "median_us" in toks and not re.search(r"\d", toks[0]):
                hdr = toks
                continue
            if hdr and len(toks) == len(hdr):
                mi = hdr.index("median_us")
                try:
                    value = float(toks[mi])
                except ValueError:
                    value = None
                if value is not None:
                    res[" / ".join(ctx[-2:]) + " :: " + " ".join(t for h, t in zip(hdr[:mi], toks[:mi]) if not MEASURE_COLUMN.search(h))] = value
                    continue
            m = MEDIAN.search(s)
            if m:
                res[MEASURE_FIELDS.sub("", TEST_PREFIX.sub("", s)).strip()] = float(m.group(1))
                continue
            start = MEASURE_START.search(s)
            value = MEASURE_VALUE.search(s)
            if start and value:
                res[TEST_PREFIX.sub("", s[:start.start()]).rstrip(" ,:")] = float(value.group(1))
                continue
            ctx.append(TEST_PREFIX.sub("", s.strip()))
    return res


def aggregate(directory, kind):
    """Минимум медиан по раундам для каждой строки."""
    vals = collections.defaultdict(list)
    for path in sorted(glob.glob(f"{directory}/{kind}_r*.log")):
        for k, v in parse_log(path).items():
            vals[k].append(v)
    return {k: min(v) for k, v in vals.items()}


def compare_dir(directory):
    """Result: строки в обеих сторонах [(строка, база, HEAD, Δ %)] и ключи только с одной стороны."""
    base, head = aggregate(directory, "base"), aggregate(directory, "head")
    rows = [(k, base[k], head[k], (head[k] - base[k]) / base[k] * 100)
            for k in base if k in head and base[k] > 0]
    return Result(rows, sorted(k for k in base if k not in head), sorted(k for k in head if k not in base))


def side_lines(result):
    out = []
    for title, keys in (("только в базе", result.only_base), ("только в HEAD", result.only_head)):
        out.append(f"{title}: {len(keys)}")
        out += [f"  {k}" for k in keys]
    return out


def render_variant(result, threshold):
    rows = result.rows
    out = ["| строка | база | HEAD | Δ % |", "|---|---|---|---|"]
    out += [f"| {k} | {b:g} | {h:g} | {d:+.1f} |" for k, b, h, d in rows]
    over = [r for r in rows if r[3] > threshold]
    out.append("")
    out.append(f"строк: {len(rows)}, хуже порога {threshold:g} %: {len(over)}")
    out += [f"  {d:+.1f}  {k}" for k, _, _, d in over]
    out += side_lines(result)
    return "\n".join(out) + "\n"


def summarize(per_variant, threshold):
    """per_variant: {вариант: Result}. Возвращает (текст, число устойчиво худших строк)."""
    variants = list(per_variant)
    table = {v: {r[0]: r[3] for r in res.rows} for v, res in per_variant.items()}
    keys = [k for k in next(iter(table.values()), {}) if all(k in table[v] for v in variants)]
    lines = []
    stable = 0
    for k in keys:
        ds = [table[v][k] for v in variants]
        med = statistics.median(ds)
        all_worse = all(d > threshold for d in ds)
        marks = []
        if all_worse:
            marks.append("устойчиво хуже порога")
            stable += 1
        elif len(variants) > 1 and max(ds) - min(ds) > threshold / 2:
            marks.append("разброс сборок")
        lines.append((k, ds, med, marks))
    head = "| строка | " + " | ".join(f"Δ % {v}" for v in variants) + " | медиана Δ % | отметка |"
    out = [head, "|---|" + "---|" * (len(variants) + 2)]
    shown = [r for r in lines if r[3] or any(d > threshold for d in r[1])]
    for k, ds, med, marks in shown:
        out.append(f"| {k} | " + " | ".join(f"{d:+.1f}" for d in ds) + f" | {med:+.1f} | {', '.join(marks)} |")
    out.append("")
    out.append(f"вариантов: {len(variants)}, строк: {len(lines)}, порог: {threshold:g} %, "
               f"показаны строки с отметкой или выше порога: {len(shown)}")
    out.append(f"устойчиво хуже порога: {stable}")
    for v, res in per_variant.items():
        if res.only_base or res.only_head:
            out.append(f"вариант {v}: только в базе {len(res.only_base)}, только в HEAD {len(res.only_head)}")
            out += side_lines(res)
    return "\n".join(out) + "\n", stable


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="mode", required=True)
    v = sub.add_parser("variant")
    v.add_argument("directory")
    s = sub.add_parser("summary")
    s.add_argument("directory")
    s.add_argument("variants", nargs="+")
    for p in (v, s):
        p.add_argument("--threshold", type=float, default=10.0)
    a = ap.parse_args(argv)
    if a.mode == "variant":
        sys.stdout.write(render_variant(compare_dir(a.directory), a.threshold))
        return 0
    per_variant = {name: compare_dir(f"{a.directory}/{name}") for name in a.variants}
    text, stable = summarize(per_variant, a.threshold)
    sys.stdout.write(text)
    return 1 if stable else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
