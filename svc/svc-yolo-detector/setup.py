__version__ = '0.1.0'

import setuptools

with open('README.md', 'r') as fh:
    long_description = fh.read()

setuptools.setup(
    name='eva4-svc-yolo-detector',
    version=__version__,
    author='Bohemia Automation / Altertech',
    author_email='div@altertech.com',
    description='EVA ICS v4 YOLO detector service',
    long_description=long_description,
    long_description_content_type='text/markdown',
    url='https://github.com/eva-ics/eva4',
    packages=setuptools.find_packages(),
    license='Apache License 2.0',
    install_requires=[
        'evaics>=0.2.35',
        'ultralytics>=8.3.0',
    ],
    classifiers=(
        'Programming Language :: Python :: 3',
        'License :: OSI Approved :: MIT License',
        'Topic :: Scientific/Engineering :: Artificial Intelligence',
    ),
    scripts=['bin/eva4-svc-yolo-detector'])
