#!/usr/bin/env python3
"""Transparent real-helper observer for explicitly authorized live acceptance.

Persists only numeric/fixed boolean facts. Never retains PCM, keys or transcript
contents. The native helper runs in normal device/network mode, without mocks.
"""
import json
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
SUMMARY = Path(os.environ['ALLOYOS_VOICE_ACCEPTANCE_SUMMARY'])
state = {'pass': False, 'kernel_sha256': os.environ.get('ALLOYOS_VOICE_ACCEPTANCE_KERNEL_SHA256'),
         'helper_sha256': hashlib.sha256((ROOT / 'target/alloyos-voice-helper').read_bytes()).hexdigest(), 'mode': 'actual-device-gemini', 'provider_ready': False, 'speech_clock': False,
         'speech_uptime': False, 'input_transcripts': 0, 'output_transcripts': 0,
         'mcp_discovery_called': False, 'clock_called': False, 'clock_result_returned': False,
         'playback_started': 0, 'playback_finished': 0, 'captured_samples': 0,
         'sent_frames': 0, 'emitted_frames': 0, 'captured_frames': 0, 'native_level_max': 0, 'conversion_errors': 0, 'muted': False, 'start_direct_audio': None, 'stats_direct_audio': None, 'level_max': 0, 'stopped': False, 'errors': 0, 'last_error_stage': '', 'last_progress': ''}
lock = threading.Lock()


def save():
    # Single writer guarded by lock. Contains no free-form provider/device text.
    temporary = SUMMARY.with_name(SUMMARY.name + '.writing')
    temporary.write_text(json.dumps(state))
    temporary.replace(SUMMARY)


child = subprocess.Popen([str(ROOT / 'target/alloyos-voice-helper')], stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)


def events():
    for raw in child.stdout:
        try:
            event = json.loads(raw)
            with lock:
                op = event.get('event')
                if op == 'audio_level':
                    state['level_max'] = max(state['level_max'], event['level'])
                elif op == 'audio_stats':
                    state['captured_samples'] = event['captured_samples']
                    state['sent_frames'] = event['sent_frames']
                    state['emitted_frames'] = event['emitted_frames']
                    state['captured_frames'] = event['captured_frames']
                    state['conversion_errors'] = event['conversion_errors']
                    state['native_level_max'] = max(state['native_level_max'], event['level'])
                    state['muted'] = event['muted']
                    state['stats_direct_audio'] = event.get('direct_audio')
                elif op == 'playback_active':
                    state['playback_started' if event['active'] else 'playback_finished'] += 1
                elif op == 'error':
                    state['errors'] += 1
                    message = event.get('message', '').lower()
                    state['last_error_stage'] = 'transport_mode' if 'transport mismatch' in message else 'permission' if 'permission' in message else 'audio_device' if 'audio' in message else 'provider_or_transport'
                elif op == 'progress':
                    stage = event.get('stage', '')
                    if stage in ('permission_pending', 'permission_granted', 'audio_starting', 'audio_ready', 'socket_open', 'setup_sent'):
                        state['last_progress'] = stage
                elif op == 'stopped':
                    state['stopped'] = True
                elif op == 'message':
                    message = json.loads(event['json'])
                    if 'setupComplete' in message:
                        state['provider_ready'] = True
                    server = message.get('serverContent', {})
                    transcript = server.get('inputTranscription', {}).get('text', '').lower()
                    if transcript:
                        state['input_transcripts'] += 1
                        state['speech_clock'] |= 'clock' in transcript
                        state['speech_uptime'] |= 'uptime' in transcript or 'up time' in transcript
                    if server.get('outputTranscription', {}).get('text'):
                        state['output_transcripts'] += 1
                    for call in message.get('toolCall', {}).get('functionCalls', []):
                        state['mcp_discovery_called'] |= call.get('name') == 'mcp_list_tools'
                        state['clock_called'] |= call.get('name') == 'mcp_call_tool' and call.get('args', {}).get('tool') == 'clock.now'
                save()
        except (KeyError, TypeError, ValueError):
            pass
        sys.stdout.buffer.write(raw);sys.stdout.buffer.flush()


state['helper_pid'] = child.pid
thread = threading.Thread(target=events, daemon=True)
thread.start()
with lock:
    save()
for raw in sys.stdin.buffer:
    try:
        command = json.loads(raw)
        if command.get('op') == 'start':
            with lock:
                state['start_direct_audio'] = command.get('direct_audio', False) is True
                save()
        if command.get('op') == 'stop':
            with lock:
                state['helper_stop_command_received'] = True
                save()
        if command.get('op') == 'send':
            payload = json.loads(command['json'])
            for response in payload.get('toolResponse', {}).get('functionResponses', []):
                if 'uptime_ms' in json.dumps(response.get('response', {})):
                    with lock:
                        state['clock_result_returned'] = True
                        save()
    except (ValueError, TypeError):
        pass
    child.stdin.write(raw);child.stdin.flush()
child.stdin.close()
child.wait(timeout=3)
thread.join(timeout=1)
with lock:
    state['helper_exited'] = True
    save()
