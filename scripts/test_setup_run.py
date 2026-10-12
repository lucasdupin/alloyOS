import contextlib
import importlib.util
import io
import os
import pty
import select
import subprocess
import sys
import termios
import time
from pathlib import Path
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('setup_run', Path(__file__).with_name('setup_run.py'))
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class SetupTests(unittest.TestCase):
    def setUp(self):
        environment = mock.patch.dict(os.environ, {}, clear=True)
        environment.start()
        self.addCleanup(environment.stop)

    def test_return_and_default_work_after_raw_terminal(self):
        master, slave = pty.openpty()
        process = None
        try:
            modes = termios.tcgetattr(slave)
            modes[0] &= ~(termios.ICRNL | termios.INLCR)
            modes[3] &= ~(termios.ICANON | termios.ISIG)
            termios.tcsetattr(slave, termios.TCSANOW, modes)
            script = str(Path(setup.__file__).resolve())
            code = 'import runpy; s=runpy.run_path(' + repr(script) + '); s["normalize_terminal_input"](); print("READY", flush=True); print("CHOICE=" + s["choose"]("Choose", ("0", "2"), "0"), flush=True); print("DEFAULT=" + s["choose"]("Choose", ("0", "2"), "0"), flush=True)'
            process = subprocess.Popen([sys.executable, '-u', '-c', code], stdin=slave, stdout=slave, stderr=slave)
            output = bytearray()
            def wait_for(marker):
                deadline = time.monotonic() + 5
                while marker not in output:
                    remaining = deadline - time.monotonic()
                    self.assertGreater(remaining, 0, repr(output))
                    readable, _, _ = select.select([master], [], [], remaining)
                    self.assertTrue(readable, repr(output))
                    output.extend(os.read(master, 4096))
            wait_for(b'READY')
            os.write(master, b'2\r')
            wait_for(b'CHOICE=2')
            os.write(master, b'\r')
            wait_for(b'DEFAULT=0')
            self.assertEqual(process.wait(timeout=5), 0)
            repaired = termios.tcgetattr(slave)
            self.assertTrue(repaired[0] & termios.ICRNL)
            self.assertTrue(repaired[3] & termios.ICANON)
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)

    def test_hidden_entry_is_saved_without_echo(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(setup.getpass, 'getpass', return_value='fixture-secret') as hidden, mock.patch('builtins.input', return_value='y'), contextlib.redirect_stdout(io.StringIO()) as output:
            kind, path = setup.credential('HF_TOKEN')
            self.assertEqual(kind, '--auth-file')
            self.assertEqual(Path(path).read_text(), 'fixture-secret\n')
            hidden.assert_called_once()
            self.assertNotIn('fixture-secret', output.getvalue())

    def test_secret_permissions_reuse_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch.dict(os.environ, {}, clear=True):
            path = setup.save_secret('HF_TOKEN', 'fixture-secret')
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(FileExistsError):
                setup.save_secret('HF_TOKEN', 'replacement')
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(setup.credential('HF_TOKEN'), ('--auth-file', str(path)))
            self.assertNotIn('fixture-secret', output.getvalue())

    def test_symlink_credentials_rejected(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch.dict(os.environ, {}, clear=True):
            root = Path(tmp)
            (root / '.secrets').mkdir()
            (root / 'external').write_text('fixture-secret')
            (root / '.secrets/HF_TOKEN').symlink_to(root / 'external')
            with self.assertRaises(ValueError):
                setup.credential('HF_TOKEN')

    def test_launch_reuses_model_and_forwards_only_cloud_credentials(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)):
            image = Path(tmp) / '.local/models/gemma-2-2b-it/model.img'
            image.parent.mkdir(parents=True)
            image.touch()
            output = io.StringIO()
            with mock.patch('sys.argv', ['setup_run', '--memory', '20G']), mock.patch.object(setup.shutil, 'which', return_value='/tool'), mock.patch('builtins.input', side_effect=AssertionError('Unexpected launch confirmation')), mock.patch.dict(os.environ, {'HF_TOKEN': 'fixture-secret', 'OPENAI_API_KEY': 'cloud-secret'}, clear=True), mock.patch.object(setup.os, 'execve') as launch, contextlib.redirect_stdout(output):
                setup.main()
            argv, env = launch.call_args.args[1:]
            self.assertEqual(argv, ['run.sh', '--non-interactive', '--memory', '20G'])
            self.assertNotIn('HF_TOKEN', env)
            self.assertEqual(env.get('OPENAI_API_KEY'), 'cloud-secret')
            self.assertNotIn('cloud-secret', repr(argv))
            self.assertNotIn('cloud-secret', output.getvalue())
            self.assertNotIn('fixture-secret', output.getvalue())
            self.assertIn('Skipping cloud setup', output.getvalue())

    def test_saved_cloud_key_skips_every_cloud_prompt(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch('sys.argv', ['setup_run']), mock.patch.object(setup.shutil, 'which', return_value='/tool'), mock.patch('builtins.input', side_effect=['n']) as prompt, contextlib.redirect_stdout(io.StringIO()) as output:
            setup.save_secret('GEMINI_API_KEY', 'fixture-secret')
            self.assertEqual(setup.main(), 0)
            self.assertEqual(prompt.call_count, 1)
            self.assertIn('Already configured: Gemini', output.getvalue())
            self.assertNotIn('Which setup instructions', output.getvalue())
            self.assertNotIn('fixture-secret', output.getvalue())

    def test_cloud_setup_can_be_requested_explicitly(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch('sys.argv', ['setup_run', '--setup-cloud']), mock.patch.object(setup.shutil, 'which', return_value='/tool'), mock.patch.dict(os.environ, {'ANTHROPIC_API_KEY': 'fixture-secret'}), mock.patch('builtins.input', side_effect=['0', 'n']) as prompt, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(setup.main(), 0)
            self.assertIn('Which setup instructions', prompt.call_args_list[0].args[0])

    def test_download_retry_uses_reference_only(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch('sys.argv', ['setup_run', '--auth-file', '/private/hf-token']), mock.patch.object(setup.shutil, 'which', return_value='/tool'), mock.patch('builtins.input', side_effect=['0', 'y', 'y']), mock.patch.object(setup.subprocess, 'run', side_effect=[mock.Mock(returncode=1), mock.Mock(returncode=0)]) as download, mock.patch.object(setup.os, 'execve') as launch, contextlib.redirect_stdout(io.StringIO()):
            setup.main()
            launch.assert_called_once()
            self.assertEqual(download.call_count, 2)
            self.assertEqual(download.call_args.args[0][-2:], ['--auth-file', '/private/hf-token'])

    def test_cloud_setup_explains_selection_and_separate_api_billing(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(setup, 'ROOT', Path(tmp)), mock.patch('sys.argv', ['setup_run']), mock.patch.object(setup.shutil, 'which', return_value='/tool'), mock.patch('builtins.input', side_effect=['3', 'n', 'n']), contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(setup.main(), 0)
            self.assertIn('through verified HTTPS', output.getvalue())
            self.assertIn('/model openai', output.getvalue())
            self.assertIn('Gemma remains the default', output.getvalue())
            self.assertIn('ChatGPT subscription does not include API billing', output.getvalue())


if __name__ == '__main__':
    unittest.main()
