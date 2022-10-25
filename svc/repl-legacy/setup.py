__version__ = '0.0.25'

import setuptools

with open('README.md', 'r') as fh:
    long_description = fh.read()

setuptools.setup(name='eva4-repl-legacy',
                 version=__version__,
                 author='Bohemia Automation / Altertech',
                 author_email='div@altertech.com',
                 description='EVA ICS v4 legacy replication service',
                 long_description=long_description,
                 long_description_content_type='text/markdown',
                 url='https://github.com/eva-ics/eva4',
                 packages=setuptools.find_packages(),
                 license='Apache License 2.0',
                 install_requires=[
                     'cachetools>=4.11', 'busrt>=0.1.0', 'evaics>=0.0.33',
                     'psrt>=0.0.18', 'pyaltt2>=0.0.116', 'pycryptodomex>=3.12.0'
                 ],
                 classifiers=('Programming Language :: Python :: 3',
                              'License :: OSI Approved :: MIT License',
                              'Topic :: Communications'),
                 scripts=['bin/eva4-svc-repl-legacy'])
