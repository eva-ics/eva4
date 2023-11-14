const sleep = require("sleep-promise");

import { Bus, Rpc, BusErrorCode, QoS, RpcEvent } from "busrt";
import { OID } from "./oid";
import { Logger } from "./log";
import {
  EvaErrorCode,
  EventKind,
  EapiTopic,
  ServicePayloadKind,
  EvaError,
  ServiceStatus,
  sleepStep,
  InitialPayload,
  InitialTimeoutConfig
} from "./types";
import { ServiceInfo, ServiceMethod } from "./info";
import { Controller } from "./controller";
import {
  readStdin,
  clockMonotonic,
  shellCommand,
  pack,
  unpack,
  getUserIds
} from "./tools";

export class Service {
  initial: InitialPayload;
  active: boolean;
  loaded: boolean;
  privilegesDropped: boolean;
  shutdownRequested: boolean;
  markedReady: boolean;
  markedTerminating: boolean;
  id: string;
  evaDir: string;
  systemName: string;
  dataPath: string | null;
  timeout: InitialTimeoutConfig;
  bus: Bus;
  rpc: Rpc;
  controller: Controller;
  onRpcCall: (e: RpcEvent) => Promise<Buffer | undefined>;
  svcInfo: ServiceInfo;
  logger: Logger;
  _svcInfoPacked: Buffer;

  constructor() {
    this.active = true;
    this.shutdownRequested = false;
    this.markedReady = false;
    this.markedTerminating = false;
    this.privilegesDropped = false;
    this.loaded = false;
  }

  async load() {
    if (this.loaded) {
      throw new Error("the service is already loaded");
    }
    const buf = await readStdin(1);
    if (buf[0] != ServicePayloadKind.Initial) {
      throw new Error("invalid payload");
    }
    const payloadLenBuf = await readStdin(4);
    const payloadLen = payloadLenBuf.readUInt32LE(0);
    const initial: InitialPayload = unpack(await readStdin(payloadLen));
    this.loaded = true;
    this.initial = initial;
    this.id = initial.id;
    this.systemName = initial.system_name;
    this.timeout = initial.timeout;
    this.evaDir = initial.core.path;
    const user = initial.user;
    if (user != "nobody") {
      this.dataPath = initial.data_path;
    } else {
      this.dataPath = null;
    }
    process.env["EVA_SYSTEM_NAME"] = this.systemName;
    process.env["EVA_DIR"] = this.evaDir;
    process.env["EVA_SVC_ID"] = this.id;
    process.env["EVA_SVC_DATA_PATH"] = this.dataPath || "";
    process.env["EVA_TIMEOUT"] = this.getTimeout().toString();
    if (initial.fail_mode && !initial.react_to_fail) {
      throw new Error(
        "the service is started in react-to-fail mode, but rtf is not supported by the service"
      );
    }
    if (this.initial.prepare_command) {
      await shellCommand(this.initial.prepare_command);
    }
    const me = this;
    process.stdin.on("readable", () => this._handleStdin(me));
    process.stdin.on("end", () => this.markTerminating(me));
  }

  needReady(): void {
    if (!this.active) {
      throw new EvaError(EvaErrorCode.RpcInternal, "service not ready");
    }
  }

  getConfig(): object {
    const config = this.initial.config;
    return config === null ? {} : config;
  }

  isModeNormal(): boolean {
    return !this.initial.fail_mode;
  }

  isModeRtf(): boolean {
    return this.initial.fail_mode;
  }

  async initBus(): Promise<void> {
    const busConfig = this.initial.bus;
    if (busConfig.type != "native") {
      throw new Error(`bus ${busConfig.type} is not supported`);
    }
    this.bus = new Bus(this.id);
    this.bus.timeout = busConfig.timeout;
    await this.bus.connect(busConfig.path);
    this.logger = new Logger(this.bus, this.initial.core.log_level);
    this.controller = new Controller(this.bus);
  }

  async initRpc(svcInfo: ServiceInfo): Promise<void> {
    const me = this;
    this.svcInfo = svcInfo;
    this._svcInfoPacked = pack(svcInfo);
    this.rpc = new Rpc(this.bus);
    this.onRpcCall = this.rpc.onCall;
    this.rpc.onCall = (e: RpcEvent) => this._handleRpcCall(e, me);
  }

