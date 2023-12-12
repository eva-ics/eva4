import sys
import os
from neotermcolor import colored

from .sharedobj import common, current_command
from .cli import CLI
from .ap import init_ap

banner = """     _______    _____       _______________
    / ____/ |  / /   |     /  _/ ____/ ___/
   / __/  | | / / /| |     / // /    \__ \\
  / /___  | |/ / ___ |   _/ // /___ ___/ /
 /_____/  |___/_/  |_|  /___/\____//____/"""

home_url = "www.eva-ics.com"
copyright = "(c) Bohemia Automation"

dir_eva_default = '/opt/eva4'


def launch():
    common.dir_eva = os.environ.get('EVA_DIR', dir_eva_default)
    if not os.path.exists(common.dir_eva):
        raise RuntimeError(
            f'EVA ICS directory not found. Consider installing EVA ICS either '
            f'in {dir_eva_default} or specifying EVA_DIR env variable')
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
        ap.interactive()
        print('Bye')
