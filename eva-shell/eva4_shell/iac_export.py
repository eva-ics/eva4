"""Write flat EVA exports into a local-node IaC deployment file."""

import os
import stat
import tempfile

import yaml


RESOURCE_IDS = {
    'acls': ('id',),
    'keys': ('id',),
    'users': ('login',),
    'items': ('oid',),
    'svcs': ('id',),
    'data_objects': ('name',),
    'generator_sources': ('name',),
    'video_recordings': ('oid',),
    'alarms': ('group', 'id'),
}


def _resource_id(resource, value):
    if not isinstance(value, dict):
        raise ValueError(f'{resource} entries must be mappings')
    fields = RESOURCE_IDS[resource]
    try:
        identity = tuple(value[field] for field in fields)
    except KeyError as e:
        raise ValueError(f'{resource} entry is missing {e.args[0]}') from e
    if any(field is None for field in identity):
        raise ValueError(f'{resource} entry has an empty identity')
    return identity


def merge_export(existing, export):
    """Replace matching resources in the .local node and retain all other data."""
    if not isinstance(export, dict) or len(export) != 1:
        raise ValueError('expected an export with one resource section')
    resource, new_values = next(iter(export.items()))
    if resource not in RESOURCE_IDS:
        raise ValueError(f'{resource} is not supported by IaC deployment')
    if not isinstance(new_values, list):
        raise ValueError(f'{resource} export must be a list')
    if existing is None:
        existing = {'version': 4, 'content': [{'node': '.local'}]}
    if not isinstance(existing, dict) or existing.get('version') != 4:
        raise ValueError('existing output is not an IaC version 4 deployment')
    content = existing.get('content')
    if not isinstance(content, list) or any(
            not isinstance(node, dict) or not isinstance(node.get('node'), str)
            for node in content):
        raise ValueError('existing IaC content must be a list of nodes')
    if content:
        node = content[0]
    else:
        node = {'node': '.local'}
        content.append(node)
    old_values = node.get(resource, [])
    if not isinstance(old_values, list):
        raise ValueError(f'existing {resource} section must be a list')
    merged = list(old_values)
    positions = {}
    for index, value in enumerate(merged):
        identity = _resource_id(resource, value)
        if identity in positions:
            raise ValueError(f'existing {resource} section has duplicate IDs')
        positions[identity] = index
    for value in new_values:
        identity = _resource_id(resource, value)
        if identity in positions:
            merged[positions[identity]] = value
        else:
            positions[identity] = len(merged)
            merged.append(value)
    node[resource] = merged
    return existing


def write_iac_export(export, output=None, json=False):
    """Emit IaC to stdout, or atomically create/merge a deployment file."""
    if output is None or json:
        return merge_export(None, export)
    path = os.path.expanduser(output)
    exists = os.path.exists(path)
    if exists:
        with open(path, encoding='utf-8') as source:
            existing = yaml.safe_load(source)
        mode = stat.S_IMODE(os.stat(path).st_mode)
    else:
        existing = None
        mode = None
    payload = merge_export(existing, export)
    dump = yaml.safe_dump(payload, default_flow_style=False, sort_keys=False)
    directory = os.path.dirname(os.path.abspath(path))
    fd, temp_path = tempfile.mkstemp(prefix='.eva-iac-', dir=directory)
    try:
        if mode is not None:
            os.fchmod(fd, mode)
        with os.fdopen(fd, 'w', encoding='utf-8') as target:
            target.write(dump)
        os.replace(temp_path, path)
    finally:
        if os.path.exists(temp_path):
            os.unlink(temp_path)
    print(f'{"merging" if exists else "creating"} {path}')
    return payload
