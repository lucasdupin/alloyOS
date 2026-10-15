#!/usr/bin/env python3
"""QEMU + production run_ui + actual native-helper media acceptance.

The test adapter only changes the start endpoint to localhost and starts the
native helper with its explicit synthetic-capture flag. It never replaces PCM
forwarding, WebSocket handling, Forge, Welder, or MCP. Disk is copied privately.
Requires built kernel/disk, native helper, and voice_qemu_probe example.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import time
from mock_live import MockLive

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, default=ROOT / 'target/aarch64-unknown-none/release/alloyos-kernel')
    parser.add_argument('--disk', type=Path, default=ROOT / 'target/aarch64-unknown-none/release/cobaltfs.img')
    parser.add_argument('--preflight', action='store_true', help='Check built artifacts and private credential reference without devices/network')
    parser.add_argument('--live-device', action='store_true', help='Actual microphone and Gemini; plays deterministic macOS speech aloud')
    parser.add_argument('--summary', type=Path, default=Path('/private/tmp/alloyos-voice-live-summary.json'))
    parser.add_argument('--screenshot', type=Path, default=Path('/private/tmp/voice-native-qemu.ppm'))
    args = parser.parse_args()
    if args.preflight:
        for artifact in (args.kernel, args.disk, ROOT / 'target/alloyos-voice-helper', ROOT / 'target/debug/examples/voice_qemu_probe'):
            assert artifact.is_file(), 'missing artifact: ' + str(artifact)
        if args.live_device and not os.environ.get('GEMINI_API_KEY'):
            key = ROOT / '.secrets/GEMINI_API_KEY'
            assert key.is_file() and not key.is_symlink() and not key.parent.is_symlink() and key.stat().st_mode & 0o077 == 0, 'private Gemini credential reference unavailable'
        assert shutil.which('qemu-system-aarch64'), 'QEMU unavailable'
        print('PASS preflight: artifacts, QEMU and credential reference ready; no microphone or network opened')
        return
    overall_deadline = time.monotonic() + 44 if args.live_device else float('inf')
    if args.live_device:
        args.summary.unlink(missing_ok=True)
    mock = None if args.live_device else MockLive(tools=True)
    with tempfile.TemporaryDirectory(prefix='voice-native-') as directory:
        work = Path(directory)
        shutil.copyfile(args.disk, work / 'disk.img')
        adapter = work / 'native-adapter.py'
        adapter.write_text('#!/usr/bin/env python3\n' + '''import json,subprocess,sys
child=subprocess.Popen([HELPER,'--test-direct-audio'],stdin=subprocess.PIPE,stdout=sys.stdout,stderr=sys.stderr)
for line in sys.stdin:
 command=json.loads(line)
 if command['op']=='start':
  assert command.get('direct_audio') is True, 'guest did not request direct audio'
  command['endpoint']=ENDPOINT
 child.stdin.write((json.dumps(command)+'\\n').encode());child.stdin.flush()
child.stdin.close();child.wait(timeout=5)
'''.replace('HELPER', repr(str(ROOT / 'target/alloyos-voice-helper'))).replace('ENDPOINT', repr(mock.endpoint if mock else '')))
        if args.live_device:
            adapter.write_text('#!/bin/sh\nexec /usr/bin/python3 ' + str(ROOT / 'scripts/audio/trace_native.py') + '\n')
        adapter.chmod(0o700)
        environment = os.environ.copy()
        environment['ALLOYOS_VOICE_ACCEPTANCE_SUMMARY'] = str(args.summary)
        environment['ALLOYOS_VOICE_ACCEPTANCE_KERNEL_SHA256'] = hashlib.sha256(args.kernel.read_bytes()).hexdigest()
        process = subprocess.Popen([str(ROOT / 'target/debug/examples/voice_qemu_probe'),
                                    str(args.kernel), str(work / 'disk.img'), str(work / 'qmp.sock'), str(adapter)] + (['--live-device'] if args.live_device else []),
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, env=environment)
        pending = bytearray()
        log = []

        def pump(duration=.1):
            deadline = time.monotonic() + duration
            while time.monotonic() < deadline:
                assert time.monotonic() < overall_deadline, 'bounded live acceptance deadline exceeded'
                if select.select([process.stdout], [], [], .03)[0]:
                    data = os.read(process.stdout.fileno(), 65536)
                    assert data, 'QEMU runner exited: ' + '\n'.join(log[-12:])
                    pending.extend(data)
                    while b'\n' in pending:
                        raw, _, rest = pending.partition(b'\n')
                        pending[:] = rest
                        line = raw.decode(errors='replace')
                        if not args.live_device:
                            log.append(line)
                        elif any(token in line for token in ('Slint window online', 'EL0 Slint ready', 'Model boot phase finished.', '[voice ', '[voice-input]')):
                            log.append(line)
                        assert 'panicked at' not in line, line
                assert not mock or not mock.error, mock.error if mock else None

        def qmp(command, arguments=None):
            request = {'execute': command}
            if arguments is not None:
                request['arguments'] = arguments
            stream.write((json.dumps(request) + '\n').encode());stream.flush()
            while True:
                reply = json.loads(stream.readline())
                if 'event' in reply:
                    continue
                assert 'error' not in reply, reply
                return reply

        def click(x, y):
            qmp('input-send-event', {'events': [
                {'type': 'abs', 'data': {'axis': 'x', 'value': int(x / 1206 * 32767)}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': int(y / 2622 * 32767)}},
                {'type': 'btn', 'data': {'down': True, 'button': 'left'}},
                {'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})

        try:
            deadline = time.monotonic() + 45
            while not (work / 'qmp.sock').exists():
                pump()
                assert time.monotonic() < deadline, 'QMP startup timeout'
            channel = socket.socket(socket.AF_UNIX)
            channel.settimeout(5)
            channel.connect(str(work / 'qmp.sock'))
            stream = channel.makefile('rwb')
            stream.readline();qmp('qmp_capabilities')
            while not any('EL0 Slint ready' in line for line in log):
                pump()
                assert time.monotonic() < deadline, 'UI boot timeout'
            # Require actual rendered controls, rather than an arbitrary startup delay.
            # QEMU screenshots currently show a known stride skew, so examine
            # the whole bottom toolbar strip instead of its logical x position.
            previous_roi = None
            readiness_deadline = min(time.monotonic() + 8, overall_deadline)
            while True:
                screen = work / 'ready.ppm'
                qmp('screendump', {'filename': str(screen)})
                raw = screen.read_bytes()
                header, pixels = raw.split(b'\n255\n', 1)
                width, height = map(int, header.splitlines()[1].split())
                assert (width, height) == (1206, 2622), 'unexpected display size'
                roi = pixels[2380 * width * 3:2560 * width * 3]
                white = sum(min(roi[index:index + 3]) >= 180 for index in range(0, len(roi), 3))
                gray = sum(30 <= min(roi[index:index + 3]) <= 70 and max(roi[index:index + 3]) - min(roi[index:index + 3]) < 10 for index in range(0, len(roi), 3))
                if white > 100 and gray > 1000 and roi == previous_roi:
                    if not args.live_device:
                        shutil.copyfile(screen, '/private/tmp/voice-native-qemu-ready.ppm')
                    break
                previous_roi = roi
                assert time.monotonic() < readiness_deadline, 'UI readiness failed: bottom toolbar was not visibly stable'
                pump(.1)
            click(459, 2478)
            voice_clicked_at = time.monotonic()
            if args.live_device:
                deadline = time.monotonic() + 12
                def summary():
                    try:
                        return json.loads(args.summary.read_text())
                    except (OSError, ValueError):
                        return {}
                while not summary().get('provider_ready'):
                    pump()
                    assert not summary().get('errors'), 'native helper error before listening'
                    assert time.monotonic() < deadline, 'actual device/provider setup failed; inspect Terminal microphone permission'
                print('Real device and Gemini connected; speaking controlled clock request aloud.', flush=True)
                print('If output is routed to headphones, say aloud: Use the clock app to read system uptime.', flush=True)
                speech = subprocess.Popen(['/usr/bin/say', '-r', '150', 'Please use the clock app. Call the clock now tool to read the system uptime, and tell me the result.'])
                deadline = min(time.monotonic() + 23, overall_deadline - 2)
                while time.monotonic() < deadline:
                    pump()
                    facts = summary()
                    if facts.get('clock_result_returned') and facts.get('playback_started') and facts.get('output_transcripts'):
                        pump(2)
                        break
                speech.wait(timeout=3)
                facts = summary()
                helper_pid = facts.get('helper_pid')
                def helper_alive():
                    if not helper_pid:
                        return None
                    try:
                        os.kill(helper_pid, 0)
                        return True
                    except ProcessLookupError:
                        return False
                assert helper_alive(), 'actual helper already exited before stop request'
                click(603, 2478)
                pump(1)
                facts = summary()
                facts['ui_stop_requested'] = True
                facts['helper_exit_observed_after_ui_stop'] = helper_alive() is False
                facts['stopped'] = facts.get('stopped', False) or facts['helper_exit_observed_after_ui_stop']
                required = ('start_direct_audio', 'stats_direct_audio', 'provider_ready', 'speech_clock', 'input_transcripts', 'sent_frames', 'clock_called', 'clock_result_returned', 'output_transcripts', 'playback_started', 'stopped')
                facts['pass'] = all(facts.get(field) for field in required) and not facts.get('errors') and not facts.get('conversion_errors')
                facts['missing_gates'] = [field for field in required if not facts.get(field)]
                facts['physical_output_audibility'] = 'manual confirmation required; playback_started measures scheduled device playback'
                args.summary.write_text(json.dumps(facts, indent=2))
                print(json.dumps(facts, indent=2), flush=True)
                assert facts['pass'], 'Live device acceptance failed: ' + ', '.join(facts['missing_gates'])
                return
            deadline = time.monotonic() + 30
            while mock.frames < 70 or not any(item['id'] == 'verify-clock' for item in mock.responses):
                pump()
                if mock.first_audio:
                    deadline = mock.first_audio + 30
                if time.monotonic() >= deadline:
                    qmp('screendump', {'filename': str(args.screenshot)})
                assert time.monotonic() < deadline, f'VOICE FAILED: frames={mock.frames}, first_audio_latency={None if mock.first_audio is None else round(mock.first_audio-voice_clicked_at, 2)}, audio_duration={None if mock.first_audio is None else round(mock.last_audio-mock.first_audio, 2)}, elapsed={round(time.monotonic()-voice_clicked_at,2)}, responses={mock.responses}; ' + '\n'.join(log[-10:])
            assert mock.samples >= 112000
            discovery = next(item for item in mock.responses if item['id'] == 'verify-discovery')
            clock = next(item for item in mock.responses if item['id'] == 'verify-clock')
            assert 'clock.now' in json.dumps(discovery), discovery
            assert 'uptime_ms' in json.dumps(clock) and 'wall_clock_available' in json.dumps(clock), clock
            assert not any('VOICE1' in line for line in log), 'raw media/control leaked into diagnostics'
            qmp('screendump', {'filename': str(args.screenshot)})
            elapsed = time.monotonic() - voice_clicked_at
            first_audio_latency = mock.first_audio - voice_clicked_at
            print(f'PASS QEMU native media: {mock.frames} frames / {mock.samples} decoded samples; first audio {first_audio_latency:.2f}s after mic, total {elapsed:.2f}s')
            print('PASS QEMU Forge/Welder/MCP: discovery and real clock.now result returned to mock model')
            print(f'REVIEW chat transcript rendering: {args.screenshot}')
            print('LIMIT: synthetic capture and localhost model; real device/Gemini recognition not tested')
        finally:
            if not args.live_device:
                Path('/private/tmp/voice-native-qemu.log').write_text('\n'.join(log))
            if mock:
                mock.close()
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL);process.wait()


if __name__ == '__main__':
    main()
