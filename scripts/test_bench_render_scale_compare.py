import contextlib
import io
import os
import tempfile
import unittest

from scripts import bench_render_scale_compare as c


def log(hover, frame):
    return (
        "running 2 tests\n"
        f"test a::ctrl_hover_render_scale_profile ... ctrl-hover panes=1 active=false median_us={hover} p95_us=999\n"
        f"ctrl-hover panes=15 active=true median_us={hover} p95_us=999\n"
        "ok\n"
        f"test b::navigator_profile ... navigator: 1x1x1, query=\"\", {frame} us/frame\n"
        "ok\n"
    )


def write_variant(root, name, base, head):
    d = os.path.join(root, name)
    os.makedirs(d)
    for kind, (hover, frame) in (("base", base), ("head", head)):
        for r, extra in ((1, 5), (2, 0)):  # минимум по раундам берёт второй
            with open(os.path.join(d, f"{kind}_r{r}.log"), "w") as f:
                f.write(log(hover + extra, frame + extra))


class CompareTest(unittest.TestCase):
    def test_parse_and_min_of_rounds(self):
        with tempfile.TemporaryDirectory() as t:
            write_variant(t, "v", (100, 400), (120, 400))
            rows = {r[0]: r for r in c.variant_rows(os.path.join(t, "v"))}
            hover = rows["ctrl-hover panes=1 active=false"]
            self.assertEqual((hover[1], hover[2]), (100, 120))
            self.assertAlmostEqual(hover[3], 20.0)
            frame = [r for r in rows.values() if "navigator" in r[0]][0]
            self.assertAlmostEqual(frame[3], 0.0)

    def test_table_format(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "x.log")
            with open(p, "w") as f:
                f.write("running\ntest m::t ... scaling\n panes median_us p95_us\n1 50 60\n15 70 80\n")
            res = c.parse_log(p)
            self.assertEqual(sorted(res.values()), [50.0, 70.0])
            self.assertEqual(len(res), 2)

    def test_summary_stable_regression(self):
        with tempfile.TemporaryDirectory() as t:
            write_variant(t, "a", (100, 400), (130, 400))
            write_variant(t, "b", (100, 400), (125, 400))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = c.main(["summary", t, "a", "b"])
            self.assertEqual(code, 1)
            self.assertIn("устойчиво хуже порога", out.getvalue())

    def test_summary_build_spread(self):
        with tempfile.TemporaryDirectory() as t:
            write_variant(t, "a", (100, 400), (130, 400))
            write_variant(t, "b", (100, 400), (95, 400))
            text, stable = c.summarize(
                {n: c.variant_rows(os.path.join(t, n)) for n in ("a", "b")}, 10.0)
            self.assertEqual(stable, 0)
            self.assertIn("разброс сборок", text)

    def test_summary_clean(self):
        with tempfile.TemporaryDirectory() as t:
            write_variant(t, "a", (100, 400), (101, 400))
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(c.main(["summary", t, "a"]), 0)


if __name__ == "__main__":
    unittest.main()
