import time
from types import SimpleNamespace
import pytz

try:
    time_zone = pytz.timezone(time.tzname[0])
except:
    import tzlocal
    time_zone = pytz.timezone(tzlocal.get_localzone_name())

common = SimpleNamespace(dir_eva=None,
                         bus_path=None,
                         bus_name=None,
                         bus_conn_no=0,
                         bus=None,
                         rpc=None,
                         cli=None,
                         interactive=False,
                         public_key=None,
                         TZ=time_zone)
current_command = SimpleNamespace(json=False,
                                  debug=False,
                                  timeout=5,
                                  exit_code=0)
