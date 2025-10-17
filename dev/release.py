#!/usr/bin/env python3

import argparse
import os
import json
import random

archs = ['aarch64-musl', 'x86_64-musl', 'x86_64-ubuntu20.04']

ap = argparse.ArgumentParser()

uri = 'pub.bma.ai/eva4'


def gsutil(params):
    cmd = f'gsutil -m {params}'
    print(cmd)
    if os.system(cmd):
        raise RuntimeError


ap.add_argument('version')
ap.add_argument('build')
ap.add_argument('--test',
                help='Update update_info.json for test only',
                action='store_true')
ap.add_argument('-u',
                '--update-info',
                help='Update update_info',
                action='store_true')

a = ap.parse_args()

if not a.test:
    for arch in archs:
        f = f'eva-{a.version}-{a.build}-{arch}.tgz'
        gsutil(f'cp -a public-read '
               f'gs://{uri}/{a.version}/nightly/{f}'
               f' gs://{uri}/{a.version}/stable/{f}')

    fname = '/tmp/update{}.sh'.format(random.randint(1, 1000000))
    gsutil(f'cp '
           f'gs://{uri}/{a.version}/nightly/update-{a.build}.sh '
           f' {fname}')
    gsutil(f'-h "Cache-Control:no-cache" cp -a public-read '
           f'{fname} '
           f' gs://{uri}/{a.version}/stable/update.sh')
    os.unlink(fname)

    gsutil(f'cp -a public-read '
           f'gs://{uri}/{a.version}/nightly/CHANGELOG.html'
           f' gs://{uri}/{a.version}/stable/CHANGELOG.html')

    gsutil(f'-h "Content-Type:text/x-rst" cp -a public-read '
           f'gs://{uri}/{a.version}/nightly/UPDATE.rst '
           f'gs://{uri}/{a.version}/stable/UPDATE.rst')

    gsutil(f'cp -a public-read '
           f'gs://{uri}/{a.version}/nightly/manifest-{a.build}.json'
           f' gs://{uri}/{a.version}/stable/manifest-{a.build}.json')

if a.update_info or a.test:
    fname = '/tmp/update_info_{}.json'.format(random.randint(1, 1000000))
    try:
        ftarget = 'update_info{}.json'.format('_test' if a.test else '')
        with open(fname, 'w') as fh:
            fh.write(json.dumps({
                'version': a.version,
                'build': int(a.build),
            }))
        gsutil(f'-h "Cache-Control:no-cache" cp -a public-read '
               f'{fname} gs://{uri}/{ftarget}')
    finally:
        try:
            os.unlink(fname)
        except FileNotFoundError:
            pass
