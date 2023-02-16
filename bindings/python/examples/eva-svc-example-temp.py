#!/opt/eva4/venv/bin/python

__version__ = '0.0.1'

import evaics.sdk as sdk
import busrt

from types import SimpleNamespace
from evaics.sdk import pack, unpack, OID, LOCAL_STATE_TOPIC

import threading

HYSTERESIS = 2.0

# define a global namespace
_d = SimpleNamespace(logger=None,
                     service=None,
                     counter=0,
                     threshold=0,
                     rcpt=None,
                     mailer_svc=None)

op_lock = threading.RLock()

# the notification map, keeps info which sensors were already processed
notified = {}


# RPC calls handler
def handle_rpc(event):
    _d.service.need_ready()
    # There is only one RPC method - "get_counter", which returns the number of
    # email messages sent
    if event.method == b'get_counter':
        try:
            return pack(dict(count=_d.counter))
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


# handle BUS/RT frames
def on_frame(frame):
    if _d.service.is_active():
        if frame.topic and frame.topic.startswith(LOCAL_STATE_TOPIC):
            # Parse sensor OID from the topic
            oid = OID(frame.topic[len(LOCAL_STATE_TOPIC):], from_path=True)
            # Parse sensor state
            state = unpack(frame.payload)
            # The next lines represent common Python code, so no comments are
            # provided
            temperature = float(state['value'])
            letter = None
            with op_lock:
                was_notified = notified.get(oid)
                if temperature > _d.threshold and not was_notified:
                    # notify high
                    text = f'{oid} temperature is {temperature}'
                    _d.logger.warning(text)
                    letter = {
                        'rcp': _d.rcpt,
                        'subject': f'{oid} is hot',
                        'text': text
                    }
                    notified[oid] = True
                elif temperature < _d.threshold - HYSTERESIS and was_notified:
                    # notify back to normal
                    text = f'{oid} temperature is {temperature}'
                    _d.logger.info(text)
                    letter = {
                        'rcp': _d.rcpt,
                        'subject': f'{oid} is back to normal',
                        'text': text
                    }
                    notified[oid] = False
            if letter and _d.rcpt and _d.mailer_svc:
                _d.counter += 1
                # Call the mailer service
                _d.service.rpc.call(_d.mailer_svc,
                                    busrt.rpc.Request(
                                        'send', pack(letter))).wait_completed()


def run():
    # define ServiceInfo object
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Temperature monitor',
                           version=__version__)
    # it is not obliged to include all available service RPC methods into
    # ServiceInfo, however including methods and their parameters provide
    # additional interface (the data is obtained by calling "info" RPC method),
    # e.g. auto-completion for eva-shell
    info.add_method('get_counter')
    # create a service object
    service = sdk.Service()
    _d.service = service
    # get the service config
    config = service.get_config()
    _d.threshold = config.get('threshold')
    _d.mailer_svc = config.get('mailer_svc')
    _d.rcpt = config.get('rcpt')
    # init the service bus
    service.init_bus()
    # drop the service process privileges
    service.drop_privileges()
    # init the service logger
    _d.logger = service.init_logs()
    # set RPC handler
    service.on_rpc_call = handle_rpc
    # init the service RPC
    service.init_rpc(info)
    # subscribe sensor OIDs via the helper method
    service.subscribe_oids(config.get('sensors'), event_kind='local')
    # set BUS/RT frame handler on the service RPC layer
    service.rpc.on_frame = on_frame
    # register service process signals
    service.register_signals()
    # mark the instance ready and active
    service.mark_ready()
    _d.logger.info('Temperature monitor service started')
    # the service is blocked until one of the following:
    # * RPC client is disconnected from the bus
    # * the service gets a termination signal
    service.block()
    # tell the core and other services that the service is terminating
    service.mark_terminating()


run()
