#!/usr/bin/env python3
"""Verify Forge's real QEMU UART cloud path with fixtures; makes no network calls."""
import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time

MAX_HTTP_FRAME_BYTES = 4 * 1024 * 1024 + 16384  # Forge cloud::HTTP_FRAME_LIMIT

MODELS = ('claude-sonnet-4-6', 'gemini-3.8-flash', 'gpt-4.1')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='alloyos-cloud-fixtures-') as directory:
        auth = Path(directory) / 'auth'
        auth.write_bytes(b'ANTHROPIC_API_KEY=fixture-key\0GEMINI_API_KEY=fixture-key\0OPENAI_API_KEY=fixture-key\0')
        auth.chmod(0o600)
        command = ['qemu-system-aarch64', '-M', 'virt', '-accel', 'hvf', '-cpu', 'host',
                   '-kernel', str(args.kernel), '-m', '512M', '-display', 'none', '-monitor', 'none',
                   '-device', 'virtio-gpu-device,xres=1206,yres=2622', '-serial', 'stdio',
                   '-fw_cfg', f'name=opt/alloyos/auth,file={auth}', '-no-reboot']
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, env={k: v for k, v in os.environ.items()
                                   if k not in ('ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'OPENAI_API_KEY')})
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        pending = bytearray()

        def send(line):
            process.stdin.write(line.encode())
            process.stdin.flush()

        def response(identifier):
            deadline = time.monotonic() + 30
            relays = 0
            while time.monotonic() < deadline:
                if b'\n' not in pending:
                    if not selector.select(1):
                        continue
                    chunk = os.read(process.stdout.fileno(), 4096)
                    if not chunk:
                        raise RuntimeError('QEMU exited before its reply')
                    pending.extend(chunk)
                    if len(pending) > MAX_HTTP_FRAME_BYTES:
                        raise RuntimeError('UART frame exceeded bounds')
                    continue
                raw, _, rest = pending.partition(b'\n')
                pending[:] = rest
                fields = raw.decode(errors='replace').strip('\r').split('\t')
                if fields[0] == 'FORGEHTTP1':
                    assert len(fields) == 4 and fields[2] == 'post'
                    request = json.loads(bytes.fromhex(fields[3]))
                    body = json.loads(request['body'])
                    assert 'fixture-key' not in request['url']
                    if request['url'] == 'https://api.anthropic.com/v1/messages':
                        assert body['model'] == MODELS[0]
                        assert ('x-api-key', 'fixture-key') in map(tuple, request['headers'])
                        reply = {'type': 'message', 'content': [{'type': 'text', 'text': 'Cloud fixture 東京'}], 'usage': {'output_tokens': 4}}
                    elif request['url'].startswith('https://generativelanguage.googleapis.com/v1beta/models/'):
                        assert request['url'].endswith(MODELS[1] + ':generateContent')
                        assert ('x-goog-api-key', 'fixture-key') in map(tuple, request['headers'])
                        reply = {'candidates': [{'content': {'parts': [{'text': 'Cloud fixture 東京'}]}}], 'usageMetadata': {'candidatesTokenCount': 4}}
                    elif request['url'] == 'https://api.openai.com/v1/responses':
                        assert body['model'] == MODELS[2]
                        assert ('Authorization', 'Bearer fixture-key') in map(tuple, request['headers'])
                        reply = {'status': 'completed', 'output': [{'type': 'message', 'role': 'assistant', 'content': [{'type': 'output_text', 'text': 'Cloud fixture 東京'}]}], 'usage': {'output_tokens': 4}}
                    else:
                        raise RuntimeError('Unexpected HTTPS destination')
                    time.sleep(1)  # Simulate a slow provider while the guest renders.
                    envelope = json.dumps({'status': 200, 'body': json.dumps(reply, ensure_ascii=False)}, ensure_ascii=False)
                    send(f'FORGEHTTP1\t{fields[1]}\tok\t{envelope.encode().hex()}\n')
                    relays += 1
                elif fields[:2] == ['FORGE1', str(identifier)]:
                    assert fields[2] == 'ok', 'Forge generation failed'
                    return bytes.fromhex(fields[3]).decode(), relays
            raise RuntimeError('QEMU cloud response timed out')

        try:
            # Wait for UART initialization and the UI's polling loop before sending.
            boot_deadline = time.monotonic() + 30
            while b'[systemui] Slint window online;' not in pending:
                if time.monotonic() > boot_deadline:
                    raise RuntimeError('QEMU did not finish boot: ' + pending.decode(errors='replace')[-1500:])
                if selector.select(1):
                    chunk = os.read(process.stdout.fileno(), 4096)
                    if not chunk:
                        raise RuntimeError('QEMU exited during boot')
                    pending.extend(chunk)
            send('FORGE1\t1\tstatus\n')
            status, relays = response(1)
            status = json.loads(status)
            assert status['default_model'] == 'gemma-2-2b-it'
            assert sum(model['state'] == 'ready' for model in status['models']) == 3
            for identifier, model in enumerate(MODELS, 2):
                send(f'FORGE1\t{identifier}\tgenerate\t{model}\t32\t{"Hello 東京".encode().hex()}\n')
                text, relays = response(identifier)
                assert text == 'Cloud fixture 東京' and relays == 1
                send(f'FORGE1\t{identifier + 100}\tstatus\n')
                updated, _ = response(identifier + 100)
                updated = json.loads(updated)
                assert updated['ui_frames'] - status['ui_frames'] > 20, 'UI stopped during cloud wait'
                status = updated
            print('All three QEMU cloud API fixture round trips passed (no network calls).')
        finally:
            selector.close()
            process.kill()
            process.wait()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
