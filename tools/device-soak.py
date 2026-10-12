#!/usr/bin/env python3
"""Timed ON-DEVICE synthetic producer/ring test. Never requests MIC_TEST or real capture."""
import argparse,json,re,time
from pathlib import Path
import serial
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--seconds',type=int,default=7200);p.add_argument('--overflow',action='store_true');args=p.parse_args()
if args.overflow:args.seconds=45
assert 30<=args.seconds<=14400
root=Path(__file__).resolve().parents[1]
s=serial.Serial(port=None,baudrate=115200,timeout=.2);s.dtr=False;s.rts=False;s.port='/dev/cu.usbmodem1101';s.open()
pending=b'';evidence=[];started=time.monotonic();sent=False;result=None
try:
 s.write(b'\n')
 while time.monotonic()-started<args.seconds+90:
  pending+=s.read(4096)
  if len(pending)>32768:pending=pending[-32768:]
  while b'\n' in pending:
   raw,pending=pending.split(b'\n',1);line=raw.decode(errors='replace').strip()
   if 'BADGE_MIC_IDLE' in line or 'SYNTHETIC_' in line or 'recorder operation failed' in line or 'Guru Meditation' in line:
    evidence.append(line);print(line,flush=True)
   if 'MIC_TEST_READY' in line and not sent:
    s.write(b'SYNTH_OVERFLOW_TEST\n' if args.overflow else f'SYNTH_RING_TEST {args.seconds}\n'.encode());sent=True
   if 'SYNTHETIC_ERROR' in line or 'recorder operation failed' in line or 'Guru Meditation' in line:raise RuntimeError('Synthetic device test failed; see filtered log')
   if args.overflow and 'SYNTHETIC_OVERFLOW_RESULT samples=640000 mic_used=false retained_all=true' in line:
    result={'samples':640000,'mic_used':False,'retained_all':True,'completed_at':time.time()}
    (root/'.tools/device-overflow-result.json').write_text(json.dumps(result,indent=2));print('PASS: buffer exhaustion stopped capture and retained all four buffers; no microphone.',flush=True);break
   match=re.search(r'SYNTHETIC_RING_RESULT seconds=(\d+) samples=(\d+) min_internal_heap=(\d+) mic_used=false cancelled=(\w+)',line)
   if match:
    result={'requested_seconds':args.seconds,'elapsed_seconds':int(match[1]),'samples':int(match[2]),'min_internal_heap':int(match[3]),'mic_used':False,'cancelled':match[4]=='true','completed_at':time.time()}
    assert not result['cancelled'] and result['elapsed_seconds']>=args.seconds
    assert abs(result['samples']/16000-result['elapsed_seconds'])<1,'Producer clock/count drift exceeded one second'
    (root/'.tools/device-soak-result.json').write_text(json.dumps(result,indent=2))
    (root/f'.tools/device-soak-{args.seconds}s-result.json').write_text(json.dumps(result,indent=2))
    print('PASS: timed hardware synthetic producer/ring stayed bounded; microphone was never used.',flush=True)
    break
  if result:break
 if not result:raise TimeoutError('No completed hardware synthetic result')
finally:
 s.close();(root/'.tools/device-soak-events.log').write_text('\n'.join(evidence))
