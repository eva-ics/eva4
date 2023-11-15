import sleep from "sleep-promise";
import { Bus, Rpc, QoS, RpcEvent } from "busrt";
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
import { ServiceInfo } from "./info";
import { Controller } from "./controller";
import {
  readStdin,
  clockMonotonic,
  shellCommand,
  pack,
  unpack,
  getUserIds
} from "./tools";

export { sleep };

/** The primary service class */
export class Service {
  /** Service initial payload */
  initial: InitialPayload;
  /** @ignore */
  active: boolean;
  /** @ignore */
  loaded: boolean;
  /** @ignore */
  privilegesDropped: boolean;
  /** @ignore */
  shutdownRequested: boolean;
  /** @ignore */
  markedReady: boolean;
  /** @ignore */
  markedTerminating: boolean;
  /** service id */
  id: string;
  /** EVA ICS directory */
  evaDir: string;
  /** EVA ICS node name */
  systemName: string;
  /** service data path (if available) */
  dataPath: string | null;
  /** service timeouts */
  timeout: InitialTimeoutConfig;
  /** service bus client */
  bus: Bus;
  /** service bus RPC client */
  rpc: Rpc;
  /** a controller instance for action processing */
  controller: Controller;
  /** @ignore */
  onRpcCall: (
    call: RpcEvent
  ) => Promise<Buffer | undefined> | Buffer | undefined;
  /** @ignore */
  svcInfo: ServiceInfo;
  /** Bus logger, recommened to use instead of console.log */
  logger: Logger;
  /** @ignore */
  _svcInfoPacked: Buffer;

  constructor() {
    this.active = true;
    this.shutdownRequested = false;
    this.markedReady = false;
    this.markedTerminating = false;
    this.privilegesDropped = false;
    this.loaded = false;
    this.initial = undefined as any;
    this.id = undefined as any;
    this.systemName = undefined as any;
    this.evaDir = undefined as any;
    this.dataPath = null;
    this.timeout = undefined as any;
    this.bus = undefined as any;
    this.rpc = undefined as any;
    this.controller = undefined as any;
    this.svcInfo = undefined as any;
    this.logger = undefined as any;
    this._svcInfoPacked = undefined as any;
    this.onRpcCall = undefined as any;
  }

  /** Service loader, must be always called first and once */
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
    if (!this.initial.config) {
      this.initial.config = {};
    }
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

  /**
   * Throws RPC error if the service is not ready yet
   *
   * @returns {void}
   * @throws {EvaError}
   */
  needReady(): void {
    if (!this.active) {
      throw new EvaError(EvaErrorCode.RpcInternal, "service not ready");
    }
  }

  /**
   * Get service configuration
   *
   * @returns {void}
   * @throws {EvaError}
   */
  getConfig(): object {
    return this.initial.config;
  }

  /**
   * Is service mode normal
   *
   * @returns {boolean}
   */
  isModeNormal(): boolean {
    return !this.initial.fail_mode;
  }

  /**
   * Is service mode react-to-fail
   *
   * @returns {boolean}
   */
  isModeRTF(): boolean {
    return this.initial.fail_mode;
  }

  /**
   * Init bus client
   *
   * @returns {Promise<void>}
   */
  async initBus(): Promise<void> {
    const busConfig = this.initial.bus;
    if (busConfig.type != "native") {
      throw new Error(`bus ${busConfig.type} is not supported`);
    }
    this.bus = new Bus(this.id);
    this.bus.timeout = busConfig.timeout || this.initial.timeout.default;
    await this.bus.connect(busConfig.path);
    this.logger = new Logger(this.bus, this.initial.core.log_level);
    this.controller = new Controller(this.bus);
  }

  /**
   * Init bus RPC client
   *
   * @param {ServiceInfo} svcInfo - service information
   *
   * @returns {Promise<void>}
   */
  async initRpc(svcInfo: ServiceInfo): Promise<void> {
    const me = this;
    this.svcInfo = svcInfo;
    this._svcInfoPacked = pack(svcInfo);
    this.rpc = new Rpc(this.bus);
    this.onRpcCall = this.rpc.onCall;
    this.rpc.onCall = (e: RpcEvent) => this._handleRpcCall(e, me);
  }

