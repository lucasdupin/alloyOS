#!/usr/bin/env python3
"""Verify actual native direct media forwarding against a localhost Live mock.

Requires scripts/audio/build.sh. Uses explicit --test-direct-audio capture/device
injection; WebSocket/audio forwarding and provider filtering are production code.
Does not test real devices, permissions, echo cancellation or Gemini recognition.
"""
import json
import os
from pathlib import Path
import select
import subprocess
import time
from mock_live import MockLive

ROOT = Path(__file__).resolve().parents[2]


def main():
    mock = MockLive()
    helper = subprocess.Popen([str(ROOT / 'target/alloyos-voice-helper'), '--test-direct-audio'],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    pending = bytearray()
    events = []

    def send(op, **fields):
        helper.stdin.write((json.dumps({'op': op, 'session': 41, **fields}) + '\n').encode())
        helper.stdin.flush()

    def pump(duration):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            assert helper.poll() is None, 'helper exited'
            if not select.select([helper.stdout], [], [], min(.05, max(0, deadline - time.monotonic())))[0]:
                continue
            data = os.read(helper.stdout.fileno(), 65536)
            assert data, 'helper stdout closed'
            pending.extend(data)
            while b'\n' in pending:
                raw, _, rest = pending.partition(b'\n')
                pending[:] = rest
                event = json.loads(raw)
                events.append(event)
                assert event['event'] != 'error', event
                assert event['event'] != 'audio', 'raw PCM leaked into guest control pipe'
            assert not mock.error, mock.error

    try:
        # Missing/false direct flag must fail before any capture/socket activity.
        for fields in ({}, {'direct_audio': False}):
            send('start', endpoint=mock.endpoint, api_key='synthetic-fixture', **fields)
            deadline = time.monotonic() + 2
            raw = bytearray()
            while b'\n' not in raw:
                assert select.select([helper.stdout], [], [], max(0, deadline-time.monotonic()))[0], 'missing direct flag was not rejected'
                raw.extend(os.read(helper.stdout.fileno(), 4096))
            response = json.loads(raw)
            assert response['event'] == 'error' and 'transport mismatch' in response.get('message', '').lower(), response
            assert mock.frames == 0 and not mock.messages, 'invalid configuration opened model transport'
        print('PASS native missing/false direct flag: rejected before capture or transport')
        send('start', endpoint=mock.endpoint, api_key='synthetic-fixture', direct_audio=True)
        pump(.3)
        assert any(event['event'] == 'ready' for event in events), 'helper not ready'
        send('send', json=json.dumps({'setup': {'model': 'models/fixture', 'generationConfig': {'responseModalities': ['AUDIO']}, 'inputAudioTranscription': {}, 'outputAudioTranscription': {}}}))
        pump(5.5)
        assert mock.frames >= 45, f'input stalled: only {mock.frames} frames reached WebSocket'
        assert mock.samples >= 72000, f'not realtime 16k PCM: {mock.samples} samples'
        assert any(event['event'] == 'audio_level' and event['level'] > 100 for event in events), 'missing voice level'
        assert any(event['event'] == 'message' and 'inputTranscription' in event['json'] for event in events), 'provider transcript not returned to guest'
        assert any(event['event'] == 'playback_active' and event['active'] for event in events), 'provider audio did not enter native playback'
        assert any(event['event'] == 'playback_active' and not event['active'] for event in events), 'playback did not complete'
        assert all('inlineData' not in event.get('json', '') for event in events), 'raw provider audio leaked into guest control pipe'
        stats = [event for event in events if event['event'] == 'audio_stats']
        assert stats and stats[-1].get('direct_audio') is True, 'native direct-audio mode not proven'
        assert stats[-1]['level'] > 100 and stats[-1]['conversion_errors'] == 0, 'converted PCM not proven'
        assert stats[-1]['sent_frames'] >= 35, 'successful sends not accounted'
        print(f'PASS native sustained media: {mock.frames} frames / {mock.samples} decoded samples at model boundary')
        send('mute', muted=True)
        pump(.3)  # Drain sends already in flight before asserting silence.
        at_mute = mock.frames
        pump(1.2)
        assert mock.frames == at_mute, 'microphone still transmits after mute'
        send('mute', muted=False)
        pump(.6)
        assert mock.frames >= at_mute + 4, 'capture did not resume'
        send('stop')
        pump(.3)
        assert any(event['event'] == 'stopped' for event in events), 'stop unacknowledged'
        at_stop = mock.frames
        pump(.5)
        assert mock.frames == at_stop, 'audio still transmits after stop'
        assert len(pending) < 131072
        print('PASS native mute/resume/stop: no post-drain media, helper remains responsive')
        print('LIMIT: does not prove QEMU chat history/MCP, real microphone, or model recognition')
    finally:
        mock.close()
        helper.kill()
        helper.wait(timeout=3)
        helper.stdin.close()
        helper.stdout.close()
        helper.stderr.close()


if __name__ == '__main__':
    main()
