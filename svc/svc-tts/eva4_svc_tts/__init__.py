__version__ = '0.0.6'

import evaics.sdk as sdk
import sys
import busrt
import ttsbroker
from types import SimpleNamespace
from evaics.sdk import pack, unpack

_d = SimpleNamespace(tts=None, logger=None)


def handle_rpc(event):
    if event.method == b'say':
        try:
            params = unpack(event.get_payload())
            text = params.pop('text')
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_INVALID_PARAMS)
        try:
            _d.tts.say(text, **params)
            return
        except Exception as e:
            import traceback
            _d.logger.error(traceback.format_exc())
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='text-to-speech service',
                           version=__version__)
    info.add_method('say', required=['text'])
    service = sdk.Service()
    _d.service = service
    config = service.get_config()
    _d.tts = ttsbroker.TTSEngine(storage_dir=config.get('storage_dir'),
                                 cache_dir=config.get('cache_dir'),
                                 cache_format=config.get('cache_format', 'wav'),
                                 device=config.get('device', 0),
                                 gain=config.get('gain', 0),
                                 provider=config['provider'],
                                 provider_options=config.get('options', {}),
                                 cmd=config.get('playback_cmd'))
    key_file = config.get('key_file')
    if key_file:
        _d.tts.set_key(key_file)
    service.init_bus()
    service.drop_privileges()
    _d.logger = service.init_logs()
    service.on_rpc_call = handle_rpc
    service.init_rpc(info)
    service.register_signals()
    service.mark_ready()
    _d.logger.info('text-to-speech service started')
    service.block()
    service.mark_terminating()
