def dict_from_str(s):
    if not isinstance(s, str):
        return s
    result = {}
    if not s:
        return result
    vals = s.split(',')
    for v in vals:
        name, value = v.split('=')
        if value.find('||') != -1:
            _value = value.split('||')
            value = []
            for _v in _value:
                if _v.find('|') != -1:
                    value.append(arr_from_str(_v))
                else:
                    value.append([_v])
        else:
            value = arr_from_str(value)
        if isinstance(value, str):
            try:
                value = float(value)
                if value == int(value):
                    value = int(value)
            except:
                pass
        result[name] = value
    return result


def arr_from_str(s):
    if not isinstance(s, str) or s.find('|') == -1:
        return s
    result = []
    vals = s.split('|')
    for v in vals:
        try:
            _v = float(v)
            if _v == int(_v):
                _v = int(_v)
        except:
            _v = v
        result.append(_v)
    return result
