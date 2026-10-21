"""Mock-only regression coverage for the probe's preliminary subprocesses."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location('transport_probe', Path(__file__).with_name('probe.py'))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class PreliminaryTimeoutTests(unittest.TestCase):
    def setUp(self):
        for target, kwargs in (
            ('sys.argv', {'new': ['probe.py', '--codex', 'mock-codex', '--out', 'mock-out',
                                 '--timeout', '17']}),
            ('tempfile.mkdtemp', {'return_value': str(Path('/mock-scratch').resolve())}),
            ('pathlib.Path.mkdir', {}),
            ('pathlib.Path.rglob', {'return_value': []}),
        ):
            patcher = patch(target, **kwargs)
            patcher.start()
            self.addCleanup(patcher.stop)
        self.run = self.enterContext(patch.object(probe.subprocess, 'run'))
        self.version = self.enterContext(patch.object(probe.subprocess, 'check_output',
                                                     return_value='mock version\n'))
        self.launch = self.enterContext(patch.object(probe.subprocess, 'Popen',
                                                    side_effect=RuntimeError('launch sentinel')))

    def test_preliminary_calls_have_bounded_timeouts(self):
        with self.assertRaisesRegex(RuntimeError, 'launch sentinel'):
            probe.main()
        self.assertEqual(self.run.call_count, 2)
        self.assertEqual(self.run.call_args_list[0].kwargs['timeout'], 60)
        self.assertEqual(self.run.call_args_list[1].kwargs['timeout'], 17)
        self.assertEqual(self.version.call_args.kwargs['timeout'], 60)
        self.assertNotIn('timeout', self.launch.call_args.kwargs)

    def assert_timeout(self, operation, expected, run_count, version_count):
        timeout = subprocess.TimeoutExpired(['private-command', 'private-argument'], 17,
                                            output=b'private-output', stderr=b'private-stderr')
        if operation == 'git':
            self.run.side_effect = timeout
        elif operation == 'schema':
            self.run.side_effect = [None, timeout]
        else:
            self.version.side_effect = timeout
        with self.assertRaises(SystemExit) as raised:
            probe.main()
        self.assertEqual(raised.exception.code, expected + ' timed out (details withheld)')
        self.assertTrue(raised.exception.__suppress_context__)
        self.assertEqual(self.run.call_count, run_count)
        self.assertEqual(self.version.call_count, version_count)
        self.launch.assert_not_called()

    def test_git_timeout_is_scrubbed_and_stops_startup(self):
        self.assert_timeout('git', 'Git initialization', 1, 0)

    def test_schema_timeout_is_scrubbed_and_stops_startup(self):
        self.assert_timeout('schema', 'Schema generation', 2, 0)

    def test_version_timeout_is_scrubbed_and_stops_startup(self):
        self.assert_timeout('version', 'Version lookup', 2, 1)


if __name__ == '__main__':
    unittest.main()
