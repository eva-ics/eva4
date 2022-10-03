#!/usr/bin/env python3

import argparse
import os
import requests
import sys
import json
import random

import pyaltt2.crypto

ap = argparse.ArgumentParser()

ap.add_argument('--version', required=True)
ap.add_argument('--build', required=True)
ap.add_argument('--key-dir', required=True)
ap.add_argument('--rsa-tool', required=True)
ap.add_argument('--archs', required=True)
ap.add_argument('--output', required=True)

a = ap.parse_args()


def sign(fname, name=None):
    data = json.loads(
        os.popen(f'{a.rsa_tool} -J sign {fname} {pvt_key}').read())
    sig = data['signature']
    if os.system(f'{a.rsa_tool} verify {fname} {pub_key} {sig} > /dev/null'):
        raise RuntimeError
    content[name if name else fname] = dict(size=os.path.getsize(fname),
                                            sha256=data['sha256'],
                                            signature=sig)


content = {}
pvt_key = f'{a.key_dir}/private.der'
pub_key = f'{a.key_dir}/public.der'
for arch in a.archs.split():
    sign(f'eva-{a.version}-{a.build}-{arch}.tgz')
sign('eva4/update.sh', f'update-{a.build}.sh')

with open(a.output, 'w') as fh:
    fh.write(
        json.dumps({
            'version': a.version,
            'build': int(a.build),
            'content': content
        }))
