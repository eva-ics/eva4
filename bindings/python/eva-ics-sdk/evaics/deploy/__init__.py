__version__ = '0.2.32'

import copy

DEPLOY_VERSION = 4


def set_value_by_path(data, key, value):
    keys = key.split('/')
    d = data
    for k in keys[:-1]:
        d = d.setdefault(k, {})
    d[keys[-1]] = value


def init_value_by_path(data, key, value):
    keys = key.split('/')
    d = data
    for k in keys[:-1]:
        d = d.setdefault(k, {})
    return d.setdefault(keys[-1], value)


class Node:
    """
    Node class is used to represent a single node in the deploy file.
    """

    def __init__(self, name: str = '.local'):
        """
        Args:
            name - the name of the node (default: '.local')
        """
        self.payload = dict(node=name)

    def to_dict(self):
        """
        Convert the Node object to a dictionary
        """
        return self.payload

    def clone(self):
        """
        Clone the Node object
        """
        return copy.deepcopy(self)

    def __repr__(self):
        import yaml
        return yaml.dump(self.to_dict(), default_flow_style=False)

    def add(self, key: str, value):
        """
        Add data to the node
        """
        data = init_value_by_path(self.payload, key, [])
        if isinstance(data, list):
            data.append(value)
        else:
            set_value_by_path(self.payload, key, value)
        return self

    def add_from_export(self, path: str):
        """
        Add data from an export file
        """
        import yaml
        with open(path, 'r') as f:
            data = yaml.safe_load(f)
        for key, values in data.items():
            for value in values:
                self.add(key, value)

    def set(self, key: str, value):
        """
        Set data to the node by key
        """
        set_value_by_path(self.payload, key, value)
        return self

    def add_element(self, element):
        """
        Add an element to the node
        """
        key = element.element_id()
        value = element.to_dict()
        if not key:
            raise ValueError('Element ID is required')
        self.add(key, value)
        return self

    def param(self, key: str, value):
        """
        Specify a parameter for the node
        """
        self.payload.setdefault('params', dict())[key] = value
        return self


class Deploy:
    """
    Deploy class is used to represent the deploy file
    """

    def __init__(self):
        self.content = []

    def add(self, node: Node):
        """
        Add a node to the deploy file
        """
        self.content.append(node)
        return self

    def merge(self, other):
        """
        Merge another deploy with the current one

        Args:
            other - the other deploy object or a path to a deploy file
        """
        if isinstance(other, str):
            other = load_deploy(other)
        for c in other.content:
            node_name = c.payload['node']
            found = False
            for node in self.content:
                if node.payload['node'] == node_name:
                    found = True
                    for item in c.payload:
                        if item == 'node':
                            continue
                        c = c.clone()
                        for (k, v) in c.payload.items():
                            if isinstance(v, list):
                                for e in v:
                                    node.add(k, e)
                            else:
                                node.set(k, v)
            if not found:
                self.content.append(c)
        return self

    def clone(self):
        """
        Clone the Deploy object
        """
        d = Deploy()
        d.content = [node.clone() for node in self.content]
        return

    def to_dict(self):
        """
        Convert the Deploy object to a dictionary
        """
        return {
            'version': DEPLOY_VERSION,
            'content': [node.to_dict() for node in self.content]
        }

    def to_yaml(self):
        """
        Convert the Deploy object to a YAML string (requires PyYAML)
        """
        import yaml
        return yaml.dump(self.to_dict(), default_flow_style=False)

    def to_json(self):
        """
        Convert the Deploy object to a JSON string
        """
        import json
        return json.dumps(self.to_dict(), indent=2, sort_keys=True)

    def print(self, *args, **kwargs):
        """
        Print the Deploy object to the console (requires PyYAML)
        """
        print(self.to_yaml(), *args, **kwargs)

    def save(self, path: str):
        """
        Save the Deploy object to a file (requires PyYAML)
        """
        with open(path, 'w') as f:
            f.write(self.to_yaml())

    def __repr__(self):
        return self.to_yaml()


def load_deploy(path: str):
    """
    Load a deployment configuration from a path
    """
    import yaml
    with open(path, 'r') as f:
        data = yaml.safe_load(f)
    version = data.get('version')
    if version != DEPLOY_VERSION:
        raise ValueError(f'Invalid deploy version: {version}')
    content = data.get('content', [])
    d = Deploy()
    for n in content:
        node = Node(n.get('node'))
        node.payload = n
        d.content.append(node)
    return d


class Element:
    """
    Basic abstract node element class
    """

    def __init__(self):
        self.payload = dict()

    def set(self, key: str, value):
        """
        Set data to the element by key
        """
        set_value_by_path(self.payload, key, value)
        return self

    def to_dict(self):
        """
        Convert the Element object to a dictionary
        """
        return self.payload

    def clone(self):
        """
        Clone the Element object
        """
        return copy.deepcopy(self)

    def __repr__(self):
        import yaml
        return yaml.dump(self.to_dict(), default_flow_style=False)

    def element_id(self) -> str:
        return ''


class Item(Element):
    """
    Item class is used to represent a node item
    """

    def __init__(self, oid):
        """
        Args:
            oid - the item OID
        """
        super().__init__()
        super().set('oid', str(oid))

    def element_id(self):
        return 'items'

    def action_svc(self, svc):
        """
        Specify the action service
        """
        self.payload.setdefault('action', dict())['svc'] = svc
        return self

    def action_timeout(self, timeout):
        """
        Specify the action timeout
        """
        self.payload.setdefault('action', dict())['timeout'] = timeout
        return self


