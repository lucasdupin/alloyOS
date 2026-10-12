#!/usr/bin/env python3
"""Type a real local chat in QEMU; check rendering, input, and the first reply."""
import argparse
import json
import os
from pathlib import Path
import selectors
import socket
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, required=True)
    parser.add_argument('--model-image', type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='mlc-') as directory:
        qmp_path = str(Path(directory) / 'qmp.sock')
        command = ['qemu-system-aarch64', '-M', 'virt', '-accel', 'hvf', '-cpu', 'host',
                   '-kernel', str(args.kernel), '-m', '16G', '-display', 'none', '-monitor', 'none',
                   '-device', 'virtio-gpu-device,xres=1206,yres=2622',
                   '-device', 'virtio-keyboard-device', '-device', 'virtio-tablet-device',
                   '-serial', 'stdio', '-qmp', f'unix:{qmp_path},server=on,wait=off',
                   '-drive', f'file={args.model_image.resolve()},if=none,format=raw,readonly=on,id=model',
                   '-device', 'virtio-blk-device,drive=model', '-no-reboot']
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, env={k: v for k, v in os.environ.items()
                                   if k not in ('ANTHROPIC_API_KEY', 'GEMINI_API_KEY', 'OPENAI_API_KEY', 'HF_TOKEN')})
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        pending = bytearray()
        log = []
        channel = None

        def line(timeout=30):
            deadline = time.monotonic() + timeout
            while b'\n' not in pending:
                if time.monotonic() > deadline:
                    raise RuntimeError('Guest stalled: ' + '\n'.join(log[-20:]))
                if selector.select(.1):
                    chunk = os.read(process.stdout.fileno(), 4096)
                    if not chunk:
                        raise RuntimeError('QEMU exited: ' + process.stderr.read().decode(errors='replace'))
                    pending.extend(chunk)
            raw, _, rest = pending.partition(b'\n')
            pending[:] = rest
            result = raw.decode(errors='replace').strip('\r')
            log.append(result)
            return result

        def status(identifier):
            start = time.monotonic()
            process.stdin.write(f'FORGE1\t{identifier}\tstatus\n'.encode())
            process.stdin.flush()
            while True:
                fields = line(5).split('\t')
                if fields[:2] == ['FORGE1', str(identifier)]:
                    assert fields[2] == 'ok'
                    result = json.loads(bytes.fromhex(fields[3]))
                    assert time.monotonic() - start < 1, 'UI thread blocked while answering status'
                    return result

        def qmp(execute, arguments=None):
            message = {'execute': execute}
            if arguments is not None:
                message['arguments'] = arguments
            stream.write((json.dumps(message) + '\n').encode())
            stream.flush()
            while True:
                result = json.loads(stream.readline())
                if 'event' in result:
                    continue
                assert 'error' not in result, result
                return result

        def key(value):
            qmp('input-send-event', {'events': [
                {'type': 'key', 'data': {'down': True, 'key': {'type': 'qcode', 'data': value}}},
                {'type': 'key', 'data': {'down': False, 'key': {'type': 'qcode', 'data': value}}}]})
            time.sleep(.05)

        try:
            deadline = time.monotonic() + 30
            while not Path(qmp_path).exists():
                assert process.poll() is None, 'QEMU exited: ' + process.stderr.read().decode(errors='replace') if process.poll() is not None else 'QEMU exited'
                assert time.monotonic() < deadline, 'QMP did not start'
                time.sleep(.02)
            channel = socket.socket(socket.AF_UNIX)
            channel.settimeout(10)
            channel.connect(qmp_path)
            stream = channel.makefile('rwb')
            assert 'QMP' in json.loads(stream.readline())
            qmp('qmp_capabilities')
            deadline = time.monotonic() + 180
            while '[systemui] Model boot phase finished.' not in line():
                assert time.monotonic() < deadline, 'Model did not initialize'
            assert any('[forge] Local model ready.' in item for item in log)
            print('Real Gemma model loaded; submitting keyboard chat.', flush=True)
            qmp('input-send-event', {'events': [
                {'type': 'abs', 'data': {'axis': 'x', 'value': int(300 / 1206 * 32767)}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': int(2500 / 2622 * 32767)}},
                {'type': 'btn', 'data': {'down': True, 'button': 'left'}},
                {'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})
            time.sleep(.3)
            for letter in 'hellow':
                key(letter)
            began = time.monotonic()
            key('ret')
            before = status(101)
            assert before['generation_pending'], 'Typed message did not start local inference'
            key('d')  # Edit the next draft while inference is still running.
            time.sleep(1)
            after = status(102)
            assert after['ui_frames'] - before['ui_frames'] >= 30, 'Rendering stalled during local inference'
            assert after['ui_input_events'] > before['ui_input_events'], 'Keyboard input stalled during inference'
            print(f"Inference pending: {after['ui_frames'] - before['ui_frames']} frames rendered; keyboard input accepted.", flush=True)
            deadline = time.monotonic() + 180
            identifier = 103
            while not any('[welder] Reply text advanced.' in item for item in log):
                assert time.monotonic() < deadline, 'No first reply token within three minutes'
                time.sleep(1)
                status(identifier)
                identifier += 1
            print(f'Local hellow chat stayed responsive; first text after {time.monotonic() - began:.1f}s.')
        finally:
            if channel:
                channel.close()
            selector.close()
            process.kill()
            process.wait()


if __name__ == '__main__':
    main()
