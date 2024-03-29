export { ServiceMethod, ServiceInfo } from "./info";
export { ItemKind, OID } from "./oid";
export type { ACIData, ACLData, XCallData, ACLAllowDeny } from "./aaa";
export { ACI, ACL, XCall, oidMatch, pathMatch } from "./aaa";
export { Service, noRpcMethod, sleep, createService } from "./service";
export { pack, unpack, clockMonotonic } from "./tools";
export { Logger } from "./log";
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
import { exit } from "node:process";
export { exit };

/**
 * A self-test function
 */
export const selfTest = (): void => {
  console.log("aaa");
  selfTestAAA();
};
