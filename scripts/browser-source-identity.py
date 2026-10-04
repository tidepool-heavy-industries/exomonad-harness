#!/usr/bin/env python3
"""Issue a content identity from the declared Harness source input tree."""
import hashlib
import json
import pathlib
import sys

source, output = map(pathlib.Path, sys.argv[1:])
digest = hashlib.sha256(b'harness-browser-source-v1\0')
for path in sorted(path for path in source.rglob('*') if path.is_file()):
    name = path.relative_to(source).as_posix().encode('utf8')
    content = path.read_bytes()
    for value in (name, content):
        digest.update(len(value).to_bytes(8, 'big'))
        digest.update(value)
output.write_text(json.dumps({'runtimeSourceSha256': digest.hexdigest()}, sort_keys=True) + '\n')
