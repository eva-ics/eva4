# EVA ICS v4 shell

## IaC exports

Use `--iac` with an export command to produce a version 4 deployment file:

```sh
eva svc export 'eva.svc.*' --iac --output deploy.yml
eva item export '#' --iac --output deploy.yml
```

Without `--output`, the deployment YAML is printed to stdout. With `--output`,
the file is created if absent or merged if it already contains a version 4
deployment. Exported resources with the same identity replace their previous
entries; other resources and nodes are retained. Exports merge into the first
node in the file. If the file has no nodes, a `.local` node is created.
An existing flat export is not silently converted or overwritten.

IaC export is supported for services, items, ACLs, keys, users, data objects,
generator sources, video recordings and alarms. Node and kiosk exports remain
flat because those resource types are not accepted by `cloud deploy`.
