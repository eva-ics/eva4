__version__ = '0.1.6'

import setuptools

with open('README.md', 'r') as fh:
    long_description = fh.read()

setuptools.setup(
    name='eva4-controller-py',
    version=__version__,
    author='Bohemia Automation / Altertech',
    author_email='div@altertech.com',
    description='EVA ICS v4 Python macros controller service',
    long_description=long_description,
    long_description_content_type='text/markdown',
    url='https://github.com/eva-ics/eva4',
    packages=setuptools.find_packages(),
    license='Apache License 2.0',
    install_requires=['busrt>=0.1.0', 'evaics>=0.2.9', 'timeouter>=0.0.15'],
    classifiers=('Programming Language :: Python :: 3',
                 'License :: OSI Approved :: MIT License',
                 'Topic :: Communications'),
    scripts=['bin/eva4-svc-controller-py'])
