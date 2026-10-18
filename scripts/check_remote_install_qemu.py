#!/usr/bin/env python3
"""Hermetic real-guest installation consent gate with a simulated model.

No network/OAuth is allowed. Copies the supplied CobaltFS disk, proposes one
synthetic public remote package, clicks native Cancel/Install and checks inventory.
"""
import argparse
import json
import os
from pathlib import Path
import selectors
import shutil
import socket
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, required=True)
    parser.add_argument('--disk', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    package = 'consentfixture'
    endpoint = 'https://docs.mcp.cloudflare.com/mcp'
    with tempfile.TemporaryDirectory(prefix='alloyos-install-gate-', dir='/tmp') as folder:
        root = Path(folder)
        disk = root / 'test.img'
        shutil.copy2(args.disk, disk)
        auth = root / 'auth'
        auth.write_bytes(b'GEMINI_API_KEY=fixture-not-a-real-key\0')
        auth.chmod(0o600)
        qmp_path = root / 'qmp.sock'
        command = ['qemu-system-aarch64', '-M', 'virt,gic-version=3', '-accel', 'hvf',
                   '-cpu', 'host', '-kernel', str(args.kernel.resolve()), '-m', '512M',
                   '-display', 'none', '-serial', 'stdio', '-no-reboot',
                   '-device', 'virtio-gpu-device,xres=1206,yres=2622',
                   '-device', 'virtio-keyboard-device', '-device', 'virtio-tablet-device',
                   '-drive', f'file={disk},if=none,format=raw,id=hd0',
                   '-device', 'virtio-blk-device,drive=hd0',
                   '-qmp', f'unix:{qmp_path},server=on,wait=off',
                   '-fw_cfg', f'name=opt/alloyos/auth,file={auth}']
        environment = {k: v for k, v in os.environ.items()
                       if k not in ('GEMINI_API_KEY', 'OPENAI_API_KEY', 'ANTHROPIC_API_KEY')}
        guest = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, env=environment)
        selector = selectors.DefaultSelector()
        selector.register(guest.stdout, selectors.EVENT_READ)
        pending = bytearray()
        calls = 0
        rpc_id = 0
        serial = []

        def send(line):
            guest.stdin.write(line.encode())
            guest.stdin.flush()

        def line(deadline):
            while b'\n' not in pending:
                if guest.poll() is not None:
                    raise RuntimeError('QEMU exited: ' + guest.stderr.read().decode(errors='replace'))
                if time.monotonic() >= deadline:
                    raise RuntimeError('guest consent gate timed out')
                if selector.select(.2):
                    chunk = os.read(guest.stdout.fileno(), 65536)
                    if not chunk:
                        raise RuntimeError('guest UART disconnected')
                    pending.extend(chunk)
                    if len(pending) > 4 * 1024 * 1024 + 16384:
                        raise RuntimeError('UART frame exceeds limit')
            data, _, rest = pending.partition(b'\n')
            pending[:] = rest
            return data.decode(errors='replace').rstrip('\r')

        def response(identifier):
            nonlocal calls
            deadline = time.monotonic() + 45
            while True:
                data = line(deadline)
                fields = data.split('\t')
                if fields[0] in ('MCPHTTP1', 'ACCOUNT1'):
                    raise RuntimeError('installation attempted network/account access')
                if fields[0] == 'FORGEHTTP1':
                    if len(fields) != 4 or fields[2] != 'post':
                        raise RuntimeError('unexpected model envelope')
                    request = json.loads(bytes.fromhex(fields[3]))
                    if not request['url'].startswith('https://generativelanguage.googleapis.com/v1beta/models/'):
                        raise RuntimeError('unexpected model destination')
                    if calls % 2 == 0:
                        model = {'kind': 'tools', 'calls': [{'name': 'mcp_install_remote',
                                 'arguments': {'app_id': package, 'name': 'Consent fixture',
                                               'endpoint': endpoint}}]}
                    else:
                        model = {'kind': 'final', 'text': 'Review the native installation confirmation.'}
                    calls += 1
                    if calls > 4:
                        raise RuntimeError('simulated model call budget exceeded')
                    result = {'candidates': [{'finishReason': 'STOP', 'content': {'parts': [
                        {'text': json.dumps(model)}]}}], 'usageMetadata': {'candidatesTokenCount': 40}}
                    envelope = {'status': 200, 'body': json.dumps(result)}
                    send(f'FORGEHTTP1\t{fields[1]}\tok\t{json.dumps(envelope).encode().hex()}\n')
                elif fields[:2] == ['FORGE1', str(identifier)]:
                    if fields[2] != 'ok':
                        raise RuntimeError('guest console request failed: ' + data)
                    return json.loads(bytes.fromhex(fields[3]))
                else:
                    serial.append(data)

        def chat(text):
            nonlocal rpc_id
            rpc_id += 1
            send(f'FORGE1\t{rpc_id}\tchat\t{text.encode().hex()}\n')
            return response(rpc_id)

        channel = None
        try:
            deadline = time.monotonic() + 40
            while '[systemui] Slint window online;' not in line(deadline):
                pass
            channel = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            channel.settimeout(5)
            channel.connect(str(qmp_path))
            stream = channel.makefile('rwb')
            stream.readline()

            def qmp(command, arguments=None):
                message = {'execute': command}
                if arguments is not None:
                    message['arguments'] = arguments
                stream.write((json.dumps(message) + '\n').encode())
                stream.flush()
                while True:
                    reply = json.loads(stream.readline())
                    if 'event' in reply:
                        continue
                    if 'error' in reply:
                        raise RuntimeError(str(reply['error']))
                    return reply

            def click(x, y):
                qmp('input-send-event', {'events': [
                    {'type': 'abs', 'data': {'axis': 'x', 'value': int(x / 1206 * 32767)}},
                    {'type': 'abs', 'data': {'axis': 'y', 'value': int(y / 2622 * 32767)}},
                    {'type': 'btn', 'data': {'down': True, 'button': 'left'}},
                    {'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})

            def capture(name):
                qmp('screendump', {'filename': str((args.output / name).resolve()), 'format': 'png'})

            qmp('qmp_capabilities')
            chat('/model gemini')  # isolated fake-key test guest only
            assert package not in chat('/apps')['text'], 'fixture already exists'
            for approved in (False, True):
                proposal = chat('Propose registering the consent fixture service.')
                outputs = [entry['output'] for entry in proposal['tools'] if entry['name'] == 'mcp_install_remote']
                assert outputs and outputs[-1]['awaiting_user_approval'] is True
                assert outputs[-1]['installed'] is False
                assert package not in chat('/apps')['text'], 'proposal wrote package before approval'
                time.sleep(.4)
                capture('install-review.png')
                click(850 if approved else 350, 1945)
                time.sleep(1)
                inventory = chat('/apps')['text']
                assert (package in inventory) is approved, inventory
                capture('installed.png' if approved else 'canceled.png')
                print('PASS: native Install registered package; no account connected' if approved
                      else 'PASS: native Cancel left package inventory unchanged', flush=True)
            assert calls == 4
            (args.output / 'serial.log').write_text('\n'.join(serial) + '\n')
            print('PASS: real guest consent gate; model simulated, no network/OAuth', flush=True)
        finally:
            (args.output / 'serial.log').write_text('\n'.join(serial) + '\n')
            if channel is not None:
                channel.close()
            selector.close()
            guest.kill()
            guest.wait()


if __name__ == '__main__':
    main()
