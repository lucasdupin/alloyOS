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

    def test_redirect_strips_auth(self):
        request = assets.urllib.request.Request('https://huggingface.co/model',headers={'Authorization':'Bearer fixture-secret'})
        redirected = assets.SafeRedirect().redirect_request(request,None,302,'',{},'https://cdn.example/model')
        self.assertIsNone(redirected.get_header('Authorization'))
        with self.assertRaises(ValueError):
            assets.SafeRedirect().redirect_request(request,None,302,'',{},'http://cdn.example/model')

if __name__ == '__main__': unittest.main()
