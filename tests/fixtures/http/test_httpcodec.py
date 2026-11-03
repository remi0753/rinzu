#!/usr/bin/env python3
"""Compare the native scanner with the previous request-line rules, including ASAN."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[3]
TOKENS = b"!#$%&'*+-.^_`|~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"


def reference(data):
    first = data.find(b' ')
    if first <= 0 or any(byte not in TOKENS for byte in data[:first]):
        return -400
    second = data.find(b' ', first + 1)
    if second < 0:
        return -400
    target = data[first + 1:second]
    if len(target) > 4096:
        return -414
    if (not target or target[0] != 47 or any(byte < 33 or byte > 126 or byte == 35 for byte in target)
            or data[second + 1:] != b'HTTP/1.1'):
        return -400
    return first


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--workdir', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    args.workdir.mkdir(parents=True, exist_ok=True)
    cases = [b'', b' ', b'GET / HTTP/1.1', b'get /a?x=%00+z HTTP/1.1', b"!#$%&'*+-.^_`|~ / HTTP/1.1"]
    for seed in list(cases[2:]):
        cases.extend(seed[:i] for i in range(len(seed)))
        for i in range(len(seed)):
            cases.extend(seed[:i] + bytes([byte]) + seed[i+1:] for byte in range(256))
    for length in [4095, 4096, 4097, 8192]:
        target = b'/' + b'x' * (length - 1)
        for method in [b'GET', b'G\x00T', b'', b'!']:
            for version in [b'HTTP/1.1', b'HTTP/1.0', b'', b'HTTP/1.1 ']:
                cases.extend([method+b' '+target+b' '+version, method+b' '+target+b'\x00 '+version])
    randomizer = random.Random(3181)
    for _ in range(20000):
        data = bytes(randomizer.randrange(256) for _ in range(randomizer.randrange(97)))
        cases.append(data)
    corpus = args.workdir / 'cases.bin'
    corpus.write_bytes(b''.join(struct.pack('<qI', reference(data), len(data)) + data for data in cases))
    rows = []
    sdk = ROOT / 'sdk/httpcodec'
    for sanitized in [False, True]:
        binary = args.workdir / ('san' if sanitized else 'plain')
        command = ['clang', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror',
                   '-I', str(sdk / 'native'), str(sdk / 'native/request_line.c'),
                   str(sdk / 'tests/native.c'), '-o', str(binary)]
        if sanitized:
            command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
        subprocess.run(command, check=True)
        run = subprocess.run([str(binary), str(corpus)], check=True, capture_output=True, text=True)
        rows.append(dict(sanitized=sanitized, passed=True, stdout=run.stdout, command=command))
    args.output.write_text(json.dumps(dict(passed=True, cases=len(cases), runs=rows,
        corpusSha256=hashlib.sha256(corpus.read_bytes()).hexdigest(),
        kernelSha256=hashlib.sha256((sdk / 'native/request_line.c').read_bytes()).hexdigest()), indent=2)+'\n')
    print(json.dumps(rows))


if __name__ == '__main__':
    main()
