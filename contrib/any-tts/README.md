# any-tts svc

Any text-to-speech service

Written in Python, requires EVA ICS v4 and EVA ICS venv set. No additional
Python modules are required.

## What does it do

The service provides EAPI bus RPC method say(text) which calls a local
program/script to synthesize and play voice. The command must accept text from
the STDIN.

## Setup

Download [any-tts.py](any-tts.py), copy-paste the service template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/any-tts/any-tts.py
config:
  # the command must accept text from stdin
  # e.g. for PicoTTS
  # also make sure "aplay" is installed
  say: "pico-tts -l en-US|aplay -q -f S16_LE -r 16"
user: nobody
workers: 1
```

(edit the command path if necessary), then run

```shell
eva svc create eva.svc.anytts /path/to/template.yml
```

Install PicoTTS: see <https://github.com/Iiridayn/pico-tts>

## Usage

Test it with eva-shell:

```
eva svc call eva.svc.anytts say text="eva i c s is the best automation platform in the world"
```

Use "say" EAPI RPC method from any other service.
