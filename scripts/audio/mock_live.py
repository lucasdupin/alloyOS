#!/usr/bin/env python3
"""Local deterministic Live server for native media/QEMU acceptance tests.

Only stdlib. No credentials, microphone, external services, or generated audio files.
Counts decoded PCM bytes at the model boundary, independently of helper telemetry.
"""
import base64
import hashlib
import json
import socket
import struct
import threading
import time


class MockLive:
    def __init__(self, tools=False):
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen(1)
        self.listener.settimeout(.2)
        self.endpoint = f'ws://127.0.0.1:{self.listener.getsockname()[1]}'
        self.frames = self.samples = 0
        self.first_audio = self.last_audio = None
        self.messages = []
        self.responses = []
        self.error = None
        self.closed = False
        self.tools = tools
        self.finished = False
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    @staticmethod
    def _read(stream, length):
        data = bytearray()
        while len(data) < length:
            part = stream.read(length - len(data))
            if not part:
                raise EOFError()
            data.extend(part)
        return bytes(data)

    def _send(self, value):
        data = json.dumps(value).encode()
        header = bytes([0x81, len(data)]) if len(data) < 126 else b'\x81\x7e' + struct.pack('!H', len(data))
        self.connection.sendall(header + data)

    def _serve(self):
        try:
            while not self.closed:
                try:
                    self.connection, _ = self.listener.accept()
                    break
                except socket.timeout:
                    continue
            else:
                return
            self.connection.settimeout(30)
            stream = self.connection.makefile('rb')
            request = []
            while True:
                line = stream.readline()
                if line == b'\r\n':
                    break
                if not line:
                    raise EOFError()
                request.append(line.decode().strip())
            headers = dict(line.split(': ', 1) for line in request[1:] if ': ' in line)
            key = next(value for name, value in headers.items() if name.lower() == 'sec-websocket-key')
            accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
            self.connection.sendall(('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + '\r\n\r\n').encode())
            fragments = bytearray()
            while not self.closed:
                first, second = self._read(stream, 2)
                length = second & 127
                if length == 126:
                    length = struct.unpack('!H', self._read(stream, 2))[0]
                elif length == 127:
                    length = struct.unpack('!Q', self._read(stream, 8))[0]
                if length > 131072:
                    raise AssertionError('unbounded client frame')
                mask = self._read(stream, 4) if second & 128 else None
                data = self._read(stream, length)
                if mask:
                    data = bytes(value ^ mask[index % 4] for index, value in enumerate(data))
                opcode = first & 15
                if opcode == 8:
                    return
                if opcode == 9:
                    self.connection.sendall(bytes([0x8a, len(data)]) + data)
                    continue
                if opcode not in (0, 1, 2):
                    continue
                fragments.extend(data)
                if not first & 128:
                    continue
                message = json.loads(fragments)
                fragments.clear()
                # Keep control data only: an unbounded sample recorder would hide leaks.
                if 'setup' in message:
                    self.messages.append(message)
                    self._send({'setupComplete': {}})
                audio = message.get('realtimeInput', {}).get('audio')
                if audio:
                    assert audio['mimeType'] == 'audio/pcm;rate=16000'
                    pcm = base64.b64decode(audio['data'], validate=True)
                    assert len(pcm) % 2 == 0 and len(pcm) <= 8192
                    assert any(pcm), 'synthetic audio unexpectedly silent'
                    self.samples += len(pcm) // 2
                    self.frames += 1
                    self.last_audio = time.monotonic()
                    self.first_audio = self.first_audio or self.last_audio
                    if self.frames == 5:
                        self._send({'serverContent': {'inputTranscription': {'text': 'Synthetic microphone accepted.', 'finished': True}}})
                        if self.tools:
                            self._send({'toolCall': {'functionCalls': [{'id': 'verify-discovery', 'name': 'mcp_list_tools', 'args': {}}]}})
                        else:
                            self.complete()
                if 'toolResponse' in message:
                    self.responses.extend(message['toolResponse']['functionResponses'])
                    if self.tools and any(item.get('id') == 'verify-discovery' for item in message['toolResponse']['functionResponses']):
                        self._send({'toolCall': {'functionCalls': [{'id': 'verify-clock', 'name': 'mcp_call_tool', 'args': {'app': 'clock', 'tool': 'clock.now', 'args': '{}'}}]}})
                    else:
                        self.complete()
        except (EOFError, OSError) as error:
            if not self.closed and not isinstance(error, EOFError):
                self.error = repr(error)
        except Exception as error:
            self.error = repr(error)

    def complete(self):
        if self.finished:
            return
        self.finished = True
        self._send({'serverContent': {'modelTurn': {'parts': [{'inlineData': {'mimeType': 'audio/pcm;rate=24000', 'data': base64.b64encode(b'\x00\x20' * 1200).decode()}}]}, 'outputTranscription': {'text': 'Native audio transport verified.'}, 'turnComplete': True}})

    def close(self):
        self.closed = True
        self.listener.close()
        if hasattr(self, 'connection'):
            self.connection.close()
