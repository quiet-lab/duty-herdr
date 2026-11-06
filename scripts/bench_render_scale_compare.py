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

KEY_COLUMNS = ("count", "panes", "clients", "hidden_output", "pattern", "mode")
TEST_PREFIX = re.compile(r"^test \S+ \.\.\. ")


def parse_log(path):
    """Строки таблиц bench -> {ключ: median_us или us/frame}."""
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
            if hdr and re.fullmatch(r"\d+", toks[0]) and len(toks) == len(hdr):
                mi = hdr.index("median_us")
                keycols = [t for h, t in zip(hdr, toks) if h in KEY_COLUMNS]
                res[" / ".join(ctx[-2:]) + " :: " + " ".join(keycols)] = float(toks[mi])
                continue
            m = re.search(r"median_us=([\d.]+)", s)
            if m:
                key = re.sub(r"(median|p95)_us=[\d.]+|max_us=[\d.]+", "", TEST_PREFIX.sub("", s)).strip()
                res[key] = float(m.group(1))
                continue
            m = re.search(r"([\d.]+) us/(frame|tick)", s)
            if m:
                res[TEST_PREFIX.sub("", s.split(", ", 1)[0] + ", " + s.rsplit(",", 1)[0])] = float(m.group(1))
                continue
            if not re.match(r"\s*\d", s):
                hdr = None
                ctx.append(TEST_PREFIX.sub("", s.strip()))
    return res


def aggregate(directory, kind):
    """Минимум медиан по раундам для каждой строки."""
    vals = collections.defaultdict(list)
    for path in sorted(glob.glob(f"{directory}/{kind}_r*.log")):
        for k, v in parse_log(path).items():
            vals[k].append(v)
    return {k: min(v) for k, v in vals.items()}


def compare_rows(base, head):
    """[(строка, база, HEAD, Δ %)] по строкам, присутствующим в обоих."""
    return [(k, base[k], head[k], (head[k] - base[k]) / base[k] * 100)
            for k in base if k in head and base[k] > 0]


def variant_rows(directory):
    return compare_rows(aggregate(directory, "base"), aggregate(directory, "head"))


def render_variant(rows, threshold):
    out = ["| строка | база | HEAD | Δ % |", "|---|---|---|---|"]
    out += [f"| {k} | {b:g} | {h:g} | {d:+.1f} |" for k, b, h, d in rows]
    over = [r for r in rows if r[3] > threshold]
    out.append("")
    out.append(f"строк: {len(rows)}, хуже порога {threshold:g} %: {len(over)}")
    out += [f"  {d:+.1f}  {k}" for k, _, _, d in over]
    return "\n".join(out) + "\n"


def summarize(per_variant, threshold):
    """per_variant: {вариант: rows}. Возвращает (текст, число устойчиво худших строк)."""
    variants = list(per_variant)
    table = {v: {r[0]: r[3] for r in rows} for v, rows in per_variant.items()}
    keys = [k for k in next(iter(table.values()), {}) if all(k in table[v] for v in variants)]
    lines = []
    stable = 0
    for k in keys:
        ds = [table[v][k] for v in variants]
        med = statistics.median(ds)
        all_worse = all(d > threshold for d in ds)
        signs_differ = min(ds) < 0 < max(ds) and max(ds) - min(ds) > threshold / 2
        spread = max(ds) - min(ds) > threshold / 2
        marks = []
        if all_worse:
            marks.append("устойчиво хуже порога")
            stable += 1
        if len(variants) > 1 and (signs_differ or spread) and not all_worse:
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
        sys.stdout.write(render_variant(variant_rows(a.directory), a.threshold))
        return 0
    per_variant = {name: variant_rows(f"{a.directory}/{name}") for name in a.variants}
    text, stable = summarize(per_variant, a.threshold)
    sys.stdout.write(text)
    return 1 if stable else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
