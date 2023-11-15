export { ServiceMethod, ServiceInfo } from "./info";
export { ItemKind, OID } from "./oid";
export type { ACIData, ACLData, XCallData, ACLAllowDeny } from "./aaa";
export { ACI, ACL, XCall, oidMatch, pathMatch } from "./aaa";
export { Service, noRpcMethod, sleep } from "./service";
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

/**
 * A self-test function
 */
export const selfTest = (): void => {
  console.log("aaa");
  selfTestAAA();
};

declare var process: any;
declare var Deno: any;

/**
 * Exit process with a result code
 *
 * @param {number} code - exit code (default: 0)
 *
 * @returns {void}
 */
export const exit = (code: number = 0): void => {
  if (process) {
    process.exit(code);
  } else if (Deno) {
    Deno.exit(code);
  } else {
    throw Error("unable to exit process, unknown runtime");
  }
};
