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
            rows = {r[0]: r for r in c.compare_dir(os.path.join(t, "v")).rows}
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

    def test_non_numeric_table_rows(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "x.log")
            with open(p, "w") as f:
                f.write("test m::t ... surface\n"
                        "  layout       panes  cells_changed  mode     median_us  bytes_per_update\n"
                        "  background      1          false   legacy        713             33909\n"
                        "  active         15           true    delta        508              1012\n"
                        "surface dense\n"
                        "  pattern       clients     mode  median_us  bytes/update\n"
                        "  active panes=1\n"
                        "  dense              1   legacy        839         33829\n")
            res = c.parse_log(p)
            self.assertEqual(len(res), 3)
            self.assertEqual(res["test m::t ... surface".replace("test m::t ... ", "") + " :: background 1 false legacy"], 713.0)
            self.assertEqual(res["surface dense / active panes=1 :: dense 1 legacy"], 839.0)

    def test_tick_keys_without_measurements(self):
        with tempfile.TemporaryDirectory() as t:
            p = os.path.join(t, "x.log")
            with open(p, "w") as f:
                f.write("test m::t ... status frame tick: animated=true, 1 working workspaces, 326.5 us/tick\n"
                        "status frame boundary ticks: 5 working workspaces (10 rows): 12.0 ticks/s, "
                        "80.5 us/tick (worst 99.0 us), deadline search 0.50 us\n")
            res = c.parse_log(p)
            self.assertEqual(res["status frame tick: animated=true, 1 working workspaces"], 326.5)
            self.assertEqual(res["status frame boundary ticks: 5 working workspaces (10 rows)"], 80.5)

    def test_one_sided_keys_reported(self):
        with tempfile.TemporaryDirectory() as t:
            write_variant(t, "a", (100, 400), (101, 400))
            with open(os.path.join(t, "a", "head_r1.log"), "a") as f:
                f.write("extra panes=3 median_us=7\n")
            res = c.compare_dir(os.path.join(t, "a"))
            self.assertEqual(res.only_head, ["extra panes=3"])
            self.assertIn("только в HEAD: 1", c.render_variant(res, 10.0))
            text, _ = c.summarize({"a": res}, 10.0)
            self.assertIn("только в HEAD 1", text)

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
                {n: c.compare_dir(os.path.join(t, n)) for n in ("a", "b")}, 10.0)
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
