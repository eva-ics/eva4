__version__ = '0.1.0'

import evaics.sdk as sdk

from types import SimpleNamespace
from evaics.sdk import pack, unpack

from ldap3 import Server, Connection, Tls, ALL
import ssl
from threading import Lock


class Authenticator:

    def __init__(self,
                 ldap_url,
                 service_user,
                 service_password,
                 ldap_path,
                 group_prefix,
                 timeout=5,
                 ca_certs=None):
        self.ldap_url = ldap_url
        self.service_user = service_user
        self.service_password = service_password
        self.ldap_path = ldap_path
        self.timeout = timeout
        self.group_prefix = group_prefix
        if ca_certs:
            tls_config = Tls(validate=ssl.CERT_REQUIRED,
                             version=ssl.PROTOCOL_TLSv1_2,
                             ca_certs_file=ca_certs)
        else:
            tls_config = None
        self.server = Server(self.ldap_url,
                             get_info=ALL,
                             tls=tls_config,
                             connect_timeout=self.timeout)
        self.conn = None
        self.lock = Lock()

    def authenticate(self, login, password, verify_only=False):
        with self.lock:
            self.prepare_conn()
            if '@' in login:
                login = self.get_username_by_email(login)
            if verify_only is not True:
                self.authenticate_ldap(login, password)
            search_dn = f"cn={login},{self.ldap_path}"
            try:
                self.conn.search(search_base=search_dn,
                                 search_filter="(objectClass=*)",
                                 attributes=["memberOf"],
                                 time_limit=self.timeout)
            except:
                self.conn = None
                raise
            if self.conn.entries is None:
                raise RuntimeError("Invalid username")
            groups = [
                group.split(',', maxsplit=1)[0].split('=', maxsplit=1)[1]
                for group in self.conn.entries[0].memberOf
            ]
            return [
                group[len(self.group_prefix):]
                for group in groups
                if group.startswith(self.group_prefix)
            ]

    def prepare_conn(self):
        if self.conn is None:
            self.conn = Connection(
                self.server,
                user=f"cn={self.service_user},{self.ldap_path}",
                password=self.service_password,
                auto_bind=True,
                receive_timeout=self.timeout)

    def authenticate_ldap(self, login, password):
        if not password:
            raise RuntimeError("Password not provided")
        Connection(self.server,
                   user=f"cn={login},{self.ldap_path}",
                   password=password,
                   auto_bind=True)

    def get_username_by_email(self, email):
        try:
            self.conn.search(search_base=self.ldap_path,
                             search_filter=f"(mail={email})",
                             attributes=["cn"],
                             time_limit=self.timeout)
        except:
            self.conn = None
            raise

        if self.conn.entries is None:
            raise RuntimeError("Invalid email address")

        return self.conn.entries[0].cn.value


_d = SimpleNamespace(service=None, authenticator=None, acl_svc=None)


def handle_rpc(event):
    if event.method == b'auth.user':
        params = unpack(event.get_payload())
        login = params['login']
        password = params.get('password', '')
        externally_verified = params.get('externally_verified', False)
        try:
            groups = _d.authenticator.authenticate(
                login, password, verify_only=externally_verified)
            return pack(
                _d.service.call('acl.format', dict(i=groups),
                                target=_d.acl_svc))
        except Exception as e:
            raise sdk.AccessDenied(str(e))
    sdk.no_rpc_method()


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='LDAP Authentication Service',
                           version=__version__)
    service = sdk.Service()
    _d.service = service
    config = service.get_config()
    timeout = int(service.timeout['default'])
    group_prefix = config.get('group_prefix', 'eva_')
    ldap_url = config['url']
    ldap_path = config['path']
    service_user = config['service_user']
    tls_ca = config.get('tls_ca', None)
    service_password = config['service_password']
    _d.acl_svc = config['acl_svc']
    _d.authenticator = Authenticator(ldap_url=ldap_url,
                                     service_user=service_user,
                                     service_password=service_password,
                                     ldap_path=ldap_path,
                                     timeout=timeout,
                                     ca_certs=tls_ca,
                                     group_prefix=group_prefix)
    service.init(info, on_rpc_call=handle_rpc)
    try:
        _d.authenticator.prepare_conn()
    except Exception as e:
        _d.service.logger.error(f"Failed to connect to LDAP server: {e}")
    service.block()
