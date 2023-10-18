class Client:
    """
    Cloud client for EVA ICS
    """

    def __init__(self, client):
        """
        Create a new Cloud client instance

        Args:
            client: HTTP or BUS/RT client
        """
        self.client = client
        self.node_map = None
        self.system_name = None

    def prepare(self):
        """
        Prepare the client

        Load node map from the local core, required to be called at least once
        """
        self.system_name = self.client.test().system_name
        self.node_map = {
            n['name']: n['svc']
            for n in self.client.bus_call('node.list')
            if n['remote'] is True
        }

    def bus_call(self,
                 method: str,
                 params: dict = None,
                 target='eva.core',
                 node=None):
        """
        Call BUS/RT EAPI method

        Requires admin permissions

        Args:
            method: API method

        Optional:

            params: API method parameters (dict)

            target: target service (default: eva.core)

            node: target node (.local can be used for the local)

        Returns:
            API response payload
        """
        if self.system_name is None:
            raise RuntimeError('client not prepared')
        if node is None or node == '.local' or node == self.system_name:
            return self.client.bus_call(method, params, target)
        else:
            try:
                target_repl_svc = self.node_map[node]
            except KeyError:
                raise RuntimeError(f'node {node} is unknown')
            if params is None:
                params = {}
            params['node'] = node
            return self.client.bus_call(f'bus::{target}::{method}',
                                        params,
                                        target=target_repl_svc)