  /**
   * Automatically calls inutBus, initRpc and drops service privileges
   *
   * @param {ServiceInfo} params.info - service info
   * @param {frame: (RpcEvent) => Promise <void>} [params.onFrame] - EAPI frame handler
   * @param {(e: RpcEvent) => Promise<Buffer | undefined> | Buffer | undefined} [params.onRpcCall] - RPC call handler
   *
   * @returns {Promise<void>}
   */
  async init({
    info,
    onFrame,
    onRpcCall
  }: {
    info: ServiceInfo;
    onFrame?: (frame: RpcEvent) => Promise<void> | void;
    onRpcCall?: (
      e: RpcEvent
    ) => Promise<Buffer | undefined> | Buffer | undefined;
  }): Promise<void> {
    await this.initBus();
    this.dropPrivileges();
    this.initRpc(info);
    if (onFrame) {
      this.rpc.onFrame = onFrame;
    }
    if (onRpcCall) {
      this.onRpcCall = onRpcCall;
    }
  }

  /**
   * Blocks the service while active
   *
   * @param {boolean} prepare - register signals and mark active (default: true)
   *
   * @returns {Promise<void>}
   */
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

  /**
   * Drops service privileges to a restricted user
   *
   * @returns {void}
   */
  dropPrivileges(): void {
    if (!this.privilegesDropped) {
      const user = this.initial.user;
      if (user) {
        const i = getUserIds(user);
        (process as any).setgroups(i.groupIds);
        (process as any).setgid(i.gid);
        (process as any).setuid(i.uid);
      }
      this.privilegesDropped = true;
    }
  }

  /**
   * Registers service signal handlers (SIGINT, SIGTERM)
   *
   * @returns {void}
   */
  registerSignals(): void {
    const me = this;
    process.on("SIGINT", () => (me.active = false));
    process.on("SIGTERM", () => (me.active = false));
  }

  /**
   * Subscribes bus client to item events
   *
   * @param {Array<string|OID>} items - items to subscribe
   * @param {EventKind} kind - bus event kind
   *
   * @returns {Promise<void>}
   */
  async subscribeOIDs(
    items: Array<string | OID>,
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
    for (const oid of items) {
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

  /**
   * Is the service active
   *
   * @returns {boolean}
   */
  isActive(): boolean {
    return this.active;
  }

  /**
   * Is the service shutdown requested
   *
   * @returns {boolean}
   */
  isShutdownRequested(): boolean {
    return this.shutdownRequested;
  }

  /**
   * Get the service default timeout (seconds)
   *
   * @returns {number}
   */
  getTimeout(): number {
    return this.timeout.default;
  }

  /**
   * Manually mark the service ready
   *
   * @returns {Promise<void>}
   */
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

  /**
   * Manually mark the service terminating
   *
   * @returns {Promise<void>}
   */
  async markTerminating(m?: Service): Promise<void> {
    const me = m || this;
    me.active = false;
    me.shutdownRequested = true;
    if (!me.markedTerminating) {
      me.markedTerminating = true;
      await me._mark(ServiceStatus.Terminating);
    }
  }

  /**
   * Wait until the core become ready (in sub-functions)
   *
   * @param {boolean} wait_forever - wait forever
   * @param {number} timeout - wait timeout
   *
   * @returns {Promise<void>}
   */
  async waitCore(wait_forever?: boolean, timeout?: number): Promise<void> {
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

  /** @ignore */
  _handleStdin(m?: Service) {
    const me = m || this;
    const buf = process.stdin.read(1);
    if (!buf || buf[0] != ServicePayloadKind.Ping) {
      me.markTerminating();
    }
  }

  /** @ignore */
  async _mark(status: ServiceStatus): Promise<void> {
    await this.bus.publish("SVC/ST", pack({ status: status }), QoS.No);
  }

  /** @ignore */
  async _handleRpcCall(e: RpcEvent, m?: Service): Promise<Buffer | undefined> {
    const me = m || this;
    const method = e.method?.toString();
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

/**
 * Throws -32601 EAPI error (RPC method not found)
 *
 * @returns {void}
 * @throws {EvaError}
 */
export const noRpcMethod = (): void => {
  throw new EvaError(EvaErrorCode.MethodNotFound, "no such method");
};

/**
 * Creates a Service class instance, automatically calls load() and init()
 * methods (loads the initial payload, initializes the bus and RPC
 *
 * @returns {Service}
 */
export const createService = async ({
  info,
  onFrame,
  onRpcCall
}: {
  info: ServiceInfo;
  onFrame?: (frame: RpcEvent) => Promise<void> | void;
  onRpcCall?: (e: RpcEvent) => Promise<Buffer | undefined> | Buffer | undefined;
}): Promise<Service> => {
  const service = new Service();
  await service.load();
  await service.init({ info, onFrame, onRpcCall });
  return service;
};
