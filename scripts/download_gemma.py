#!/usr/bin/env python3
"""Download pinned Gemma weights and build a credential-free read-only model image."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys
import urllib.request
import urllib.parse

REPO = 'google/gemma-2-2b-it-GGUF'
REVISION = '67803c710cb0c89bae2047a5f50a2481c9339d24'
SHA256 = '8fc1a799613b873edd085ffcf33fd710077daf03833ead29e136cca9b528ecb6'
SIZE = 10463413632
FILENAME = '2b_it_v2.gguf'

class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.startswith('https://'):
            raise ValueError('insecure model redirect rejected')
        result = super().redirect_request(req, fp, code, msg, headers, newurl)
        # Never forward HF credentials to signed CDN storage URLs.
        if urllib.parse.urlparse(newurl).netloc != 'huggingface.co':
            result.remove_header('Authorization')
        return result

def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as src:
        while chunk := src.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()

def verify(path, expected_sha, expected_size):
    if path.stat().st_size != expected_size or digest(path) != expected_sha:
        raise ValueError('model integrity check failed')
    with path.open('rb') as src:
        if src.read(4) != b'GGUF':
            raise ValueError('model is not GGUF')

def download(path, token, opener=None):
    opener = opener or urllib.request.build_opener(SafeRedirect())
    if path.exists():
        verify(path, SHA256, SIZE)
        return
    partial = path.with_suffix(path.suffix + '.part')
    start = partial.stat().st_size if partial.exists() else 0
    if start > SIZE:
        raise ValueError('partial model exceeds pinned size')
    if start == SIZE:
        verify(partial, SHA256, SIZE)
        os.replace(partial, path)
        return
    headers = {'Authorization': 'Bearer ' + token} if token else {}
    if start:
        headers['Range'] = f'bytes={start}-'
    url = f'https://huggingface.co/{REPO}/resolve/{REVISION}/{FILENAME}'
    with opener.open(urllib.request.Request(url, headers=headers), timeout=120) as response:
        append = start and response.status == 206
        if append and not response.headers.get('Content-Range', '').startswith(f'bytes {start}-'):
            raise ValueError('invalid resume response')
        with partial.open('ab' if append else 'wb') as output:
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
                if output.tell() > SIZE:
                    raise ValueError('download exceeds pinned size')
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
    with source.open('rb') as src, partial.open('wb') as dest:
        dest.write(header)
        while chunk := src.read(1024 * 1024):
            dest.write(chunk)
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
                raise ValueError('credential file must be small and private')
            token = args.auth_file.read_text().strip()
        else:
            token = os.environ.get(args.auth_env or 'HF_TOKEN', '').strip()
        if '\n' in token or '\r' in token:
            raise ValueError('invalid credential')
        args.output_dir.mkdir(parents=True, exist_ok=True)
        source = args.output_dir / FILENAME
        if not args.package_only:
            download(source, token)
        package(source, args.output_dir / 'model.img')
        (args.output_dir / 'manifest.json').write_text(json.dumps({'id':'gemma-2-2b-it','repository':REPO,'revision':REVISION,'filename':FILENAME,'sha256':SHA256,'size':SIZE,'image':'model.img','payload_offset':4096}, indent=2) + '\n')
        print('Verified Gemma weights and created local model image.')
    except Exception:
        # Exceptions can contain request URLs/headers and signed storage URLs.
        print('Model acquisition failed. Check license access, local authentication, connectivity, disk space, and pinned integrity.', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    sys.exit(main())
