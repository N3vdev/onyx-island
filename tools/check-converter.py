"""Authored local converter checks. Never reads or logs user documents."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import uuid
import zlib

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--helper', type=Path, default=ROOT / 'target/release/converter/Onyx.Converter.exe')
parser.add_argument('--report', type=Path, default=ROOT / 'target/converter-check-results.json')
args = parser.parse_args()
HELPER = args.helper.resolve()
WORK = ROOT / "target" / ("converter-check-" + uuid.uuid4().hex[:10])
WORK.mkdir()


def request(command, files, operation=None, cancel=False):
    result = subprocess.run([str(HELPER)], input=json.dumps({"command": command,
        "files": [str(path) for path in files], "operation": operation}) + "\n" + ("cancel\n" if cancel else ""),
        capture_output=True, text=True, timeout=90)
    events = [json.loads(line) for line in result.stdout.splitlines() if line]
    assert events, "Converter returned no events"
    return events


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


image = WORK / "Sample picture.png"
pixels = b"".join(b"\0" + bytes(component for x in range(32) for component in (x * 7, y * 9, 120)) for y in range(24))
image.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 32, 24, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b""))
pdf = WORK / "Two page sample.pdf"
streams = [b"BT /F1 18 Tf 30 130 Td (Onyx authored fixture: page one) Tj ET",
           b"BT /F1 18 Tf 30 130 Td (Onyx authored fixture: page two) Tj ET"]
objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 7 0 R >> >> /Contents 4 0 R >>",
    b"<< /Length " + str(len(streams[0])).encode() + b" >>\nstream\n" + streams[0] + b"\nendstream",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >>",
    b"<< /Length " + str(len(streams[1])).encode() + b" >>\nstream\n" + streams[1] + b"\nendstream",
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
body = bytearray(b"%PDF-1.4\n")
offsets = [0]
for i, obj in enumerate(objects, 1):
    offsets.append(len(body))
    body.extend(str(i).encode() + b" 0 obj\n" + obj + b"\nendobj\n")
xref = len(body)
body.extend(b"xref\n0 8\n0000000000 65535 f \n")
for offset in offsets[1:]: body.extend(f"{offset:010d} 00000 n \n".encode())
body.extend(f"trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
pdf.write_bytes(body)
originals = {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in [image, pdf]}
checks = []


def convert(files, operation):
    events = request("convert", files, operation)
    final = next(event for event in events if event["type"] == "complete")
    assert final["succeeded"] == len(files) and final["failed"] == 0
    outputs = [Path(event["output"]) for event in events if event["type"] == "outcome" and event.get("output")]
    assert all(path.exists() for path in outputs)
    return outputs


catalog = request("catalog", [pdf])[0]
ids = [op["id"] for op in catalog["operations"]]
assert "convert.pdf.png" in ids and "convert.pdf.jpg" in ids
checks.append("pdf-catalog")
for fmt, signature in [("png", b"\x89PNG"), ("jpg", b"\xff\xd8")]:
    result = convert([pdf], "convert.pdf." + fmt)[0]
    pages = list(result.glob("*." + fmt))
    assert len(pages) == 2 and all(page.read_bytes().startswith(signature) for page in pages)
    checks.append("two-page-pdf-to-" + fmt)
first_pngs = sorted(WORK.glob("Two page sample (PNG)*"))
second = convert([pdf], "convert.pdf.png")[0]
assert second not in first_pngs and all(path.exists() for path in first_pngs)
checks.append("collision-safe-folder")
catalog = request("catalog", [image])[0]
for op in catalog["operations"]:
    result = convert([image], op["id"])[0]
    assert result.stat().st_size > 0
    checks.append("image-to-" + op["label"].lower())
before = convert([image], "convert.image.jpg")[0]
before_hash = hashlib.sha256(before.read_bytes()).hexdigest()
after = convert([image], "convert.image.jpg")[0]
assert before != after and hashlib.sha256(before.read_bytes()).hexdigest() == before_hash
checks.append("collision-safe-file")
unknown = WORK / "Unsupported.txt"
unknown.write_text("Authored converter fixture.", encoding="utf-8")
assert request("catalog", [unknown])[0]["operations"] == []
assert request("catalog", [pdf, image])[0]["operations"] == []
checks.append("unsupported-and-mixed-selection")
broken = WORK / "Broken.pdf"
broken.write_bytes(b"This is an authored invalid-PDF fixture.")
events = request("convert", [broken], "convert.pdf.png")
assert any(event["type"] == "complete" and event["failed"] == 1 for event in events)
checks.append("damaged-pdf-error")
events = request("convert", [pdf], "not-an-operation")
assert any(event["type"] == "error" for event in events)
checks.append("operation-validation")
events = request("convert", [pdf], "convert.image.jpg")
assert any(event["type"] == "error" for event in events)
checks.append("cross-type-operation-rejected")
events = request("convert", [pdf], "convert.pdf.png", cancel=True)
assert any(event["type"] == "complete" and event["canceled"] for event in events)
checks.append("cancel-keeps-originals")
for path, expected in originals.items(): assert hashlib.sha256(path.read_bytes()).hexdigest() == expected
checks.append("originals-unchanged")
svg = WORK / "Sample mark.svg"
svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60"><rect width="80" height="60" fill="#202431"/><circle cx="40" cy="30" r="18" fill="#a4ccb4"/></svg>',encoding="utf-8")
catalog=request("catalog",[svg])[0]
assert len(catalog["operations"])==8
converted=convert([svg],"convert.image.png")[0]
assert converted.read_bytes().startswith(b"\x89PNG")
checks.append("svg-to-png-eight-format-catalog")
import math, wave
audio=WORK/"Sample tone.wav"
with wave.open(str(audio),'wb') as wav:
    wav.setparams((1,2,16000,0,'NONE','not compressed'))
    wav.writeframes(b''.join(struct.pack('<h',int(3000*math.sin(2*math.pi*440*i/16000))) for i in range(16000)))
catalog=request("catalog",[audio])[0]
audio_ids={op["id"] for op in catalog["operations"]}
for extension in ['mp3','flac']:
    operation=next(op for op in audio_ids if op.endswith('.'+extension))
    result=convert([audio],operation)[0]
    assert result.stat().st_size>128
    checks.append('audio-wav-to-'+extension)
video = WORK / 'Sample pattern.mp4'
shutil.copyfile(ROOT / 'OrbitDrop-main/tests/OrbitDrop.Tests/Fixtures/testpattern.mp4', video)
video_hash = hashlib.sha256(video.read_bytes()).hexdigest()
catalog = request('catalog', [video])[0]
for extension in ['wmv', 'gif']:
    operation = next(op['id'] for op in catalog['operations'] if op['label'].lower() == extension)
    result = convert([video], operation)[0]
    assert result.stat().st_size > 128
    checks.append('video-to-' + extension)
assert hashlib.sha256(video.read_bytes()).hexdigest() == video_hash
report = {"passed": checks, "count": len(checks), "fixture_directory": str(WORK), "helper": str(HELPER)}
args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
print(json.dumps(report, indent=2))
