__version__ = '0.1.2'

import setuptools

with open('README.md', 'r') as fh:
    long_description = fh.read()

setuptools.setup(name='eva4-bridge-udp',
                 version=__version__,
                 author='Bohemia Automation / Altertech',
                 author_email='div@altertech.com',
                 description='EVA ICS v4 UDP bridge',
                 long_description=long_description,
                 long_description_content_type='text/markdown',
                 url='https://github.com/eva-ics/eva4',
                 packages=setuptools.find_packages(),
                 license='Apache License 2.0',
                 install_requires=['busrt>=0.1.0', 'evaics>=0.2.9'],
                 classifiers=('Programming Language :: Python :: 3',
                              'License :: OSI Approved :: MIT License',
                              'Topic :: Communications'),
                 scripts=['bin/eva4-svc-bridge-udp'])
