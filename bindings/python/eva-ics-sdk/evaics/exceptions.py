class GenericException(Exception):

    def __init__(self, msg=''):
        super().__init__(str(msg))


class InvalidParameter(Exception):

    def __str__(self):
        msg = super().__str__()
        return 'Invalid parameter' + (': {}'.format(msg) if msg else '')


class FunctionFailed(GenericException):
    """
    raised when a function failed is failed with any reason
    """

    def __str__(self):
        msg = super().__str__()
        return msg if msg else 'Function call failed'


class ResourceNotFound(GenericException):
    """
    raised when the requested resource is not found
    """

    def __str__(self):
        msg = super().__str__()
        return msg + ' not found' if msg else 'Resource not found'


class ResourceBusy(GenericException):
    """
    raised when the requested resource is busy or can not be modified
    """

    def __str__(self):
        msg = super().__str__()
        return msg if msg else 'Resource is in use'


class ResourceAlreadyExists(GenericException):
    """
    raised when the requested resource already exists
    """

    def __str__(self):
        msg = super().__str__()
        return msg + ' already exists' if msg else 'Resource already exists'


class AccessDenied(GenericException):
    """
    raised when a call has no access to the resource
    """

    def __str__(self):
        msg = super().__str__()
        return msg if msg else 'Access to resource is denied'


class MethodNotImplemented(GenericException):
    """
    raised when the requested method exists but requested functionality is not
    implemented
    """

    def __str__(self):
        msg = super().__str__()
        return msg if msg else 'Method not implemented'


class TimeoutException(GenericException):
    """
    raised when a call is timed out
    """
    pass
