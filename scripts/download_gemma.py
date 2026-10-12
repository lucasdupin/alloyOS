#!/usr/bin/env python3
"""Download pinned Gemma weights and build a credential-free read-only model image."""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import socket
import threading
import time
import urllib.error
import urllib.request
import urllib.parse

REPO = 'google/gemma-2-2b-it-GGUF'
REVISION = '67803c710cb0c89bae2047a5f50a2481c9339d24'
SHA256 = '8fc1a799613b873edd085ffcf33fd710077daf03833ead29e136cca9b528ecb6'
SIZE = 10463413632
FILENAME = '2b_it_v2.gguf'

class AcquisitionError(ValueError):
    """A diagnostic written by us, safe to display without request data."""

class Progress:
    """Throttled byte progress; heartbeat also covers blocked network reads."""
    def __init__(self, label, total, initial=0, stream=None):
        self.label, self.total = label, total
        self.stream = stream if stream is not None else sys.stderr
        self.tty = self.stream.isatty()
        self.initial = self.completed = initial
        self.started = self.changed = time.monotonic()
        self.lock = threading.Lock()
        self.stop = threading.Event()

    def update(self, completed):
        with self.lock:
            self.completed = completed
            self.changed = time.monotonic()

    def line(self, now):
        with self.lock:
            completed, changed = self.completed, self.changed
        ratio = min(completed / max(self.total, 1), 1)
        filled = int(ratio * 16)
        speed = max(completed - self.initial, 0) / max(now - self.started, 0.001)
        eta = f'{int((self.total - completed) / speed)}s' if speed > 0 else '--'
        activity = f'waiting {int(now - changed)}s' if now - changed >= 5 and completed < self.total else f'ETA {eta}'
        return (f'{self.label} [{"#" * filled}{"-" * (16 - filled)}] '
                f'{ratio:5.1%} {completed / 2**30:.2f}/{self.total / 2**30:.2f} GiB '
                f'{speed / 2**20:.1f} MiB/s {activity}')

    def render(self, final=False):
        line = self.line(time.monotonic())
        self.stream.write(('\r\033[2K' if self.tty else '') + line + ('\n' if final or not self.tty else ''))
        self.stream.flush()

    def heartbeat(self):
        while not self.stop.wait(0.5 if self.tty else 10):
            self.render()

    def __enter__(self):
        self.render()
        self.thread = threading.Thread(target=self.heartbeat, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stop.set()
        self.thread.join()
        self.render(final=True)

def failure_message(error):
    if isinstance(error, AcquisitionError):
        return str(error)
    if isinstance(error, urllib.error.HTTPError):
        if error.code == 401:
            return 'Hugging Face rejected authentication (HTTP 401). Use a valid read token with access to this model via HF_TOKEN or --auth-file.'
        if error.code == 403:
            return f'Hugging Face denied model access (HTTP 403). Accept the Gemma license at https://huggingface.co/{REPO} using the account that owns your token, and grant the token read access to gated models.'
        if error.code == 404:
            return 'The pinned model file was not found (HTTP 404). Check repository access; if access is granted, the pinned repository revision may need updating.'
        if error.code == 429:
            return 'Hugging Face rate limited the download (HTTP 429). Wait and rerun; the partial download will resume.'
        return f'Model server returned HTTP {error.code}. Rerun later; the partial download will resume.'
    if isinstance(error, (TimeoutError, socket.timeout)):
        return 'Model download timed out. Check your connection and rerun; the partial download will resume.'
    if isinstance(error, urllib.error.URLError):
        return 'Could not connect securely to the model server. Check internet access, proxy settings, and Python HTTPS certificates, then rerun.'
    if isinstance(error, OSError):
        if error.errno == errno.ENOSPC:
            return 'Not enough disk space for Gemma. Allow approximately 21 GB for weights and the image, then rerun.'
        if error.errno in (errno.EACCES, errno.EPERM):
            return 'Permission denied reading credentials or writing model assets. Check token-file access and write access to the output directory.'
        if error.errno == errno.ENOENT:
            return 'A required local file is missing. Check --auth-file; for --package-only, download the weights first.'
        return 'Local file operation failed. Check output-directory access and available disk space.'
    return 'Unexpected model acquisition failure. Rerun; if it persists, report the failure without including credentials or signed URLs.'

class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.startswith('https://'):
            raise AcquisitionError('Model server redirected to insecure HTTP. Download stopped; check proxy settings or report the server redirect.')
        result = super().redirect_request(req, fp, code, msg, headers, newurl)
        # Never forward HF credentials to signed CDN storage URLs.
        if urllib.parse.urlparse(newurl).netloc != 'huggingface.co':
            result.remove_header('Authorization')
        return result

def digest(path):
    h = hashlib.sha256()
    with Progress('Verifying', path.stat().st_size) as progress, path.open('rb') as src:
        while chunk := src.read(1024 * 1024):
            h.update(chunk)
            progress.update(src.tell())
    return h.hexdigest()

def verify(path, expected_sha, expected_size):
    if path.stat().st_size != expected_size or digest(path) != expected_sha:
        raise AcquisitionError('Model integrity check failed. Remove the local 2b_it_v2.gguf and its .part file from the output directory, then rerun to download the pinned weights again.')
    with path.open('rb') as src:
        if src.read(4) != b'GGUF':
            raise AcquisitionError('Downloaded weights are not GGUF. Remove the local weights and rerun the downloader.')

def download(path, token, opener=None):
    opener = opener or urllib.request.build_opener(SafeRedirect())
    if path.exists():
        verify(path, SHA256, SIZE)
        return
    partial = path.with_suffix(path.suffix + '.part')
    start = partial.stat().st_size if partial.exists() else 0
    if start > SIZE:
        raise AcquisitionError('Partial download is larger than the pinned model. Remove 2b_it_v2.gguf.part from the output directory and rerun.')
    if start == SIZE:
        verify(partial, SHA256, SIZE)
        os.replace(partial, path)
        return
    headers = {'Authorization': 'Bearer ' + token} if token else {}
    if start:
        headers['Range'] = f'bytes={start}-'
    url = f'https://huggingface.co/{REPO}/resolve/{REVISION}/{FILENAME}'
    print('Connecting to Hugging Face; interrupted downloads resume automatically.', file=sys.stderr, flush=True)
    with Progress('Downloading', SIZE, start) as progress, opener.open(urllib.request.Request(url, headers=headers), timeout=120) as response:
        append = start and response.status == 206
        if append and not response.headers.get('Content-Range', '').startswith(f'bytes {start}-'):
            raise AcquisitionError('Server returned an invalid download resume range. Remove 2b_it_v2.gguf.part from the output directory and rerun.')
        with partial.open('ab' if append else 'wb') as output:
            # A server ignoring Range restarts the transfer from zero.
            if not append:
                with progress.lock:
                    progress.initial = 0
                progress.update(0)
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
                if output.tell() > SIZE:
                    raise AcquisitionError('Download exceeded the pinned model size. Remove 2b_it_v2.gguf.part and check model access before rerunning.')
                progress.update(output.tell())
            output.flush()
            os.fsync(output.fileno())
    verify(partial, SHA256, SIZE)
    os.replace(partial, path)

def package(source, output, expected_sha=SHA256, expected_size=SIZE):
    verify(source, expected_sha, expected_size)
    partial = output.with_suffix(output.suffix + '.part')
    header = bytearray(4096)
    header[:8] = b'FGMODEL1'
    struct.pack_into('<Q', header, 8, expected_size)
    header[16:48] = bytes.fromhex(expected_sha)
    header[48:48 + len(b'gemma-2-2b-it')] = b'gemma-2-2b-it'
    with Progress('Packaging', expected_size) as progress, source.open('rb') as src, partial.open('wb') as dest:
        dest.write(header)
        while chunk := src.read(1024 * 1024):
            dest.write(chunk)
            progress.update(src.tell())
        padding = (-dest.tell()) % 512
        dest.write(bytes(padding))
        dest.flush()
        os.fsync(dest.fileno())
    os.replace(partial, output)

def main():
    p = argparse.ArgumentParser(description=__doc__)
    auth = p.add_mutually_exclusive_group()
    auth.add_argument('--auth-env', default=None, metavar='VARIABLE', help='Environment variable name containing HF token (default HF_TOKEN)')
    auth.add_argument('--auth-file', type=Path, help='Private local file containing HF token')
    p.add_argument('--output-dir', type=Path, default=Path('.local/models/gemma-2-2b-it'))
    p.add_argument('--package-only', action='store_true')
    args = p.parse_args()
    try:
        if args.auth_file:
            stat = args.auth_file.stat()
            if stat.st_size > 16384 or stat.st_mode & 0o077:
                raise AcquisitionError('Token file must be private and smaller than 16 KiB. Run chmod 600 on your token file and store only the Hugging Face token in it.')
            token = args.auth_file.read_text().strip()
        else:
            token = os.environ.get(args.auth_env or 'HF_TOKEN', '').strip()
        if '\n' in token or '\r' in token:
            raise AcquisitionError('Token contains multiple lines. Provide only one Hugging Face token in the environment variable or private token file.')
        args.output_dir.mkdir(parents=True, exist_ok=True)
        source = args.output_dir / FILENAME
        if not args.package_only:
            if not source.exists() and not token:
                raise AcquisitionError(f'No Hugging Face token found. Set {args.auth_env or "HF_TOKEN"} locally, or run ./run.sh --auth-file /private/path/hf-token with a nonempty token file (chmod 600). First accept the Gemma license at https://huggingface.co/{REPO}. Never put the token in command arguments or Git.')
            download(source, token)
        package(source, args.output_dir / 'model.img')
        (args.output_dir / 'manifest.json').write_text(json.dumps({'id':'gemma-2-2b-it','repository':REPO,'revision':REVISION,'filename':FILENAME,'sha256':SHA256,'size':SIZE,'image':'model.img','payload_offset':4096}, indent=2) + '\n')
        print('Verified Gemma weights and created local model image.')
    except Exception as error:
        # Exceptions can contain request URLs/headers and signed storage URLs.
        print('Model acquisition failed: ' + failure_message(error), file=sys.stderr)
        if isinstance(error, urllib.error.HTTPError):
            error.close()
        return 1
    return 0

if __name__ == '__main__':
    sys.exit(main())
