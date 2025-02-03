import sys
import signal
import os
from neotermcolor import colored

from .sharedobj import common, current_command
from .cli import CLI
from .ap import init_ap
from .tools import is_local_shell

banner = """     _______    _____       _______________
    / ____/ |  / /   |     /  _/ ____/ ___/
   / __/  | | / / /| |     / // /    \__ \\
  / /___  | |/ / ___ |   _/ // /___ ___/ /
 /_____/  |___/_/  |_|  /___/\____//____/"""

home_url = "www.eva-ics.com"
copyright = "(c) Bohemia Automation"

dir_eva_default = '/opt/eva4'


def launch():
    bus = os.environ.get('EVA_BUS')
    if bus:
        common.dir_eva = None
        common.bus_path = bus
        import socket
        common.bus_name = f'eva-shell.{socket.gethostname()}.{os.getpid()}'
    else:
        common.dir_eva = os.environ.get('EVA_DIR', dir_eva_default)
        if not os.path.exists(common.dir_eva):
            raise RuntimeError(
                f'EVA ICS directory not found. Consider installing EVA ICS either '
                f'in {dir_eva_default} or specifying EVA_DIR env variable. '
                f'For For a remote host, set EVA_BUS=HOST:PORT env variable')
        common.bus_path = f'{common.dir_eva}/var/bus.ipc'
        common.bus_name = f'eva-shell.{os.getpid()}'

    common.cli = CLI()

    ap = init_ap()

    if len(sys.argv) > 1:
        ap.launch()
        exit(current_command.exit_code)
    else:
        lines = banner.split('\n')
        size = len(max(lines, key=len))
        pal = 3
        n = size // (pal - 1)
        for line in banner.split('\n'):
            chunks = [line[i:i + n] for i in range(0, len(line), n)]
            for i, ch in enumerate(chunks):
                if i > pal:
                    i = pal
                print(colored(ch, color=31 + i), end='')
            print()
        print()
        print(' ' + colored(home_url, color=24, attrs=['underline']), end=' ')
        print(colored(copyright, color=25))
        print()
        common.cli.version()
        common.interactive = True

        def handle_signal(signum, frame):
            import readline
            if ap.interactive_history_file:
                try:
                    readline.write_history_file(
                        os.path.expanduser(ap.interactive_history_file))
                except:
                    pass
            sys.exit(0)

        signal.signal(signal.SIGTERM, handle_signal)
        signal.signal(signal.SIGHUP, handle_signal)

        ap.interactive()
        print('Bye')
