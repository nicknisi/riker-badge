#!/usr/bin/env python3
"""Provision ONLY the dedicated device credential over local USB. Never prints it.
uv run --with pyserial python tools/configure-cloud.py
"""
from pathlib import Path
import json,time
import serial
root=Path(__file__).resolve().parents[1]
path=root/'.tools/transcribe-device-credentials.json'
assert path.stat().st_mode & 0o077 == 0,'Device credential file must be private'
creds=json.loads(path.read_text())
s=serial.Serial(port=None,baudrate=115200,timeout=.2);s.dtr=False;s.rts=False;s.port='/dev/cu.usbmodem1101';s.open()
pending=b''
def wait(marker):
 global pending
 end=time.monotonic()+20
 while time.monotonic()<end:
  pending+=s.read(2048)
  while b'\n' in pending:
   raw,pending=pending.split(b'\n',1);line=raw.decode(errors='replace').strip()
   if 'CLOUD_CONFIG_ERROR' in line:raise RuntimeError('Badge rejected cloud configuration')
   if marker in line:return
 raise TimeoutError('Badge did not reach '+marker)
try:
 s.write(b'\n');wait('MIC_TEST_READY')
 s.write(b'CLOUD_CONFIG\n');wait('CLOUD_CLIENT_ID>')
 s.write(creds['client_id'].encode()+b'\n');wait('CLOUD_CLIENT_SECRET>')
 s.write(creds['client_secret'].encode()+b'\n');wait('CLOUD_CONFIG_SAVED')
 print('Dedicated upload credential saved on badge. Admin setup token was NOT sent. Microphone was not started.')
finally:s.close()
