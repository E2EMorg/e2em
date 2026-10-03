from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from runtime_energy import energy_delta, read_domains
from measure_runtime import counters


class RuntimeMeasurement(unittest.TestCase):
    def test_energy_wraparound_is_reported_in_joules_without_summing_domains(self):
        first = {"package": {"name": "package-0", "energy_uj": 9_000_000, "max_energy_range_uj": 10_000_000},
                 "core": {"name": "core", "energy_uj": 1_000_000, "max_energy_range_uj": 10_000_000}}
        last = {"package": {**first["package"], "energy_uj": 2_000_000},
                "core": {**first["core"], "energy_uj": 2_000_000}}
        result = energy_delta(first, last, 2)
        self.assertEqual(result["domains"]["package"]["joules"], 3)
        self.assertEqual(result["domains"]["package"]["average_watts"], 1.5)
        self.assertEqual(result["domains"]["core"]["joules"], 1)
        self.assertNotIn("total_joules", result)
        for changed, seconds in (({}, 2), (last, 0), ({"package": {**last["package"], "max_energy_range_uj": 20}, "core": last["core"]}, 2)):
            with self.assertRaises(ValueError):
                energy_delta(first, changed, seconds)

    def test_nested_energy_domains_require_real_counter_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(OSError):
                read_domains(root)
            folder = root / "intel-rapl:0" / "intel-rapl:0:0"
            folder.mkdir(parents=True)
            for name, value in (("name", "core"), ("energy_uj", "123"), ("max_energy_range_uj", "1000")):
                (folder / name).write_text(value)
            self.assertEqual(read_domains(root)[folder.name]["energy_uj"], 123)

    def test_idle_counters_include_worker_threads_not_only_the_broker_main_thread(self):
        import os
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            process = root / "123"
            process.mkdir()
            (process / "status").write_text("VmRSS:\t10000 kB\nvoluntary_ctxt_switches:\t2\n")
            fields = ["S"] + ["0"] * 30
            fields[11], fields[12] = "150", "50"
            (process / "stat").write_text("123 (name with spaces) " + " ".join(fields))
            for tid, voluntary, slices in (("123", 2, 4), ("124", 30, 35)):
                task = process / "task" / tid
                task.mkdir(parents=True)
                (task / "status").write_text(f"voluntary_ctxt_switches:\t{voluntary}\nnonvoluntary_ctxt_switches:\t1\n")
                (task / "schedstat").write_text(f"0 0 {slices}")
            result = counters(123, root)
            self.assertEqual(result["voluntary_ctxt_switches"], 32)
            self.assertEqual(result["nonvoluntary_ctxt_switches"], 2)
            self.assertEqual(result["scheduled_timeslices"], 39)
            self.assertEqual(result["threads"], 2)
            self.assertEqual(result["cpu_seconds"], 200 / os.sysconf("SC_CLK_TCK"))


if __name__ == "__main__":
    unittest.main()
