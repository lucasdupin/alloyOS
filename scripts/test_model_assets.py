import contextlib
import hashlib
import importlib.util
import io
import pathlib
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('model_assets',pathlib.Path(__file__).with_name('download_gemma.py'))
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)

class ModelAssetsTests(unittest.TestCase):
    def test_progress_reports_resume_speed_eta_and_waiting(self):
        output = io.StringIO()
        with mock.patch.object(assets.time, 'monotonic', return_value=0):
            progress = assets.Progress('Downloading', 100 * 2**20, 40 * 2**20, stream=output)
            progress.update(60 * 2**20)
        line = progress.line(2)
        self.assertIn('60.0%', line)
        self.assertIn('10.0 MiB/s', line)
        self.assertIn('ETA 4s', line)
        self.assertIn('waiting 6s', progress.line(6))

    def test_terminal_bar_finishes_on_its_own_line(self):
        output = io.StringIO()
        output.isatty = lambda: True
        with assets.Progress('Downloading', 10, stream=output) as progress:
            progress.update(10)
        self.assertIn('\r\033[2K', output.getvalue())
        self.assertIn('100.0%', output.getvalue())
        self.assertTrue(output.getvalue().endswith('\n'))
        self.assertFalse(progress.thread.is_alive())

    def test_progress_cleans_up_after_failed_read(self):
        output = io.StringIO()
        with self.assertRaises(OSError):
            with assets.Progress('Downloading', 10, stream=output) as progress:
                raise OSError('fixture-secret')
        self.assertFalse(progress.thread.is_alive())
        self.assertNotIn('fixture-secret', output.getvalue())
        self.assertNotIn('\033', output.getvalue())

    def test_packaging_and_integrity(self):
        with tempfile.TemporaryDirectory() as directory:
            src = pathlib.Path(directory)/'model.gguf'
            output = pathlib.Path(directory)/'model.img'
            contents = b'GGUF' + bytes(range(256))
            src.write_bytes(contents)
            sha = hashlib.sha256(contents).hexdigest()
            assets.package(src,output,sha,len(contents))
            image = output.read_bytes()
            self.assertEqual(image[:8],b'FGMODEL1')
            self.assertEqual(int.from_bytes(image[8:16],'little'),len(contents))
            self.assertEqual(image[16:48],bytes.fromhex(sha))
            self.assertEqual(image[4096:4096+len(contents)],contents)
            self.assertEqual(len(image)%512,0)
            with self.assertRaises(ValueError):
                assets.package(src,output,'0'*64,len(contents))
            self.assertFalse(output.with_suffix('.img.part').exists())

    def test_resume_ignoring_range_restarts(self):
        contents = b'GGUFfixture'
        class Response(io.BytesIO):
            status = 200
            headers = {}
        class Opener:
            def open(self, request, timeout):
                self.request = request
                return Response(contents)
        with tempfile.TemporaryDirectory() as directory:
            src = pathlib.Path(directory)/'model.gguf'
            src.with_suffix('.gguf.part').write_bytes(b'GGU')
            opener = Opener()
            with mock.patch.object(assets,'SIZE',len(contents)), mock.patch.object(assets,'SHA256',hashlib.sha256(contents).hexdigest()):
                assets.download(src,'fixture-secret',opener)
            self.assertEqual(src.read_bytes(),contents)
            self.assertEqual(opener.request.get_header('Range'),'bytes=3-')

    def test_error_never_prints_secret(self):
        error = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, mock.patch('sys.argv',['download_gemma','--output-dir',directory]), mock.patch.dict('os.environ',{'HF_TOKEN':'fixture-secret'}), mock.patch.object(assets,'download',side_effect=Exception('Authorization: fixture-secret')), contextlib.redirect_stderr(error):
            self.assertEqual(assets.main(),1)
        self.assertNotIn('fixture-secret',error.getvalue())

    def test_missing_token_explains_fix_without_network(self):
        error = io.StringIO()
        with tempfile.TemporaryDirectory() as directory, mock.patch('sys.argv', ['download_gemma', '--output-dir', directory]), mock.patch.dict('os.environ', {'HF_TOKEN': ''}), mock.patch.object(assets, 'download') as download, contextlib.redirect_stderr(error):
            self.assertEqual(assets.main(), 1)
        download.assert_not_called()
        self.assertIn('No Hugging Face token found', error.getvalue())
        self.assertIn('--auth-file', error.getvalue())
        self.assertIn('Gemma license', error.getvalue())

    def test_http_diagnostics_redact_response_details(self):
        for code, expected in [(401, 'valid read token'), (403, 'Accept the Gemma license'), (404, 'pinned model file'), (429, 'Wait and rerun'), (503, 'HTTP 503')]:
            with self.subTest(code=code):
                failure = assets.urllib.error.HTTPError('https://example.com/?token=fixture-secret', code, 'fixture-secret', {'Authorization': 'fixture-secret'}, None)
                message = assets.failure_message(failure)
                failure.close()
                self.assertIn(expected, message)
                self.assertNotIn('fixture-secret', message)
                self.assertNotIn('example.com', message)

    def test_local_and_network_diagnostics(self):
        cases = [
            (OSError(assets.errno.ENOSPC, 'fixture-secret'), '21 GB'),
            (OSError(assets.errno.EACCES, 'fixture-secret'), 'Permission denied'),
            (FileNotFoundError(assets.errno.ENOENT, 'fixture-secret'), '--auth-file'),
            (TimeoutError('fixture-secret'), 'timed out'),
            (assets.urllib.error.URLError('fixture-secret'), 'HTTPS certificates'),
        ]
        for failure, expected in cases:
            message = assets.failure_message(failure)
            self.assertIn(expected, message)
            self.assertNotIn('fixture-secret', message)

    def test_redirect_strips_auth(self):
        request = assets.urllib.request.Request('https://huggingface.co/model',headers={'Authorization':'Bearer fixture-secret'})
        redirected = assets.SafeRedirect().redirect_request(request,None,302,'',{},'https://cdn.example/model')
        self.assertIsNone(redirected.get_header('Authorization'))
        with self.assertRaises(ValueError):
            assets.SafeRedirect().redirect_request(request,None,302,'',{},'http://cdn.example/model')

if __name__ == '__main__': unittest.main()