  async init({
    info,
    onFrame,
    onRpcCall
  }: {
    info?: ServiceInfo;
    onFrame?: (frame: RpcEvent) => void;
    onRpcCall?: (e: RpcEvent) => Promise<Buffer | undefined>;
  }) {
    await this.initBus();
    this.dropPrivileges();
    if (info) {
      this.initRpc(info);
      if (onFrame) {
        this.rpc.onFrame = onFrame;
      }
      if (onRpcCall) {
        this.onRpcCall = onRpcCall;
      }
    }
  }

  async block(prepare: boolean = true): Promise<void> {
    if (prepare) {
      this.registerSignals();
      await this.markReady();
    }
    while (this.active) {
      await sleep(sleepStep);
    }
    if (prepare) {
      await this.markTerminating();
    }
  }

  dropPrivileges(): void {
    if (!this.privilegesDropped) {
      const user = this.initial.user;
      if (user) {
        const i = getUserIds(user);
        process.setgroups(i.groupIds);
        process.setgid(i.gid);
        process.setuid(i.uid);
      }
      this.privilegesDropped = true;
    }
  }

  registerSignals(): void {
    const me = this;
    process.on("SIGINT", () => (me.active = false));
    process.on("SIGTERM", () => (me.active = false));
  }

  async subscribeOIDs(
    sub: Array<string | OID>,
    kind: EventKind
  ): Promise<void> {
    let sfx;
    switch (kind) {
      case EventKind.Any:
        sfx = EapiTopic.AnyStateTopic;
        break;
      case EventKind.Local:
        sfx = EapiTopic.LocalStateTopic;
        break;
      case EventKind.Remote:
        sfx = EapiTopic.RemoteStateTopic;
        break;
      case EventKind.RemoteArchive:
        sfx = EapiTopic.RemoteArchiveStateTopic;
        break;
      default:
        throw new Error(`invalid event kind: ${kind}`);
    }
    const topics: Array<string> = [];
    for (const oid of sub) {
      let path;
      if (oid === "#") {
        path = oid;
      } else if (oid.constructor.name === "OID") {
        path = (oid as OID).asPath();
      } else {
        path = new OID(oid as string).asPath();
      }
      topics.push(`${sfx}${path}`);
    }
    await this.bus.subscribe(topics);
  }

  isActive(): boolean {
    return this.active;
  }

  isShutdownRequested(): boolean {
    return this.shutdownRequested;
  }

  getTimeout(): number {
    return this.timeout.default;
  }

  async markReady(): Promise<void> {
    if (!this.markedReady) {
      this.markedReady = true;
      await this._mark(ServiceStatus.Ready);
      await this.logger.info(
        this.svcInfo.description
          ? `${this.svcInfo.description} started`
          : "started"
      );
    }
  }

  _handleStdin(m?: Service) {
    const me = m || this;
    const buf = process.stdin.read(1);
    if (!buf || buf[0] != ServicePayloadKind.Ping) {
      me.markTerminating();
    }
  }

  async markTerminating(m?: Service): Promise<void> {
    const me = m || this;
    me.active = false;
    me.shutdownRequested = true;
    if (!me.markedTerminating) {
      me.markedTerminating = true;
      await me._mark(ServiceStatus.Terminating);
    }
  }

  async waitCore(wait_forever?: boolean, timeout?: number) {
    const t = timeout || this.timeout.startup || this.timeout.default;
    const waitUntil = clockMonotonic() + t;
    while (this.active) {
      try {
        const req = await this.rpc.call("eva.core", "test");
        const result = await req.waitCompleted();
        if ((unpack(result.getPayload()) as any).active === true) {
          return;
        }
      } catch (e: any) {}
      if (!wait_forever && waitUntil <= clockMonotonic()) {
        throw new Error("core wait timeout");
      }
      await sleep(sleepStep);
    }
  }

  async _mark(status: ServiceStatus): Promise<void> {
    await this.bus.publish("SVC/ST", pack({ status: status }), QoS.No);
  }

  async _handleRpcCall(e: RpcEvent, m?: Service): Promise<Buffer | undefined> {
    const me = m || this;
    const method = e.method.toString();
    switch (method) {
      case "test":
        return;
      case "info":
        return me._svcInfoPacked;
      case "stop":
        me.active = false;
        return;
      default:
        return me.onRpcCall(e);
    }
  }
}

export const noRpcMethod = () => {
  throw new EvaError(EvaErrorCode.MethodNotFound, "no such method");
};
