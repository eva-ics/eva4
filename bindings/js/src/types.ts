export enum LogLevelCode {
  Trace = 0,
  Debug = 10,
  Info = 20,
  Warn = 30,
  Error = 40
}

export enum LogLevelName {
  Trace = "trace",
  Debug = "debug",
  Info = "info",
  Warn = "warn",
  Error = "error"
}

export enum EventKind {
  Any = "any",
  Local = "local",
  Remote = "remote",
  RemoteArchive = "remote_archive"
}

export enum EvaErrorCode {
  NotFound = -32001,
  AccessDenied = -32002,
  SystemError = -32003,
  Other = -32004,
  NotReady = -32005,
  Unsupported = -32006,
  CoreError = -32007,
  Timeout = -32008,
  InvalidData = -32009,
  FuncFailed = -32010,
  Aborted = -32011,
  AlreadyExists = -32012,
  Busy = -32013,
  MethodNotImplemented = -32014,
  TokenRestricted = -32015,
  Io = -32016,
  Registry = -32017,
  EvahiAuthRequired = -32018,
  AccessDeniedMoreDataRequired = -32022,
  Parse = -32700,
  InvalidRequest = -32600,
  MethodNotFound = -32601,
  InvalidParams = -32602,
  RpcInternal = -32603,
  BusClientNotRegistered = -32113,
  BusData = -32114,
  BusIo = -32115,
  BusOther = -32116,
  BusNotSupported = -32117,
  BusBusy = -32118,
  BusNotDelivered = -32119,
  BusTimeout = -32120
}

export enum EapiTopic {
  RawStateTopic = "RAW/",
  LocalStateTopic = "ST/LOC/",
  RemoteStateTopic = "ST/REM/",
  RemoteArchiveStateTopic = "ST/RAR/",
  AnyStateTopic = "ST/+/",
  ReplicationStateTopic = "RPL/ST/",
  ReplicationInventoryTopic = "RPL/INVENTORY/",
  ReplicationNodeStateTopic = "RPL/NODE/",
  LogInputTopic = "LOG/IN/",
  LogEventTopic = "LOG/EV/",
  LogCallTraceTopic = "LOG/TR/",
  ServiceStatusTopic = "SVC/ST",
  AaaAclTopic = "AAA/ACL/",
  AaaKeyTopic = "AAA/KEY/",
  AaaUserTopic = "AAA/USER/",
  ActionStatus = "ACT/"
}

export class EvaError {
  code: number;
  message?: string;

  constructor(code: number, message?: string) {
    this.code = code;
    this.message = message;
  }
}

export enum ServicePayloadKind {
  Initial = 1,
  Ping = 0
}

export enum ServiceStatus {
  Ready = "ready",
  Terminating = "terminating"
}

export const itemStatusError = -1;

export const sleepStep = 100;

export interface InitialPayload {
  version: number;
  system_name: string;
  id: string;
  command: string;
  prepare_command: string | null;
  data_path: string;
  timeout: InitialTimeoutConfig;
  core: InitialCoreInfo;
  bus: InitialBusConfig;
  config: any | null;
  workers: number;
  user: string | null;
  react_to_fail: boolean;
  fail_mode: boolean;
  fips: boolean;
  call_tracing: boolean;
}

export interface InitialTimeoutConfig {
  startup: number;
  shutdown: number;
  default: number;
}

export interface InitialBusConfig {
  type: string;
  path: string;
  timeout: number | null;
  buf_size: number;
  buf_ttl: number;
  queue_size: number;
}

export interface InitialCoreInfo {
  build: number;
  version: string;
  eapi_version: number;
  path: string;
  log_level: number;
  active: boolean;
}

export enum ActionStatusCode {
  Created = 0b0000_0000,
  Accepted = 0b0000_0001,
  Pending = 0b0000_0010,
  Running = 0b0000_1000,
  Completed = 0b0000_1111,
  Failed = 0b1000_0000,
  Canceled = 0b1000_0001,
  Terminated = 0b1000_0010
}
