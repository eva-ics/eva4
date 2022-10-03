def sleep(t, safe=True):
    """
    pause operations

    Unlike standard time.sleep(...), breaks pause when controller shutdown
    event is received.

    Args:
        t: number of seconds to sleep

    Optional:
        safe: break on shutdown event (default is True)

    Returns:
        True if sleep is finished, False if shutdown event is received
    """
    if safe:
        import time
        time_start = time.perf_counter()
        time_end = time_start + t
        while time.perf_counter() < time_end:
            if is_shutdown():
                return False
            _sleep(_polldelay)
        return True
    else:
        _sleep(t)
        return True


def alias(alias_obj, src_obj):
    """
    create object alias

    Args:
        alias_obj: alias object
        src_obj: source object

    Returns:
        True if alias is set. Doesn't raise any exceptions, safe to use in
        common files
    """
    g = globals()
    try:
        g[alias_obj] = g[src_obj]
        return True
    except:
        return False
