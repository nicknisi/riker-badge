#!/usr/bin/env python3
"""uv run --python 3.12 --with pillow --with zxing-cpp python tools/test_badge_qr.py
Round-trip the firmware's URL through its real M5GFX encoder and an independent decoder.
"""
from io import BytesIO
from pathlib import Path
import math
import re
import subprocess
import tempfile
from PIL import Image
import zxingcpp

root = Path(__file__).resolve().parents[1]
source = (root / "components/badge_display/badge_display.cpp").read_text()
url = re.search(r'LINKEDIN_URL\[\] = "([^"]+)"', source)[1]
size = int(re.search(r'QR_SIZE = (\d+)', source)[1])
version = int(re.search(r'QR_VERSION = (\d+)', source)[1])
assert url == "https://linkedin.com/in/nicknisi"
assert size * math.sqrt(2) < 466, "QR quiet-zone square must fit inside the visible round screen"
encoder = root / "components/M5GFX/src/lgfx/utility"
with tempfile.TemporaryDirectory() as temp:
    temp = Path(temp)
    (temp / "qr.c").write_text(r'''
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include "lgfx_qrcode.h"
int main(int argc, char** argv) {
    assert(argc == 4);
    int width = atoi(argv[2]), version = atoi(argv[3]);
    uint8_t* data = calloc(1, lgfx_qrcode_getBufferSize(version));
    assert(data);
    QRCode qr;
    assert(lgfx_qrcode_initText(&qr, data, version, ECC_LOW, argv[1]) == 0);
    // Same integer module/quiet-zone geometry as LGFXBase::qrcode(margin=true).
    int scale = width / qr.size;
    int offset = (width - qr.size * scale) / 2;
    if (offset < 4 * scale) {
        scale = (width - 8 * scale) / qr.size;
        offset = (width - qr.size * scale) / 2;
    }
    assert(scale > 0 && offset >= 4 * scale);
    printf("P1\n%d %d\n", width, width);
    for (int y = 0; y < width; ++y) {
        for (int x = 0; x < width; ++x) {
            int inside = x >= offset && y >= offset && x < offset + qr.size * scale && y < offset + qr.size * scale;
            int black = inside && lgfx_qrcode_getModule(&qr, (x-offset)/scale, (y-offset)/scale);
            printf("%d ", black);
        }
        printf("\n");
    }
    free(data);
}
''')
    subprocess.run(["cc", "-std=c99", "-I", str(encoder), str(temp / "qr.c"),
                    str(encoder / "lgfx_qrcode.c"), "-o", str(temp / "qr")], check=True)
    wifi_size = int(re.search(r'WIFI_QR_SIZE = (\d+)', source)[1])
    wifi_version = int(re.search(r'WIFI_QR_VERSION = (\d+)', source)[1])
    for x in [93,93+wifi_size]:
        for y in [105,105+wifi_size]:
            assert math.hypot(x-233,y-233)<233
    cases = [(url,size,version,"linkedin-qr.png"),
        ("WIFI:T:WPA;S:Nick-Badge-abcdef;P:0123456789abcdef01234567;;",wifi_size,wifi_version,"wifi-qr-fixture.png"),
        ("http://192.168.4.1/",wifi_size,2,"wifi-page-qr.png")]
    for payload,width,v,filename in cases:
        bitmap = subprocess.check_output([str(temp / "qr"), payload, str(width), str(v)])
        image = Image.open(BytesIO(bitmap)).convert("RGB")
        decoded = zxingcpp.read_barcode(image)
        assert decoded is not None and decoded.text == payload, "QR round-trip failed"
        (root / ".tools").mkdir(exist_ok=True)
        image.save(root / ".tools" / filename)
print("PASS: native LinkedIn, setup-network and local-form QRs round-trip; quiet zones and screen bounds verified")
