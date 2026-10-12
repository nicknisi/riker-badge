#!/usr/bin/env python3
"""On-device AP/form/security smoke test; no microphone or credential changes.
uv run --with pyserial python tools/check-wifi.py --port PORT
"""
import argparse
from pathlib import Path
import re
import subprocess
import time
import serial

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--port',default='/dev/cu.usbmodem1101')
parser.add_argument('--require-station',action='store_true',help='Also require a live STA network for the LAN rejection check')
parser.add_argument('--no-reset',action='store_true',help='Use immediately after flashing; avoid another USB reset')
parser.add_argument('--settle-seconds',type=int,default=0,help='Wait after boot before opening setup, to catch later startup failures')
parser.add_argument('--via-display',action='store_true',help='Run setup on the display task, as the Settings selection does')
parser.add_argument('--start-failure',action='store_true',help='Prove setup/exit survive failed optional-service startup')
args=parser.parse_args()
if not args.no_reset:
    subprocess.run(['uvx','--from','esptool==5.4.0','esptool','--chip','esp32s3','--port',args.port,
                    '--after','hard-reset','chip-id'],check=True,stdout=subprocess.DEVNULL)
port=serial.Serial(port=None,baudrate=115200,timeout=.1)
port.dtr=False;port.rts=False;port.port=args.port;port.open()
pending=b''; evidence=[]

def wait(marker,seconds):
    global pending
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        pending+=port.read(4096)
        if len(pending)>32768: pending=pending[-32768:]
        while b'\n' in pending:
            raw,pending=pending.split(b'\n',1)
            line=re.sub(r'\x1b\[[0-9;]*m','',raw.decode(errors='replace')).strip()
            if any(s in line for s in ['WIFI_', 'BADGE_MIC_IDLE', 'BADGE_RENDER_READY', 'MIC_TEST_READY', 'Guru Meditation', 'panicked at', 'Wi-Fi manager:','could not start','startup stopped safely','RIKER_START_TEST_FAILURE']):
                line=re.sub(r'https?://\S+','<URL omitted>',line)
                evidence.append(line)
            if 'WIFI_CHECK_ERROR' in line or 'Wi-Fi setup could not start' in line: raise AssertionError(line)
            if any(m in line for m in ([marker] if isinstance(marker,str) else marker)):return line
    raise AssertionError('Timed out waiting for '+str(marker))

def close_setup():
    port.write(b'WIFI_UI_CLOSE\n' if args.via_display else b'WIFI_CLOSE\n')
    wait('WIFI_AP_CLOSED',15)

try:
    if args.no_reset: port.write(b'WIFI_CLOSE\n')
    wait('MIC_TEST_READY',20)
    if args.require_station: wait('WIFI_READY',45)
    if args.settle_seconds:
        try: wait('WIFI_UNEXPECTED_SETTLE_SENTINEL',args.settle_seconds)
        except AssertionError as error:
            if not str(error).startswith('Timed out waiting'):raise
    port.write(b'WIFI_UI_SETUP\n' if args.via_display else b'WIFI_SETUP\n')
    wait('WIFI_AP_READY',15)
    if not any('WIFI_DISPLAY phase=2 qr=1' in line for line in evidence):
        wait('WIFI_DISPLAY phase=2 qr=1',5)
    if args.start_failure:
        port.write(b'WIFI_START_FAILURE_TEST\n')
        wait('RIKER_START_TEST_FAILURE:',5)
        close_setup()
        port.write(b'WIFI_UI_SETUP\n')
        wait('WIFI_AP_READY',15)
        wait('WIFI_DISPLAY phase=2 qr=1',5)
    if not args.require_station:
        try: wait('WIFI_READY',8)
        except AssertionError: pass  # Offline setup is a required supported path.
    port.write(b'WIFI_CHECK\n')
    outcome=wait(('WIFI_CHECK_PASS','WIFI_CHECK_PARTIAL'),20)
    if args.require_station: assert 'WIFI_CHECK_PASS' in outcome
    close_setup()
    if args.start_failure:
        port.write(b'WIFI_UI_SETUP\n')
        wait('WIFI_AP_READY',15)
        wait('WIFI_DISPLAY phase=2 qr=1',5)
        close_setup()
    print('PASS: secured AP started, QR drawn/pushed to display, AP form served, invalid CSRF rejected, AP shut down.')
    print('STA/forged-Host rejection: '+('passed' if 'WIFI_CHECK_PASS' in outcome else 'not exercised; no live STA network'))
finally:
    # Best-effort close even if a probe fails. The independent 10-minute expiry remains.
    try:port.write(b'WIFI_CLOSE\n');time.sleep(.3)
    except serial.SerialException:pass
    port.close()
    path=Path(__file__).resolve().parents[1]/'.tools/wifi-check.log'
    path.write_text('\n'.join(evidence))
    print('\n'.join(evidence[-35:]))
