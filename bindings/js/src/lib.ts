export { ServiceMethod, ServiceInfo } from "./info";
export { ItemKind, OID } from "./oid";
export type { ACIData, ACLData, XCallData } from "./aaa";
export { ACI, ACL, XCall, oidMatch, pathMatch } from "./aaa";
export { Service, noRpcMethod } from "./service";
export { pack, unpack, clockMonotonic } from "./tools";
export {
  LogLevelCode,
  LogLevelName,
  EventKind,
  EvaErrorCode,
  EapiTopic,
  EvaError,
  itemStatusError,
  sleepStep
} from "./types";
export type {
  InitialPayload,
  InitialTimeoutConfig,
  InitialBusConfig,
  InitialCoreInfo
} from "./types";
export { Bus, QoS, Rpc, RpcEvent } from "busrt";
export type { BusActionStatus, BusAction, ActionParams } from "./controller";
export { Action, Controller } from "./controller";

import { selfTest as selfTestAAA } from "./aaa";

/**
 * A self-test function
 */
export const selfTest = (): void => {
  console.log("aaa");
  selfTestAAA();
};