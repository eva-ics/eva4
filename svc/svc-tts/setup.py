__version__ = '0.0.6'

import setuptools

with open('README.md', 'r') as fh:
    long_description = fh.read()

setuptools.setup(name='eva4-svc-tts',
                 version=__version__,
                 author='Bohemia Automation / Altertech',
                 author_email='div@altertech.com',
                 description='EVA ICS v4 text-to-speech service',
                 long_description=long_description,
                 long_description_content_type='text/markdown',
                 url='https://github.com/eva-ics/eva4',
                 packages=setuptools.find_packages(),
                 license='Apache License 2.0',
                 install_requires=[
                     'evaics>=0.0.33', 'ttsbroker', 'soundfile', 'sounddevice',
                     'oauth2client', 'numpy'
                 ],
                 classifiers=('Programming Language :: Python :: 3',
                              'License :: OSI Approved :: MIT License',
                              'Topic :: Communications'),
                 scripts=['bin/eva4-svc-tts'])
