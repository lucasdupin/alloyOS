#!/usr/bin/env python3
"""Exercise the actual UI/Welder MCP search loop in a fresh QEMU VM.

Default: deterministic model responses plus live public MCP Registry HTTPS.
--live-model also uses the configured Gemini credential; never prints credentials,
request prompts, response text, or raw UART HTTP frames.
"""
import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import time
import urllib.request

FRAME_LIMIT = 4 * 1024 * 1024 + 16384
REGISTRY_PREFIX = 'https://registry.modelcontextprotocol.io/v0.1/servers?'
GEMINI_URL = 'https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent'


def registry_fetch(url):
    result = subprocess.run(['curl', '-q', '--proto', '=https', '--connect-timeout', '5',
                             '--max-time', '10', '--retry', '1', '--retry-delay', '1',
                             '--retry-all-errors', '--max-filesize', '1048576', '--silent',
                             '--write-out', '\n%{http_code}', url],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=25)
    if result.returncode:
        raise OSError('registry HTTPS failed')
    body, status = result.stdout.rsplit(b'\n', 1)
    return int(status), body


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel', type=Path, required=True)
    parser.add_argument('--live-model', action='store_true')
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    events = []

    def event(stage, **fields):
        record = {'stage': stage, **fields}
        events.append(record)
        print(json.dumps(record), flush=True)
        args.log.write_text('\n'.join(json.dumps(item) for item in events) + '\n')
        args.log.chmod(0o600)

    with tempfile.TemporaryDirectory(prefix='alloyos-mcp-chat-') as folder:
        key = 'fixture-key'
        if args.live_model:
            key = (Path(__file__).resolve().parents[1] / '.secrets/GEMINI_API_KEY').read_text().strip()
            if key.startswith('GEMINI_API_KEY='):
                key = key.split('=', 1)[1]
        auth = Path(folder) / 'auth'
        auth.write_bytes(('GEMINI_API_KEY=' + key + '\0').encode())
        auth.chmod(0o600)
        command = ['qemu-system-aarch64', '-M', 'virt', '-accel', 'hvf', '-cpu', 'host',
                   '-kernel', str(args.kernel), '-m', '256M', '-display', 'none',
                   '-monitor', 'none', '-serial', 'stdio', '-no-reboot',
                   '-device', 'virtio-gpu-device,xres=1206,yres=2622',
                   '-fw_cfg', f'name=opt/alloyos/auth,file={auth}']
        environment = {k: v for k, v in os.environ.items() if k not in ('GEMINI_API_KEY', 'OPENAI_API_KEY', 'ANTHROPIC_API_KEY')}
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, env=environment)
        selector = selectors.DefaultSelector()
        selector.register(process.stdout, selectors.EVENT_READ)
        pending = bytearray()
        model_calls = 0
        registry_calls = 0
        current_model_start = 0

        def send(line):
            process.stdin.write(line.encode())
            process.stdin.flush()

        def cloud_response(request):
            nonlocal model_calls
            assert request['url'] == GEMINI_URL, 'unexpected cloud destination'
            body = json.loads(request['body'])
            max_tokens = body['generationConfig']['maxOutputTokens']
            if model_calls - current_model_start >= 8:
                raise RuntimeError("approved live-model call budget exhausted")
            model_calls += 1
            event('model_request', max_output_tokens=max_tokens, live=args.live_model)
            if args.live_model:
                req = urllib.request.Request(request['url'], data=request['body'].encode(),
                                             headers=dict(request['headers']), method='POST')
                for attempt in range(2):
                    try:
                        with urllib.request.urlopen(req, timeout=45) as response:
                            result = json.loads(response.read(65537))
                        break
                    except urllib.error.HTTPError as error:
                        event('model_http_error', status=error.code)
                        if error.code not in (429, 500, 502, 503, 504) or attempt == 1:
                            return {'status': error.code, 'body': error.read(65537).decode()}
                        if model_calls - current_model_start >= 8:
                            raise RuntimeError('approved live-model call budget exhausted')
                        model_calls += 1
                        time.sleep(1)
            elif max_tokens < 1024:
                result = {'candidates': [{'finishReason': 'MAX_TOKENS', 'content': {'parts': []}}], 'usageMetadata': {'thoughtsTokenCount': max_tokens}}
            else:
                prompt = body['contents'][0]['parts'][0]['text']
                current_turn = prompt[prompt.rfind('\nuser: '):]
                tool_results = []
                for line in current_turn.splitlines():
                    if line.startswith('tool: '):
                        tool_results.append(json.loads(line[6:]))
                results = [item for item in tool_results if item.get('source') == 'official_mcp_registry' and item.get('ok')]
                if results:
                    assert results[-1]['count'] > 0, 'registry returned no Gmail services'
                    text = json.dumps({'kind': 'final', 'text': 'Gmail MCP service found: ' + results[-1]['servers'][0]['name']})
                else:
                    text = json.dumps({'kind': 'tools', 'calls': [{'name': 'mcp_search_services', 'arguments': {'query': 'gmail'}}]})
                result = {'candidates': [{'finishReason': 'STOP', 'content': {'parts': [{'text': text}]}}], 'usageMetadata': {'candidatesTokenCount': 40}}
            candidate = (result.get('candidates') or [{}])[0]
            text_bytes = sum(len(part.get('text', '').encode()) for part in candidate.get('content', {}).get('parts', []) if not part.get('thought'))
            reason = candidate.get('finishReason')
            event('model_response', finish=reason if reason in ('STOP', 'MAX_TOKENS', 'SAFETY') else 'other', text_bytes=text_bytes,
                  output_tokens=result.get('usageMetadata', {}).get('candidatesTokenCount'), thoughts=result.get('usageMetadata', {}).get('thoughtsTokenCount'))
            return {'status': 200, 'body': json.dumps(result)}

        def read_line(deadline):
            while b'\n' not in pending:
                if time.monotonic() >= deadline:
                    raise RuntimeError('QEMU phase timed out; inspect the stage log')
                if selector.select(1):
                    chunk = os.read(process.stdout.fileno(), 8192)
                    if not chunk:
                        raise RuntimeError('QEMU exited before completion')
                    pending.extend(chunk)
                    if len(pending) > FRAME_LIMIT:
                        raise RuntimeError('UART frame exceeded bounds')
            line, _, rest = pending.partition(b'\n')
            pending[:] = rest
            return line.decode(errors='replace').rstrip('\r')

        def response(identifier):
            nonlocal registry_calls
            deadline = time.monotonic() + 160
            while True:
                line = read_line(deadline)
                fields = line.split('\t')
                if fields[0] == 'FORGEHTTP1':
                    assert len(fields) == 4 and fields[2] == 'post'
                    envelope = cloud_response(json.loads(bytes.fromhex(fields[3])))
                    send(f'FORGEHTTP1\t{fields[1]}\tok\t{json.dumps(envelope).encode().hex()}\n')
                elif fields[0] == 'MCPHTTP1':
                    assert len(fields) == 4 and fields[2] == 'get'
                    request = json.loads(bytes.fromhex(fields[3]))
                    assert request['url'].startswith(REGISTRY_PREFIX), 'unexpected registry destination'
                    assert request['headers'] == [] and request['body'] == ''
                    registry_calls += 1
                    event('registry_request')
                    try:
                        status, body = registry_fetch(request['url'])
                        assert len(body) <= 1024 * 1024
                        data = json.loads(body)
                        event('registry_response', status=status, body_bytes=len(body), count=len(data.get('servers', [])))
                        envelope = {'status': status, 'headers': [], 'body': body.decode()}
                        send(f'MCPHTTP1\t{fields[1]}\tok\t{json.dumps(envelope).encode().hex()}\n')
                    except (OSError, subprocess.TimeoutExpired):
                        event('registry_transport_error')
                        send(f'MCPHTTP1\t{fields[1]}\terror\tregistry_https_failed\n')
                elif fields[:2] == ['FORGE1', str(identifier)]:
                    if fields[2] != 'ok':
                        detail = bytes.fromhex(fields[-1]).decode()
                        raise RuntimeError('chat failed: ' + detail)
                    return json.loads(bytes.fromhex(fields[3]))
                elif line.startswith(('[mcp-search]', '[welder-e2e]')):
                    event('guest_stage', message=line)

        try:
            deadline = time.monotonic() + 40
            while '[systemui] Slint window online;' not in read_line(deadline):
                pass
            event('boot_complete')
            send('FORGE1\t1\tstatus\n')
            initial = response(1)
            assert 'gemini-3.8-flash' in json.dumps(initial)
            send(f'FORGE1\t10\tchat\t{"/model gemini".encode().hex()}\n')
            selected = response(10)
            assert 'gemini' in selected['text'], 'Gemini selection failed'
            event('model_selected')
            for identifier in (2, 3):
                before = registry_calls
                current_model_start = model_calls
                query = ('Is Gmail MCP available?' if identifier == 2 else
                         'Search for Gmail MCP services again. Do a fresh lookup and give one exact service name returned by the tool.')
                send(f'FORGE1\t{identifier}\tchat\t{query.encode().hex()}\n')
                output = response(identifier)
                results = [tool['output'] for tool in output['tools'] if tool['name'] == 'mcp_search_services' and tool['output'].get('ok')]
                assert results and results[-1]['count'] > 0, 'no successful registry tool result'
                names = [server['name'] for server in results[-1]['servers']]
                assert any(name in output['text'] for name in names), 'final answer did not use registry results'
                assert registry_calls > before, 'fresh lookup was not executed'
                event('chat_pass', iteration=identifier - 1, services=len(names))
            send('FORGE1\t4\tstatus\n')
            final = response(4)
            assert final['ui_frames'] > initial['ui_frames'] + 10, 'UI frames did not progress'
            event('e2e_pass', model_calls=model_calls, registry_calls=registry_calls, ui_frames=final['ui_frames'] - initial['ui_frames'], live_model=args.live_model)
        finally:
            selector.close()
            process.kill()
            process.wait()
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
