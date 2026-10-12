#!/usr/bin/env python3
"""Real-time HTTPS/storage/processing soak with generated audio, never the microphone.
Uses a bounded six-chunk simulated sender and a deliberate 35s upload outage.
"""
from collections import deque
import argparse
import hashlib
import json
from pathlib import Path
import time
import urllib.request
import urllib.error
import uuid
import wave

ROOT=Path(__file__).resolve().parents[1]
RATE=16000
SAMPLES=160000
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):return None


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--hours',type=float,default=4)
    args=p.parse_args()
    assert 0<args.hours<=4
    creds=json.loads((ROOT/'.tools/transcribe-device-credentials.json').read_text())
    headers={'CF-Access-Client-Id':creds['client_id'],'CF-Access-Client-Secret':creds['client_secret'],'User-Agent':'riker-synthetic-soak/0.1'}
    client=urllib.request.build_opener(NoRedirect)
    def call(path,method='GET',body=None,extra=None):
        h={**headers,**(extra or {})}
        if isinstance(body,dict):body=json.dumps(body).encode();h['Content-Type']='application/json'
        request=urllib.request.Request('https://transcribe.nisi.sh'+path,method=method,data=body,headers=h)
        with client.open(request,timeout=20) as r:return json.load(r)
    with wave.open(str(ROOT/'.tools/transcribe-smoke.wav'),'rb') as w:
        assert (w.getframerate(),w.getsampwidth(),w.getnchannels())==(RATE,2,1)
        speech=w.readframes(w.getnframes())
    id=str(uuid.uuid4());call('/api/device/recordings','POST',{'id':id,'title':f'Synthetic {args.hours:g}-hour soak — not a real meeting'})
    total_samples=int(args.hours*3600*RATE);total_chunks=(total_samples+SAMPLES-1)//SAMPLES
    queue=deque();captured=0;saved=0;errors=0;high_water=0;started=time.monotonic();next_upload=0;interrupted=False
    logpath=ROOT/'.tools/cloud-soak-progress.json'
    def chunk(seq):
        count=min(SAMPLES,total_samples-seq*SAMPLES);pcm=bytearray(count*2)
        # Put synthetic speech across a transport boundary every five minutes.
        start=seq*SAMPLES;end=start+count;period=300*RATE
        for cycle in range(max(0,start//period-1),end//period+1):
            voice_start=cycle*period+3*RATE;voice_end=voice_start+len(speech)//2
            left=max(start,voice_start);right=min(end,voice_end)
            if left<right:pcm[(left-start)*2:(right-start)*2]=speech[(left-voice_start)*2:(right-voice_start)*2]
        return bytes(pcm)
    def progress(stage):
        result={'id':id,'stage':stage,'started_at':time.time()-(time.monotonic()-started),'elapsed_seconds':round(time.monotonic()-started,1),
            'captured_chunks':captured,'saved_chunks':saved,'expected_chunks':total_chunks,'buffered_chunks':len(queue),'buffer_high_water':high_water,'http_errors':errors,'microphone_used':False}
        temp=logpath.with_suffix('.tmp');temp.write_text(json.dumps(result,indent=2));temp.replace(logpath)
        return result
    print('Synthetic soak recording:',id,flush=True)
    while captured<total_chunks or queue:
        now=time.monotonic();elapsed=now-started
        while captured<total_chunks and elapsed>=(captured+1)*10:
            if len(queue)>=6:interrupted=True;break
            data=chunk(captured);queue.append((captured,data,hashlib.sha256(data).hexdigest()));captured+=1;high_water=max(high_water,len(queue))
        if interrupted:break
        outage=300<=elapsed<335
        if queue and not outage and now>=next_upload:
            seq,data,checksum=queue[0]
            try:
                answer=call(f'/api/device/recordings/{id}/chunks/{seq}','PUT',data,{'Content-Type':'application/octet-stream','X-Audio-Sha256':checksum})
                assert answer.get('saved') is True and answer.get('sha256')==checksum
                queue.popleft();saved+=1
                if saved%30==0:print('Saved',saved,'/',total_chunks,'chunks; simulated buffer high-water',high_water,flush=True)
            except (OSError,urllib.error.HTTPError,TimeoutError) as error:
                errors+=1;next_upload=time.monotonic()+3
                print('Upload retry pending; status:',getattr(error,'code',type(error).__name__),flush=True)
        progress('capturing')
        time.sleep(.2)
    if interrupted:
        result=progress('failed-buffer-exhaustion');print(json.dumps(result),flush=True)
        raise RuntimeError('Synthetic sender buffer exhausted; recording remains visibly incomplete')
    answer=call(f'/api/device/recordings/{id}/seal','POST',{'expected_chunks':total_chunks,'total_samples':total_samples,'reason':'stopped'})
    progress('processing');print('Sealed:',answer,flush=True)
    deadline=time.monotonic()+600
    while time.monotonic()<deadline:
        state=call(f'/api/device/recordings/{id}')
        if state['status'] in ('complete','partial','notes-failed'):
            result=progress(state['status']);result['processing_counts']=state['counts'];result['completion_at']=time.time()
            logpath.write_text(json.dumps(result,indent=2))
            assert state['status']=='complete',f"Processing ended as {state['status']}"
            assert sum(x['count'] for x in state['counts'] if x['status']=='done')==total_chunks
            print('PASS: full real-time synthetic soak saved and processed every chunk, including outage recovery.',flush=True)
            return
        time.sleep(10)
    raise TimeoutError('Soak audio saved, but processing did not converge within ten minutes')

if __name__=='__main__':main()
