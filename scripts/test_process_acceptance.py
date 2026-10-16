"""Host regression tests for QEMU acceptance gates, with no builds or guests."""
import unittest

from process_acceptance import (
    ISOLATION_MARKER, RUNTIME_MARKERS, acceptance_errors, has_fatal_failure,
)


def transcript(lines):
    return b'\n'.join(lines) + b'\n'


class ProcessAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.runtime = transcript(RUNTIME_MARKERS)
        self.isolation = transcript((
            b'[user-panic] intentional EL0 panic',
            b'[crash] exception domain=user pid=7 ec=0x24',
            b'[crash] frame=2 pc=0x1234 kind=return',
            ISOLATION_MARKER,
        ))

    def test_all_runtime_acceptance_probes_are_required(self):
        self.assertFalse(acceptance_errors(self.runtime))
        for missing in RUNTIME_MARKERS:
            with self.subTest(missing=missing):
                errors = acceptance_errors(transcript(m for m in RUNTIME_MARKERS if m != missing))
                self.assertEqual(errors, ['missing: ' + missing.decode()])

    def test_partial_and_embedded_pass_markers_cannot_accept(self):
        for marker in RUNTIME_MARKERS:
            for replacement in (marker[:-1], b'quoted ' + marker, marker + b' stale'):
                with self.subTest(marker=marker, replacement=replacement):
                    self.assertTrue(acceptance_errors(self.runtime.replace(marker, replacement)))
        self.assertTrue(acceptance_errors(self.runtime.rstrip(b'\n')))

    def test_crlf_serial_output_accepts(self):
        self.assertFalse(acceptance_errors(self.runtime.replace(b'\n', b'\r\n')))
        self.assertFalse(acceptance_errors(self.isolation.replace(b'\n', b'\r\n'), isolation=True))

    def test_kernel_failure_rejects_even_after_all_passes(self):
        for failure in (b'[crash] kernel-panic assertion failed',
                        b'[crash] exception domain=kernel pid=0 ec=0x25'):
            for output in (failure + b'\n' + self.runtime, self.runtime + failure):
                with self.subTest(failure=failure):
                    self.assertTrue(has_fatal_failure(output))
                    self.assertIn('kernel panic in guest output', acceptance_errors(output))

    def test_contained_faults_do_not_fail_runtime(self):
        output = self.runtime + self.isolation + b'[display] reason=process-fault\n'
        self.assertFalse(has_fatal_failure(output))
        self.assertFalse(acceptance_errors(output))

    def test_isolation_requires_full_probe_result_not_survival_fragment(self):
        self.assertFalse(acceptance_errors(self.isolation, isolation=True))
        self.assertTrue(acceptance_errors(self.isolation.replace(ISOLATION_MARKER, b'kernel survival'), isolation=True))

    def test_isolation_requires_panic_and_actual_user_unwind_records(self):
        for line in self.isolation.splitlines():
            with self.subTest(line=line):
                self.assertTrue(acceptance_errors(self.isolation.replace(line + b'\n', b''), isolation=True))
        for original, replacement in ((b'domain=user ', b'domain=kernel '),
                                      (b'frame=2 ', b'frame=20 '),
                                      (b'[crash] frame=2 ', b'quoted [crash] frame=2 ')):
            self.assertTrue(acceptance_errors(self.isolation.replace(original, replacement), isolation=True))

    def test_empty_log_fails_both_gates(self):
        self.assertTrue(acceptance_errors(b''))
        self.assertTrue(acceptance_errors(b'', isolation=True))


if __name__ == '__main__':
    unittest.main()
