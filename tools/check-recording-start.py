#!/usr/bin/env python3
"""Verify offline start rejection uses the real recorder path and Not started UI.
Uses ONLY SYNTH_CLOUD (generated silence), never requests real microphone capture.
Run with: uv run --with pyserial python tools/check-recording-start.py
"""
import serial,time
from pathlib import Path
s=serial.Serial(port=None,baudrate=115200,timeout=.2);s.dtr=False;s.rts=False;s.port='/dev/cu.usbmodem1101';s.open()
pending=b'';events=[]
def wait(markers,seconds=20):
 global pending
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  pending+=s.read(4096)
  while b'\n' in pending:
   raw,pending=pending.split(b'\n',1);line=raw.decode(errors='replace').strip()
   if any(m in line for m in ['CLOUD_CHECK','RECORDER_','recorder operation failed','BADGE_MIC_IDLE']):events.append(line)
   if any(m in line for m in markers):return line
 raise AssertionError('Timed out waiting for '+repr(markers))
try:
 s.write(b'\n');wait(['MIC_TEST_READY'])
 s.write(b'CLOUD_CHECK\n');state=wait(['CLOUD_CHECK_ERROR','CLOUD_CONNECTION_READY'])
 assert 'No Wi-Fi' in state or 'waiting for clock sync' in state,'Offline fixture unavailable; no capture requested'
 s.write(b'SYNTH_CLOUD 30\n');wait(['RECORDER_PHASE 10'])
 assert any('recorder operation failed' in x and ('No Wi-Fi' in x or 'clock sync' in x) for x in events)
 print('PASS: offline preflight rejected before capture; specific cause and Not started display reached.')
finally:
 s.close();Path(__file__).resolve().parents[1].joinpath('.tools/recording-start-after.log').write_text('\n'.join(events));print('\n'.join(events))
