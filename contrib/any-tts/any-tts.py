__version__ = '0.0.1'

import evaics.sdk as sdk
import subprocess
import busrt

from types import SimpleNamespace

d = SimpleNamespace(say_command=None)


def say(text):
    p = subprocess.run(d.say_command, input=text.encode(), shell=True)
    if p.returncode:
        raise RuntimeError(f'say command exited with {p.returncode}')


def handle_rpc(event):
    if event.method == b'say':
        try:
            say(sdk.unpack(event.get_payload())['text'])
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Any-TTS service',
                           version=__version__)
    info.add_method('say', required=['text'])
    service = sdk.Service()
    config = service.get_config()
    d.say_command = config['say']
    service.init(info, on_rpc_call=handle_rpc)
    service.block()


run()
