import {
  ServiceInfo,
  ServiceMethod,
  Service,
  pack,
  unpack,
  Action,
  XCall,
  XCallData,
  noRpcMethod,
  RpcEvent
} from "@eva-ics/sdk";

const onFrame = async (e: RpcEvent): Promise<void> => {
  console.log(e);
};

const main = async () => {
  const service = new Service();
  await service.load();
  const onRpcCall = async (e: RpcEvent): Promise<Buffer | undefined> => {
    const method = e.method?.toString();
    const payload = unpack(e.getPayload());
    switch (method) {
      case "x":
        const xcall = new XCall(payload as XCallData);
        xcall.acl.requireAdmin();
        console.log(xcall);
        return pack({ ok: true });
      case "run":
        const lAction = new Action(payload);
        console.log(lAction);
        service.controller.eventRunning(lAction);
        service.controller.eventFailed(
          lAction,
          "lmacro execution",
          "execution failed: not supported",
          -15
        );
        return;
      case "action":
        const uAction = new Action(payload);
        console.log(uAction);
        service.controller.eventRunning(uAction);
        service.controller.eventCompleted(uAction, "all fine");
        return;
      case "hello":
        return payload?.name ? pack(`hello ${payload.name}`) : pack("hi there");
      case "config":
        return pack(service.initial);
      default:
        noRpcMethod();
    }
  };
  await service.init({
    info: new ServiceInfo({
      author: "Bohemia Automation",
      description: "Test JS service",
      version: "0.1"
    }).addMethod(new ServiceMethod("hello").optional("name")),
    onFrame: onFrame,
    onRpcCall: onRpcCall
  });
  //await service.subscribeOIDs(["#"], EventKind.Any);
  await service.waitCore(false, 2);
  await service.block();
  console.log("exiting");
  process.exit(0);
};

main();
