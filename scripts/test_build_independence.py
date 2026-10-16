"""Self-tests for the opt-in M9 evidence gate; no builds or QEMU required."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("gate", Path(__file__).with_name("test-build-independence.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class BuildEvidenceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.kernel = Path(temporary.name) / "kernel"
        self.kernel.write_bytes(b"kernel ELF fixture")
        self.baseline = [gate.fingerprint(self.kernel)]

    def evidence(self, package="alloyos-welder", fresh=False, kind="bin"):
        return "\n".join(json.dumps(m) for m in [
            {"reason": "compiler-artifact", "package_id": f"path+file:///repo#{package}@0.1.0",
             "fresh": fresh, "target": {"kind": [kind]}},
            {"reason": "build-finished", "success": True}])

    def test_actual_executable_build_and_unchanged_kernel_pass(self):
        self.assertEqual(gate.verify(self.baseline, self.evidence(), "alloyos-welder"), [])

    def test_empty_unrelated_library_and_fresh_builds_fail(self):
        for log in ["", self.evidence("xtask"), self.evidence(fresh=True), self.evidence(kind="lib")]:
            with self.subTest(log=log):
                self.assertTrue(gate.verify(self.baseline, log, "alloyos-welder"))

    def test_kernel_host_compilation_fails_even_with_unchanged_guest(self):
        log = self.evidence("alloyos-kernel") + "\n" + self.evidence()
        self.assertIn("Cargo compiled a kernel target (host or guest)",
                      gate.verify(self.baseline, log, "alloyos-welder"))

    def test_changed_bytes_and_mtime_are_independent_failures(self):
        import os
        old = self.baseline[0]
        self.kernel.write_bytes(b"changed")
        os.utime(self.kernel, ns=(old["mtime_ns"], old["mtime_ns"]))
        self.assertTrue(gate.verify(self.baseline, self.evidence(), "alloyos-welder"))
        self.kernel.write_bytes(b"kernel ELF fixture")
        os.utime(self.kernel, ns=(old["mtime_ns"], old["mtime_ns"] + 1_000_000))
        self.assertTrue(gate.verify(self.baseline, self.evidence(), "alloyos-welder"))

    def test_failed_incomplete_and_non_json_logs_fail(self):
        for log in [self.evidence().splitlines()[0], self.evidence().replace('"success": true', '"success": false'), "build complete", "[]", "null"]:
            self.assertTrue(gate.verify(self.baseline, log, "alloyos-welder"))

    def test_architecture_checks_transitive_normal_and_build_dependencies(self):
        def package(name, deps=(), binary=False):
            return {"name": name, "manifest_path": f"/repo/{name}/Cargo.toml",
                    "dependencies": [{"path": f"/repo/{dep}", "kind": kind} for dep, kind in deps],
                    "targets": [{"kind": ["bin" if binary else "lib"]}]}
        packages = [package("xtask", [("image", None)]),
                    package("image", [("alloyos-kernel", "build")]),
                    package("alloyos-kernel", [("alloyos-ferrofluid", "dev")]),
                    package("alloyos-welder", binary=True), package("alloyos-ferrofluid")]
        errors = gate.architecture_errors({"packages": packages})
        self.assertEqual(len(errors), 1)
        self.assertIn("xtask", errors[0])
        packages[1]["dependencies"] = []
        self.assertEqual(gate.architecture_errors({"packages": packages}), [])


if __name__ == "__main__":
    unittest.main()
