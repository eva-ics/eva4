import {
  ServiceInfo,
  ServiceMethod,
  Service,
  pack,
  unpack,
  EventKind,
  Action,
  XCall,
  XCallData,
  EapiTopic,
  noRpcMethod,
  RpcEvent,
  Logger,
  sleep
} from "@eva-ics/sdk";

let service: Service;
/** The service can log messages to stdout (info) and stderr (error) via
 * console.log methods but it is highly recommended to use the bus logger which
 * is available automatically after the service bus has been initialized */
let log: Logger;

/** Bus frame handler */
const onFrame = async (e: RpcEvent): Promise<void> => {
  if (!service.isActive()) {
    return;
  }
  const payload = unpack(e.frame.getPayload());
  // payloads may contain data, unserializable by the default JSON, so better
  // log them to stdout
  console.log(
    "sender:",
    e.frame.primary_sender,
    "topic:",
    e.frame.topic,
    "payload:",
    payload
  );
};

/** RPC calls handler */
const onRpcCall = async (e: RpcEvent): Promise<Buffer | undefined> => {
  service.needReady();
  const method = e.method?.toString();
  const payload = unpack(e.getPayload());
  switch (method) {
    // process HTTP x-calls
    case "x":
      const xcall = new XCall(payload as XCallData);
      xcall.acl.requireAdmin();
      console.log(xcall);
      return pack({ ok: true });
    // process lmacro executions
    case "run":
      const lAction = new Action(payload);
      console.log(lAction);
      await service.controller.eventRunning(lAction);
      await service.controller.eventFailed(
        lAction,
        "lmacro execution",
        "execution failed: not supported",
        -15
      );
      return;
    // process unit actions
    case "action":
      const uAction = new Action(payload);
      console.log(uAction);
      await service.controller.eventRunning(uAction);
      await service.controller.eventCompleted(uAction, "all fine");
      const path = uAction.oid.asPath();
      await service.bus.publish(
        `${EapiTopic.RawStateTopic}${path}`,
        pack({ status: 1, value: uAction.params?.value })
      );
      return;
    // example RPC method which deals with the params payload
    case "hello":
      return payload?.name ? pack(`hello ${payload.name}`) : pack("hi there");
    // get service initial payload
    case "config":
      return pack(service.initial);
    default:
      noRpcMethod();
  }
};

const main = async () => {
  service = new Service();
  // the service initial payload must be loaded
  await service.load();
  // initialize the local bus and RPC engine
  await service.init({
    info: new ServiceInfo({
      author: "Bohemia Automation",
      description: "Test JS service",
      version: "0.0.1"
    })
      .addMethod(new ServiceMethod("hello").optional("name"))
      .addMethod(new ServiceMethod("config")),
    onFrame: onFrame,
    onRpcCall: onRpcCall
  });
  // set the global logger
  log = service.logger;
  // subscribe to item events
  await service.subscribeOIDs(["#"], EventKind.Any);
  // block the service while active
  log.warn("ready");
  await service.block();
  log.warn("exiting");
  // wait a bit to let tasks finish
  await sleep(500);
  // terminate the process with no error
  process.exit(0);
};

main();