class ACL(Element):
    """
    ACL class is used to represent an access control list
    """

    def __init__(self, id):
        """
        Args:
            id - the ACL ID
        """
        super().__init__()
        super().set('id', id)

    def element_id(self):
        return 'acls'


class Key(Element):
    """
    Key class is used to represent an API key
    """

    def __init__(self, id):
        """
        Args:
            id - the key ID
        """
        super().__init__()
        super().set('id', id)

    def element_id(self):
        return 'keys'


class User(Element):
    """
    User class is used to represent a user
    """

    def __init__(self, login):
        """
        Args:
            login - the user login
        """
        super().__init__()
        super().set('login', login)

    def element_id(self):
        return 'users'


class Upload(Element):
    """
    Upload class is used to represent a file upload element
    """

    def __init__(self, src=None, text=None, target=None):
        """
        Args:
            src - the source file path or URL
            target - the target file path
        """
        super().__init__()
        if target is None:
            target = '/'
        if src is not None and text is not None:
            raise ValueError('Both src and text cannot be set')
        if src is not None:
            if src.startswith('http://') or src.startswith('https://'):
                super().set('url', src)
            else:
                super().set('src', src)
        if text is not None:
            super().set('text', text)
        super().set('target', target)

    def element_id(self):
        return 'upload'


def service_from_tpl(id: str, tpl_path):
    """
    Create a service from a template file (requires PyYAML)
    """
    import yaml
    with open(tpl_path, 'r') as f:
        tpl = yaml.safe_load(f)
    svc = Service(id, tpl['command'])
    svc.set('params', tpl)
    return svc


class Service(Element):
    """
    Service class is used to represent a service
    """

    def __init__(self, id: str, command: str):
        """
        Args:
            id - the service ID
            command - the service command
        """
        super().__init__()
        super().set('id', id)
        params = {
            'bus': {
                'path': 'var/bus.ipc'
            },
            'command': command,
            'config': {},
            'workers': 1,
            'user': 'nobody',
        }
        super().set('params', params)

    def user(self, user: str):
        """
        Specify the user for the service
        """
        self.payload['params']['user'] = user
        return self

    def workers(self, workers: int):
        """
        Specify the number of workers for the service
        """
        self.payload['params']['workers'] = workers
        return self

    def config(self, config: dict):
        """
        Specify the configuration for the service
        """
        self.payload['params']['config'] = config
        return

    def element_id(self):
        return 'svcs'


class DataObject(Element):
    """
    DataObject class is used to represent a data object
    """

    def __init__(self, name):
        """
        Args:
            name - the data object name
        """
        super().__init__()
        super().set('name', name)
        super().set('fields', [])

    def fields(self, fields: list):
        """
        Specify the fields for the data object
        """
        self.payload['fields'] = fields
        return self

    def field(self, name: str, type: str):
        """
        Add a field to the data object
        """
        self.payload['fields'].append(dict(name=name, type=type))
        return self

    def element_id(self):
        return 'data_objects'


class GeneratorSource(Element):
    """
    GeneratorSource class is used to represent a generator source
    """

    def __init__(self, name: str, sampling: int):
        """
        Args:
            name - the generator source name
            sampling - the generator source sampling frequency
        """
        super().__init__()
        super().set('name', name)
        super().set('sampling', sampling)
        super().set('params', {})
        super().set('targets', [])

    def params(self, params: dict):
        """
        Specify the parameters for the generator source
        """
        self.payload['params'] = params
        return self

    def targets(self, targets: list):
        """
        Specify the target items for the generator source
        """
        self.payload['targets'] = targets
        return self

    def target(self, target):
        """
        Add a target item to the generator source
        """
        self.payload['targets'].append(str(target))
        return self

    def element_id(self):
        return 'generator_sources'


class Alarm(Element):
    """
    Alarm class is used to represent an alarm
    """

    def __init__(self, group: str, id: str, level: int):
        """
        Args:
            group - the alarm group
            id - the alarm ID
            level - the alarm level
        """
        super().__init__()
        super().set('group', group)
        super().set('id', id)
        super().set('level', level)

    def element_id(self):
        return 'alarms'


class ExtraComamand(Element):

    def __init__(self, on='deploy', stage='after'):
        super().__init__()
        if on not in ['deploy', 'undeploy']:
            raise ValueError(f'Invalid on value: {on}')
        self.on = on
        if stage not in ['before', 'after']:
            raise ValueError(f'Invalid stage value: {stage}')
        self.stage = stage

    def element_id(self):
        return f'extra/{self.on}/{self.stage}'


class EAPICall(ExtraComamand):
    """
    EAPICall class is used to represent an API call
    """

    def __init__(self, method: str, on='deploy', stage='after'):
        super().__init__(on, stage)
        super().set('method', method)

    def params(self, params: dict):
        self.payload['params'] = params
        return self


class Function(ExtraComamand):
    """
    Function class is used to represent a function call
    """

    def __init__(self, function: str, *args, on='deploy', stage='after'):
        """
        Args:
            function - the function name
            args - the function arguments, passed as-is
            on - deploy/undeploy (default: 'deploy')
            stage - the stage value, before/after (default: 'after')
        """
        super().__init__(on, stage)
        super().set('function', function)
        super().set('args', list(args))
