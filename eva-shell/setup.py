__version__ = '0.2.18'

import setuptools

with open('README.md', 'r') as fh:
    long_description = fh.read()

setuptools.setup(name='eva-shell',
                 version=__version__,
                 author='Bohemia Automation / Altertech',
                 author_email='div@altertech.com',
                 description='EVA ICS v4 shell',
                 long_description=long_description,
                 long_description_content_type='text/markdown',
                 url='https://github.com/eva-ics/eva4',
                 packages=setuptools.find_packages(),
                 license='Apache License 2.0',
                 install_requires=[
                     'busrt>=0.1.0', 'evaics>=0.0.32', 'yedb[cli]>=0.2.25',
                     'argcomplete>=2.0.0', 'python-dateutil>=2.7.3',
                     'neotermcolor>=2.0.10', 'pyyaml>=6.0', 'pygments>=2.11.2',
                     'pytz>=2024.1', 'pwinput>=1.0.2', 'tzlocal>=5.2'
                 ],
                 classifiers=('Programming Language :: Python :: 3',
                              'License :: OSI Approved :: MIT License',
                              'Topic :: Communications'),
                 scripts=['bin/eva'])
