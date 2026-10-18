#!/usr/bin/env python3
"""Hermetic OS-owned OAuth integration over real guest UART ports.

Copies a supplied disk and boots three times. No browser, DNS, provider, model,
real account or network calls: the host supplies synthetic protocol responses.
Production guest crypto/core/private store/transport run without substitutes.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import subprocess
import tempfile
import time
from urllib.parse import parse_qs, urlencode, urlsplit

RESOURCE='https://resource.example.com/mcp'
ISSUER='https://issuer.example.com'


def boot(kernel,disk,mode,folder,output):
    config=folder/'mode'
    config.write_text(mode)
    command=['qemu-system-aarch64','-M','virt,gic-version=3','-accel','hvf','-cpu','host',
             '-kernel',str(kernel.resolve()),'-m','512M','-display','none','-serial','stdio','-no-reboot',
             '-device','virtio-rng-device','-drive',f'file={disk},if=none,format=raw,id=fixture',
             '-device','virtio-blk-device,drive=fixture','-fw_cfg',f'name=opt/alloyos/oauth-probe,file={config}']
    child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    selector=selectors.DefaultSelector();selector.register(child.stdout,selectors.EVENT_READ)
    pending=bytearray();logs=[];state=None;challenge=None;counts={}

    def reply(prefix,identifier,value):
        child.stdin.write(f'{prefix}\t{identifier}\tok\t{json.dumps(value).encode().hex()}\n'.encode());child.stdin.flush()

    try:
        deadline=time.monotonic()+90
        while time.monotonic()<deadline:
            if child.poll() is not None:
                raise RuntimeError('guest exited: '+child.stderr.read().decode(errors='replace'))
            if not selector.select(.2):
                continue
            pending.extend(os.read(child.stdout.fileno(),65536))
            if len(pending)>4*1024*1024:
                raise RuntimeError('excess UART frame')
            while b'\n' in pending:
                raw,_,pending=pending.partition(b'\n')
                line=raw.decode(errors='replace').rstrip('\r');fields=line.split('\t')
                if fields[0]=='BROWSER1':
                    assert mode=='connect' and len(fields)==3
                    request=json.loads(bytes.fromhex(fields[2]));operation=request.pop('operation');op=operation['op']
                    counts[op]=counts.get(op,0)+1
                    if op=='prepare':
                        assert operation['port']==39173
                        outcome={'status':'prepared','redirect_uri':'http://127.0.0.1:39173/callback'}
                    elif op=='open':
                        url=urlsplit(operation['authorization_url']);query=parse_qs(url.query,strict_parsing=True)
                        assert url.scheme=='https' and url.netloc=='issuer.example.com' and url.path=='/authorize'
                        assert all(len(v)==1 for v in query.values())
                        assert query['resource']==[RESOURCE] and query['client_id']==['fixture-client']
                        assert query['code_challenge_method']==['S256'] and query['scope']==['read']
                        state=query['state'][0];challenge=query['code_challenge'][0]
                        assert len(state)>=32 and len(challenge)==43
                        outcome={'status':'opened'}
                    elif op=='poll':
                        assert state
                        outcome={'status':'callback','query':urlencode({'code':'fixture-code','state':state,'iss':ISSUER})}
                    elif op=='cancel':
                        outcome={'status':'cancelled'}
                    else:
                        raise RuntimeError('unexpected browser operation')
                    reply('BROWSER1',fields[1],dict(request,outcome=outcome))
                elif fields[0]=='MCPHTTP1':
                    assert len(fields)==4 and fields[2] in ('authhttp','mcp')
                    request=json.loads(bytes.fromhex(fields[3]));status=200
                    if fields[2]=='authhttp':
                        headers={n.lower():v for n,v in request['headers']}
                        assert headers['x-alloyos-oauth']=='1' and 'authorization' not in headers
                        url=request['url'];counts[url]=counts.get(url,0)+1
                        if url=='https://resource.example.com/.well-known/oauth-protected-resource/mcp':
                            assert mode=='connect' and request['method']=='GET'
                            body={'resource':RESOURCE,'authorization_servers':[ISSUER]}
                        elif url==ISSUER+'/.well-known/oauth-authorization-server':
                            assert mode=='connect'
                            body={'issuer':ISSUER,'authorization_endpoint':ISSUER+'/authorize','token_endpoint':ISSUER+'/token','revocation_endpoint':ISSUER+'/revoke','code_challenge_methods_supported':['S256'],'authorization_response_iss_parameter_supported':True}
                        elif url==ISSUER+'/token':
                            form=parse_qs(request['body'],strict_parsing=True);assert all(len(v)==1 for v in form.values())
                            assert form['resource']==[RESOURCE] and form['client_id']==['fixture-client']
                            if mode=='connect':
                                assert form['grant_type']==['authorization_code'] and form['code']==['fixture-code']
                                computed=base64.urlsafe_b64encode(hashlib.sha256(form['code_verifier'][0].encode()).digest()).rstrip(b'=').decode()
                                assert computed==challenge,'guest PKCE does not match browser challenge'
                                token='fixture-access'
                            else:
                                assert mode=='restore' and form['grant_type']==['refresh_token'] and form['refresh_token']==['fixture-refresh']
                                token='fixture-refreshed'
                            body={'access_token':token,'refresh_token':'fixture-refresh','token_type':'Bearer','scope':'read','expires_in':3600}
                        elif url==ISSUER+'/revoke':
                            assert mode=='restore';status=503;body={'error':'fixture_provider_unavailable'}
                        else:
                            raise RuntimeError('unexpected OAuth destination')
                    else:
                        assert mode in ('connect','restore') and request['url']==RESOURCE
                        headers={n.lower():v for n,v in request['headers']}
                        assert headers['x-alloyos-trusted-bearer']=='fixture-access'
                        assert 'authorization' not in headers
                        rpc=json.loads(request['body']);method=rpc['method'];counts[method]=counts.get(method,0)+1
                        assert rpc['params']['_meta']['io.modelcontextprotocol/protocolVersion']=='2026-07-28'
                        if method=='tools/list':
                            result={'tools':[{'name':'read','description':'Synthetic read','inputSchema':{'type':'object','properties':{}}}]}
                        elif method=='tools/call':
                            assert rpc['params']['name']=='read';result={'content':[{'type':'text','text':'synthetic account data'}]}
                        else:
                            raise RuntimeError('unexpected MCP method')
                        body={'jsonrpc':'2.0','id':rpc['id'],'result':result}
                    reply('MCPHTTP1',fields[1],{'status':status,'headers':[['Content-Type','application/json']],'body':json.dumps(body)})
                elif fields[0] in ('ACCOUNT1','FORGEHTTP1'):
                    raise RuntimeError('obsolete broker or model access attempted')
                else:
                    logs.append(line)
                    if 'OAUTH-GUEST: FAIL' in line:
                        raise RuntimeError(line)
                    if 'OAUTH-GUEST: integration PASS' in line:
                        if mode!='revoked':
                            assert counts.get('tools/list')==1 and counts.get('tools/call')==1
                        else:
                            assert not counts
                        output.joinpath(mode+'-counts.json').write_text(json.dumps(counts,indent=2)+'\n')
                        print(f'PASS: real guest OS OAuth {mode} phase (synthetic provider)',flush=True)
                        return
        raise RuntimeError('OAuth guest fixture timed out')
    finally:
        output.joinpath(mode+'.log').write_text('\n'.join(logs)+'\n')
        selector.close();child.kill();child.wait()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel',type=Path,required=True);parser.add_argument('--disk',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True);args=parser.parse_args()
    args.output.mkdir(parents=True,exist_ok=True)
    args.output.joinpath('artifact.json').write_text(json.dumps({'kernel':str(args.kernel.resolve()),'sha256':hashlib.sha256(args.kernel.read_bytes()).hexdigest()},indent=2)+'\n')
    with tempfile.TemporaryDirectory(prefix='alloyos-oauth-fixture-',dir='/tmp') as temporary:
        folder=Path(temporary);disk=folder/'fixture.img';shutil.copy2(args.disk,disk)
        for mode in ('connect','restore','revoked'):
            boot(args.kernel,disk,mode,folder,args.output)
    print('PASS: OS private persistence, PKCE, refresh, revocation-failure isolation; no real account/network',flush=True)


if __name__=='__main__':
    main()
